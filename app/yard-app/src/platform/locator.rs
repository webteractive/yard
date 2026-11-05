//! The I/O around `yard_app_model::locate`: the daemon's LaunchAgent plist and which candidate
//! `yard` binaries are executable.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use yard_app_model::locate::{LocateInput, Location, locate};
use yard_core::render::launchd::read_agent;

/// `~/Library/LaunchAgents/dev.more.yard.daemon.plist`.
pub fn agent_plist(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents/dev.more.yard.daemon.plist")
}

/// Where this app's bundle keeps helper binaries (M7), when it has one.
pub fn bundle_helpers() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let helpers = exe.parent()?.parent()?.join("Helpers");
    helpers.is_dir().then_some(helpers)
}

fn executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Reads the agent plist (a missing or broken one is no agent) and checks the candidates.
pub fn location_from(
    root_arg: Option<PathBuf>,
    cli_setting: Option<PathBuf>,
    home: &Path,
    plist: &Path,
    bundle_helpers: Option<PathBuf>,
) -> Location {
    let agent = std::fs::read(plist).ok().and_then(|b| read_agent(&b).ok());
    let mut candidates = vec![];
    candidates.extend(cli_setting.clone());
    candidates.extend(agent.as_ref().and_then(|a| a.program.parent().map(|d| d.join("yard"))));
    candidates.extend(bundle_helpers.as_ref().map(|d| d.join("yard")));
    let executables: HashSet<PathBuf> = candidates.into_iter().filter(|p| executable(p)).collect();
    locate(&LocateInput {
        root_arg,
        cli_setting,
        agent,
        home: home.to_path_buf(),
        bundle_helpers,
        executables,
    })
}

/// The app's location: `--root`, the `YardCLIPath` setting, the real agent plist and bundle.
pub fn location(root_arg: Option<PathBuf>, cli_setting: Option<PathBuf>) -> Location {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    location_from(root_arg, cli_setting, &home, &agent_plist(&home), bundle_helpers())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use yard_core::render::launchd::daemon_plist;

    #[test]
    fn location_reads_a_temp_agent_plist() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("target/release");
        std::fs::create_dir_all(&bin).unwrap();
        let yard = bin.join("yard");
        std::fs::write(&yard, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&yard, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plist = dir.path().join("dev.more.yard.daemon.plist");
        std::fs::write(
            &plist,
            daemon_plist(&bin.join("yardd"), Some(Path::new("/tmp/yard-dev")), Path::new("/l")),
        )
        .unwrap();
        let l = location_from(None, None, Path::new("/Users/me"), &plist, None);
        assert_eq!(l.root, PathBuf::from("/tmp/yard-dev"));
        assert_eq!(l.yard, Some(yard));
        assert!(l.agent_installed);
        let none = location_from(None, None, Path::new("/Users/me"), &dir.path().join("missing.plist"), None);
        assert_eq!(
            (none.root, none.yard, none.agent_installed),
            (PathBuf::from("/Users/me/.yard"), None, false)
        );
    }
}
