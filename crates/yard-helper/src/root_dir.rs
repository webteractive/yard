//! Whether a data root is safe for root to run nginx from: every directory on the path
//! must belong to the owner or root and be writable by nobody else. Otherwise another
//! local user could swap the nginx config root executes. nginx is given the path as
//! written, so the walk resolves it one component at a time like the kernel does,
//! checking each directory it passes and each symlink it follows (a symlink must belong
//! to the owner or root, and its target is walked too). A world-writable directory with
//! the sticky bit (`/tmp`) is fine above the root, since others cannot replace what they
//! do not own; a root that does not exist yet is checked as far as it exists, and
//! checked again before nginx runs.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

const STICKY: u32 = 0o1000;
/// The kernel's own limit on symlinks in one lookup (`MAXSYMLINKS`).
const MAX_SYMLINKS: usize = 32;

pub fn check(root: &Path, owner_uid: u32) -> Result<(), YardError> {
    if !root.is_absolute() {
        return Err(refused(format!("the data root {} is not an absolute path", root.display()))
            .with_hint("pass an absolute --root"));
    }
    let check_dir = |dir: &Path, is_root: bool| -> Result<(), YardError> {
        let meta =
            std::fs::metadata(dir).map_err(|e| refused(format!("cannot read {}: {e}", dir.display())))?;
        owned(dir, meta.uid(), owner_uid, root)?;
        let shared = meta.mode() & 0o022 != 0;
        let sticky_parent = meta.mode() & STICKY != 0 && !is_root;
        if shared && !sticky_parent {
            return Err(refused(format!(
                "{} is writable by other users, so root will not run nginx from {}",
                dir.display(),
                root.display()
            ))
            .with_hint(format!("chmod go-w {:?}", dir.display().to_string())));
        }
        Ok(())
    };

    let mut current = PathBuf::from("/");
    check_dir(&current, false)?;
    let mut pending = names(root);
    let mut followed = 0;
    while let Some(name) = pending.pop_front() {
        if name == ".." {
            current.pop();
            continue;
        }
        let next = current.join(&name);
        let meta = match std::fs::symlink_metadata(&next) {
            Ok(meta) => meta,
            // The rest is created later, inside the last directory checked.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(refused(format!("cannot read {}: {e}", next.display()))),
        };
        if meta.file_type().is_symlink() {
            owned(&next, meta.uid(), owner_uid, root)?;
            followed += 1;
            if followed > MAX_SYMLINKS {
                return Err(refused(format!("too many symlinks resolving {}", root.display())));
            }
            let target = std::fs::read_link(&next)
                .map_err(|e| refused(format!("cannot read {}: {e}", next.display())))?;
            if target.is_absolute() {
                current = PathBuf::from("/");
            }
            for part in names(&target).into_iter().rev() {
                pending.push_front(part);
            }
            continue;
        }
        check_dir(&next, pending.is_empty())?;
        current = next;
    }
    Ok(())
}

/// A path's names in order; `/` and `.` drop out and `..` is kept as `..`.
fn names(path: &Path) -> VecDeque<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_owned()),
            Component::ParentDir => Some("..".into()),
            _ => None,
        })
        .collect()
}

fn owned(path: &Path, uid: u32, owner_uid: u32, root: &Path) -> Result<(), YardError> {
    if uid == owner_uid || uid == 0 {
        return Ok(());
    }
    Err(refused(format!(
        "{} belongs to uid {uid}, so root will not run nginx from {}",
        path.display(),
        root.display()
    ))
    .with_hint("use a data root inside your home directory"))
}

fn refused(message: String) -> YardError {
    YardError::new(ErrorCode::HelperRefused, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use yard_proto::ErrorCode;

    fn me() -> u32 {
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() }
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn own_private_dir_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        check(dir.path(), me()).unwrap();
        check(&dir.path().join("not/yet/created"), me()).unwrap();
    }

    #[test]
    fn writable_by_others_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Yard");
        std::fs::create_dir(&root).unwrap();
        chmod(&root, 0o777);
        let e = check(&root, me()).unwrap_err();
        assert_eq!(e.code, ErrorCode::HelperRefused);
        assert!(e.hint.unwrap().contains("chmod go-w"));
        chmod(&root, 0o755);
        chmod(dir.path(), 0o775);
        assert!(check(&root, me()).is_err(), "a group-writable parent counts too");
        assert!(check(&root.join("later"), me()).is_err(), "so does a missing root's parent");
        chmod(dir.path(), 0o700);
    }

    #[test]
    fn someone_elses_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(check(dir.path(), me() + 1).unwrap_err().code, ErrorCode::HelperRefused);
    }

    #[test]
    fn relative_root_is_refused() {
        assert_eq!(check(Path::new("yard-dev"), me()).unwrap_err().code, ErrorCode::HelperRefused);
    }

    #[test]
    fn symlink_through_a_shared_dir_is_refused() {
        // nginx is given the path as written, so the directory holding a symlink on it
        // matters even when the symlink's target is safe.
        let dir = tempfile::tempdir().unwrap();
        let safe = dir.path().join("safe");
        let shared = dir.path().join("shared");
        std::fs::create_dir(&safe).unwrap();
        std::fs::create_dir(&shared).unwrap();
        chmod(&shared, 0o777);
        std::os::unix::fs::symlink(&safe, shared.join("link")).unwrap();
        let e = check(&shared.join("link"), me()).unwrap_err();
        assert!(e.message.contains("writable by other users"), "{}", e.message);
        chmod(&shared, 0o755);
    }

    #[test]
    fn symlinks_are_followed_and_their_targets_checked() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::os::unix::fs::symlink(&shared, dir.path().join("abs")).unwrap();
        std::os::unix::fs::symlink("shared", dir.path().join("rel")).unwrap();
        check(&dir.path().join("abs"), me()).unwrap();
        check(&dir.path().join("rel/../rel/later"), me()).unwrap();
        chmod(&shared, 0o777);
        assert!(check(&dir.path().join("abs"), me()).is_err(), "the target is the root here");
        assert!(check(&dir.path().join("rel"), me()).is_err());
        chmod(&shared, 0o755);
    }

    #[test]
    fn symlink_loop_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("b", dir.path().join("a")).unwrap();
        std::os::unix::fs::symlink("a", dir.path().join("b")).unwrap();
        assert_eq!(check(&dir.path().join("a"), me()).unwrap_err().code, ErrorCode::HelperRefused);
    }

    #[test]
    fn sticky_tmp_is_a_safe_parent() {
        let dir = tempfile::Builder::new().prefix("yard-test-").tempdir_in("/tmp").unwrap();
        check(dir.path(), me()).unwrap();
    }
}
