//! Whether a data root is safe for root to run nginx from: the root and every directory
//! above it must belong to the owner or root and be writable by nobody else. Otherwise
//! another local user could swap the nginx config root executes. A world-writable
//! directory with the sticky bit (`/tmp`) is fine above the root, since others cannot
//! replace what they do not own; a root that does not exist yet is checked from its
//! nearest existing parent, and checked again before nginx runs.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use yard_proto::{ErrorCode, YardError};

const STICKY: u32 = 0o1000;

pub fn check(root: &Path, owner_uid: u32) -> Result<(), YardError> {
    if !root.is_absolute() {
        return Err(refused(format!("the data root {} is not an absolute path", root.display()))
            .with_hint("pass an absolute --root"));
    }
    let mut existing = root;
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().unwrap_or(Path::new("/"));
    }
    let real = std::fs::canonicalize(existing)
        .map_err(|e| refused(format!("cannot resolve {}: {e}", existing.display())))?;
    let is_root = existing == root;
    for (i, dir) in real.ancestors().enumerate() {
        let meta =
            std::fs::metadata(dir).map_err(|e| refused(format!("cannot read {}: {e}", dir.display())))?;
        if meta.uid() != owner_uid && meta.uid() != 0 {
            return Err(refused(format!(
                "{} belongs to uid {}, so root will not run nginx from {}",
                dir.display(),
                meta.uid(),
                root.display()
            ))
            .with_hint("use a data root inside your home directory"));
        }
        let shared = meta.mode() & 0o022 != 0;
        let sticky_parent = meta.mode() & STICKY != 0 && !(is_root && i == 0);
        if shared && !sticky_parent {
            return Err(refused(format!(
                "{} is writable by other users, so root will not run nginx from {}",
                dir.display(),
                root.display()
            ))
            .with_hint(format!("chmod go-w {:?}", dir.display().to_string())));
        }
    }
    Ok(())
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
    fn sticky_tmp_is_a_safe_parent() {
        let dir = tempfile::Builder::new().prefix("yard-test-").tempdir_in("/tmp").unwrap();
        check(dir.path(), me()).unwrap();
    }
}
