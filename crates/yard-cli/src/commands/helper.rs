//! `sudo yard helper install|uninstall`: the root LaunchDaemon `dev.more.yard.helper`.
//!
//! Everything the helper trusts (owner uid, data root, brew prefix) is fixed into its
//! plist here, at install time, from `SUDO_UID`/`SUDO_USER`; callers never send it.

use super::lifecycle::{launchctl, sibling, unloaded};
use super::{Ctx, Outcome};
use serde_json::Value;
use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::paths::{HELPER_BINARY, HELPER_LABEL, HELPER_SOCKET, RESOLVER_DIR, RESOLVER_MARKER};
use yard_core::render::launchd::helper_plist;
use yard_daemon::helper_client::HelperClient;
use yard_proto::{ErrorCode, YardError};

const PLIST: &str = "/Library/LaunchDaemons/dev.more.yard.helper.plist";
/// M1's helper ran nginx as root. Sent to the helper being replaced, so its nginx lets go
/// of the ports before yardd starts its own.
const LEGACY_NGINX_STOP: &str = "nginx.stop";
const READY_WAIT: Duration = Duration::from_secs(5);

/// Whose helper this is: the user who ran sudo.
struct Owner {
    uid: u32,
    user: String,
    home: PathBuf,
}

pub async fn install(ctx: &Ctx) -> Outcome {
    require_root("install")?;
    let owner = owner()?;
    let source = sibling("yard-helper")?;
    // Under sudo, HOME may be root's; the data root is the owner's unless one was given.
    let paths = if ctx.root_overridden {
        ctx.paths.clone()
    } else {
        Paths::from_vars(None, Some(owner.home.clone()))?
    };
    let paths = Paths::new(helper_root(paths.root(), owner.uid)?);
    let brew = Brew::detect(&|p: &Path| p.exists())?;

    let binary = Path::new(HELPER_BINARY);
    place(binary, &std::fs::read(&source).map_err(|e| io(&source, e))?, 0o755)?;
    let plist = helper_plist(binary, owner.uid, &owner.user, paths.root(), &brew.prefix);
    place(Path::new(PLIST), &plist, 0o644)?;

    let service = format!("system/{HELPER_LABEL}");
    stop_legacy_nginx().await;
    // Not loaded yet is fine; a loaded one must go so the new binary and plist load.
    if launchctl(&["bootout", &service]).is_ok() {
        unloaded(&service).await;
    }
    let removed = remove_owned_by(&legacy_root_files(&paths), 0);
    launchctl(&["bootstrap", "system", PLIST])?;

    let helper = HelperClient::new(HELPER_SOCKET);
    let deadline = Instant::now() + READY_WAIT;
    let status = loop {
        match helper.status().await {
            Ok(status) => break status,
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(e) => {
                return Err(YardError::new(
                    ErrorCode::HelperNotInstalled,
                    format!("the helper did not start: {}", e.message),
                )
                .with_hint(format!("sudo tail -50 /var/log/{HELPER_LABEL}.log"))
                .into());
            }
        }
    };
    ctx.emit(&status, || {
        let mut out = String::new();
        for file in &removed {
            out.push_str(&format!("Removed {}, which M1's root nginx left behind.\n", file.display()));
        }
        out.push_str(&format!(
            "Installed the helper {} for {} (data root {}).\nyard start runs yardd next.\n",
            status.version,
            owner.user,
            paths.root().display()
        ));
        out
    });
    Ok(0)
}

/// Asks the running helper to stop the root nginx M1 started. A helper without the verb,
/// or no helper at all, has no nginx to stop.
async fn stop_legacy_nginx() {
    match HelperClient::new(HELPER_SOCKET).call::<Value>(LEGACY_NGINX_STOP, Value::Null).await {
        Err(e) if !matches!(e.code, ErrorCode::MethodNotFound | ErrorCode::HelperNotInstalled) => eprintln!(
            "warning: could not stop M1's nginx ({}); its pid is in run/nginx.pid under the data root",
            e.message
        ),
        _ => {}
    }
}

/// Files M1's root nginx created in the data root. yardd's nginx, running as the user,
/// cannot open them, and recreates both once they are gone.
fn legacy_root_files(paths: &Paths) -> [PathBuf; 2] {
    [paths.logs_dir().join("nginx-error.log"), paths.nginx_pid()]
}

/// Removes each entry `uid` owns, other than a directory, whose parent is a real
/// directory. Root removes rather than chowns: the owner can make these names hard links
/// to any root file on the volume, and unlinking a link never touches what it points at.
/// Returns the entries it removed.
fn remove_owned_by(files: &[PathBuf], uid: u32) -> Vec<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut removed = vec![];
    for file in files {
        let parent_is_dir =
            file.parent().and_then(|p| std::fs::symlink_metadata(p).ok()).is_some_and(|m| m.is_dir());
        let Ok(meta) = std::fs::symlink_metadata(file) else { continue };
        if !parent_is_dir || meta.is_dir() || meta.uid() != uid {
            continue;
        }
        match std::fs::remove_file(file) {
            Ok(()) => removed.push(file.clone()),
            Err(e) => eprintln!("warning: cannot remove {}: {e}", file.display()),
        }
    }
    removed
}

pub async fn uninstall(ctx: &Ctx) -> Outcome {
    require_root("uninstall")?;
    let _ = launchctl(&["bootout", &format!("system/{HELPER_LABEL}")]);
    for file in [PLIST, HELPER_BINARY] {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(YardError::internal(format!("cannot remove {file}: {e}")).into()),
        }
    }
    let removed = remove_resolvers(Path::new(RESOLVER_DIR))?;
    ctx.emit(&serde_json::json!({ "removed_resolvers": removed }), || {
        let mut out = "Removed the helper.\n".to_string();
        for file in &removed {
            out.push_str(&format!("Removed {file}.\n"));
        }
        out
    });
    Ok(0)
}

/// The data root the helper's plist names: absolute, since the helper runs from `/`, and
/// writable by nobody but the owner, since root changes files in it on install.
fn helper_root(root: &Path, owner_uid: u32) -> Result<PathBuf, YardError> {
    let root = std::path::absolute(root).map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot resolve {}: {e}", root.display()))
    })?;
    yard_helper::root_dir::check(&root, owner_uid)?;
    Ok(root)
}

fn require_root(verb: &str) -> Result<(), YardError> {
    // SAFETY: geteuid cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }
    Err(YardError::new(
        ErrorCode::InvalidArgument,
        format!("yard helper {verb} changes system files: run with sudo"),
    )
    .with_hint(format!("sudo yard helper {verb}")))
}

fn owner() -> Result<Owner, YardError> {
    let refuse = || {
        YardError::new(ErrorCode::InvalidArgument, "SUDO_UID and SUDO_USER are not set")
            .with_hint("run sudo yard helper install from your own account, not a root shell")
    };
    let uid: u32 = std::env::var("SUDO_UID").ok().and_then(|u| u.parse().ok()).ok_or_else(refuse)?;
    let user = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty()).ok_or_else(refuse)?;
    if uid == 0 {
        return Err(refuse());
    }
    let home = home_of(&user).ok_or_else(|| {
        YardError::internal(format!("no home directory for {user}")).with_hint("pass --root <path>")
    })?;
    Ok(Owner { uid, user, home })
}

fn home_of(user: &str) -> Option<PathBuf> {
    let name = CString::new(user).ok()?;
    // SAFETY: getpwnam returns null or a pointer into static storage that stays valid
    // until the next getpw* call, and pw_dir is read before any other call.
    unsafe {
        let pw = libc::getpwnam(name.as_ptr());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        Some(PathBuf::from(dir))
    }
}

/// Writes `bytes` beside `path`, sets root:wheel and `mode`, then renames it into place.
fn place(path: &Path, bytes: &[u8], mode: u32) -> Result<(), YardError> {
    let dir =
        path.parent().ok_or_else(|| YardError::internal(format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let tmp = dir.join(format!(".{}.yard-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    let write = || -> std::io::Result<()> {
        std::fs::write(&tmp, bytes)?;
        std::os::unix::fs::chown(&tmp, Some(0), Some(0))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| io(path, e))
}

/// The resolver files that carry Yard's marker; others' files are left alone.
fn remove_resolvers(dir: &Path) -> Result<Vec<String>, YardError> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(vec![]) };
    let mut removed = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = std::fs::read_to_string(&path).is_ok_and(|t| t.lines().next() == Some(RESOLVER_MARKER));
        if ours {
            std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
            removed.push(path.display().to_string());
        }
    }
    Ok(removed)
}

fn io(path: &Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> u32 {
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() }
    }

    #[test]
    fn legacy_root_files_are_m1_nginx_outputs() {
        let paths = Paths::new("/r");
        assert_eq!(
            legacy_root_files(&paths),
            [PathBuf::from("/r/logs/nginx-error.log"), PathBuf::from("/r/run/nginx.pid")]
        );
    }

    #[test]
    fn removes_only_regular_files_the_uid_owns() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let (logs, elsewhere) = (dir.path().join("logs"), dir.path().join("elsewhere"));
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let target = elsewhere.join("target");
        std::fs::write(&target, "keep").unwrap();
        let plain = logs.join("nginx-error.log");
        std::fs::write(&plain, "m1").unwrap();
        let hardlink = logs.join("hardlink");
        std::fs::hard_link(&target, &hardlink).unwrap();
        let link = logs.join("link");
        symlink(&target, &link).unwrap();
        // A parent that is a symlink is not followed either.
        let run = dir.path().join("run");
        symlink(&elsewhere, &run).unwrap();

        let files = [plain.clone(), hardlink.clone(), link.clone(), run.join("target")];
        assert_eq!(remove_owned_by(&files, me()), [plain.clone(), hardlink.clone(), link.clone()]);
        assert!(!plain.exists() && !hardlink.exists() && std::fs::symlink_metadata(&link).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep", "the linked file is untouched");

        std::fs::write(&plain, "mine").unwrap();
        assert!(
            remove_owned_by(std::slice::from_ref(&plain), me() + 1).is_empty(),
            "another uid's file stays"
        );
        assert!(plain.exists());
    }

    #[test]
    fn helper_root_is_absolute_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(helper_root(dir.path(), me()).unwrap(), dir.path());
        assert!(helper_root(Path::new("yard-dev"), me()).unwrap().is_absolute());

        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(helper_root(&shared, me()).unwrap_err().code, ErrorCode::HelperRefused);
    }
}
