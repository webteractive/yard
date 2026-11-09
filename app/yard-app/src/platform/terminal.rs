//! The Terminal hand-off (M6 design §10.3): a `0700` `.command` file in the app's temp directory,
//! opened with the terminal app; stale ones are removed at launch.

use crate::platform::open::Opener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use yard_app_model::terminal::terminal_script;
use yard_proto::YardError;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// `sudo <yard> [--root <root>] helper install`, built from the located `yard`, never a hint.
pub fn helper_install_args(yard: &Path, root: &Path, root_is_default: bool) -> Vec<String> {
    let mut args = vec!["sudo".to_string(), yard.display().to_string()];
    if !root_is_default {
        args.extend(["--root".to_string(), root.display().to_string()]);
    }
    args.extend(["helper".to_string(), "install".to_string()]);
    args
}

/// Writes `yard-<id>.command` in `dir`, mode `0700`, running `args`.
pub fn write_command(dir: &Path, args: &[String]) -> Result<PathBuf, YardError> {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let id = format!("{}-{nanos}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let file = dir.join(format!("yard-{id}.command"));
    let fail = |e: std::io::Error| YardError::internal(format!("cannot write {}: {e}", file.display()));
    std::fs::write(&file, terminal_script(args)).map_err(fail)?;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).map_err(fail)?;
    Ok(file)
}

/// Removes `yard-*.command` files a previous launch left in `dir`.
pub fn clean_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("yard-") && name.ends_with(".command") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub struct TerminalLauncher {
    temp: PathBuf,
    /// The terminal app chosen in Settings, read at each launch.
    terminal_app: Box<dyn Fn() -> Option<PathBuf>>,
}

impl TerminalLauncher {
    pub fn new(temp: PathBuf, terminal_app: impl Fn() -> Option<PathBuf> + 'static) -> TerminalLauncher {
        TerminalLauncher { temp, terminal_app: Box::new(terminal_app) }
    }

    pub fn terminal_app(&self) -> Option<PathBuf> {
        (self.terminal_app)()
    }

    /// Writes the command file and opens it with the terminal app (Terminal.app when unset).
    pub fn launch(&self, args: &[String]) -> Result<(), YardError> {
        let file = write_command(&self.temp, args)?;
        Opener::open_with(&file, self.terminal_app().as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The terminal app is read at each launch, so a new one chosen in Settings is used at once.
    #[test]
    fn the_terminal_app_is_read_when_launching() {
        let chosen = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
        let c = chosen.clone();
        let launcher = TerminalLauncher::new(std::env::temp_dir(), move || c.borrow().clone());
        assert_eq!(launcher.terminal_app(), None);
        *chosen.borrow_mut() = Some(PathBuf::from("/Applications/Ghostty.app"));
        assert_eq!(launcher.terminal_app(), Some(PathBuf::from("/Applications/Ghostty.app")));
    }

    #[test]
    fn command_file_is_0700_and_quoted() {
        let dir = tempfile::tempdir().unwrap();
        let args = helper_install_args(Path::new("/Users/a b/yard"), Path::new("/tmp/r"), false);
        assert_eq!(args, ["sudo", "/Users/a b/yard", "--root", "/tmp/r", "helper", "install"]);
        let file = write_command(dir.path(), &args).unwrap();
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("yard-") && name.ends_with(".command"), "{name}");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o700);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("'sudo' '/Users/a b/yard' '--root' '/tmp/r' 'helper' 'install'"), "{text}");
        assert_eq!(
            helper_install_args(Path::new("/y"), Path::new("/r"), true),
            ["sudo", "/y", "helper", "install"]
        );
    }

    #[test]
    fn stale_command_files_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let old = write_command(dir.path(), &["true".to_string()]).unwrap();
        let other = dir.path().join("keep.command");
        std::fs::write(&other, "").unwrap();
        clean_stale(dir.path());
        assert!(!old.exists());
        assert!(other.exists(), "only yard-*.command files go");
    }
}
