//! How this Yard was installed, how release versions compare, the release feed
//! (`latest.json`), and what `yard uninstall` removes (M7 spec §4–§6, §10). Pure: the CLI
//! gathers the facts and does the work.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use yard_proto::{ErrorCode, YardError};

/// Where releases are published; `latest/download` redirects to the newest release's assets
/// without GitHub's rate-limited API.
pub const RELEASE_URL: &str = "https://github.com/webteractive/yard/releases/latest/download";

/// `YARD_RELEASE_URL` when set (tests point it at a `file://` directory), else [`RELEASE_URL`].
pub fn release_url() -> String {
    release_url_from(std::env::var("YARD_RELEASE_URL").ok())
}

pub fn release_url_from(var: Option<String>) -> String {
    match var {
        Some(url) if !url.is_empty() => url.trim_end_matches('/').to_string(),
        _ => RELEASE_URL.to_string(),
    }
}

/// Lower-case hex SHA-256, as `shasum -a 256` and `latest.json` write it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A release version, `MAJOR.MINOR.PATCH`, with an optional leading `v`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl FromStr for Version {
    type Err = YardError;

    fn from_str(s: &str) -> Result<Version, YardError> {
        let bad = || YardError::new(ErrorCode::InvalidArgument, format!("{s:?} is not a version like 0.1.0"));
        let parts: Vec<u32> = s
            .strip_prefix('v')
            .unwrap_or(s)
            .split('.')
            .map(|p| p.parse().map_err(|_| bad()))
            .collect::<Result<_, _>>()?;
        match parts[..] {
            [major, minor, patch] => Ok(Version { major, minor, patch }),
            _ => Err(bad()),
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `latest.json`, the release feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Latest {
    pub version: String,
    pub zip: String,
    pub sha256: String,
    pub notes_url: String,
}

/// How the running `yard` got onto this Mac, which decides who may update it (spec §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Not inside a `Yard.app`: a `target/release` build.
    Dev,
    /// A `Yard.app` that `install.sh` put there; `yard self-update` updates it.
    Bundle { app: PathBuf },
    /// A `Yard.app` that brew's cask put there; only `brew upgrade` updates it.
    Brew { app: PathBuf },
}

impl InstallKind {
    pub fn app(&self) -> Option<&Path> {
        match self {
            InstallKind::Dev => None,
            InstallKind::Bundle { app } | InstallKind::Brew { app } => Some(app),
        }
    }
}

/// `exe` is the canonical path of the running `yard`; `caskroom_exists` says whether
/// `<brew prefix>/Caskroom/yard` exists.
pub fn install_kind(exe: &Path, caskroom_exists: bool) -> InstallKind {
    let app = (|| {
        if exe.file_name()? != "yard" {
            return None;
        }
        let helpers = exe.parent()?;
        let contents = helpers.parent()?;
        let app = contents.parent()?;
        let is_app = app.extension().is_some_and(|e| e == "app");
        (helpers.file_name()? == "Helpers" && contents.file_name()? == "Contents" && is_app)
            .then(|| app.to_path_buf())
    })();
    match app {
        None => InstallKind::Dev,
        Some(app) if caskroom_exists => InstallKind::Brew { app },
        Some(app) => InstallKind::Bundle { app },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    UpToDate,
    Available(Version),
}

/// Updates only move forward: a feed at or below the installed version is up to date.
pub fn update(installed: Version, latest: Version) -> Update {
    if latest > installed { Update::Available(latest) } else { Update::UpToDate }
}

/// One step of `yard uninstall`, in the order they run (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UninstallStep {
    StopDaemon,
    Untrust,
    HelperUninstall,
    RemoveData,
    RemovePrefs,
    RemoveBundle,
    RemoveBrewCask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallFacts {
    pub kind: InstallKind,
    pub agent_installed: bool,
    pub ca_exists: bool,
    pub helper_installed: bool,
    pub keep_data: bool,
    /// The data root is there: one never made, or already removed, is not a step.
    pub data_exists: bool,
}

pub fn uninstall_plan(f: &UninstallFacts) -> Vec<UninstallStep> {
    use UninstallStep::*;
    let mut steps = vec![];
    if f.agent_installed {
        steps.push(StopDaemon);
    }
    if f.ca_exists {
        steps.push(Untrust);
    }
    if f.helper_installed {
        steps.push(HelperUninstall);
    }
    if !f.keep_data && f.data_exists {
        steps.push(RemoveData);
    }
    steps.push(RemovePrefs);
    match f.kind {
        InstallKind::Dev => {}
        InstallKind::Bundle { .. } => steps.push(RemoveBundle),
        InstallKind::Brew { .. } => steps.push(RemoveBrewCask),
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(v("0.10.0") > v("0.9.9"));
        assert_eq!(v("v0.1.0"), v("0.1.0"));
        assert_eq!(v("1.2.3"), Version { major: 1, minor: 2, patch: 3 });
        for bad in ["0.1", "x", "1.2.3.4", "", "1.-2.3"] {
            let e = bad.parse::<Version>().unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{bad}");
        }
    }

    #[test]
    fn an_older_or_equal_feed_is_up_to_date() {
        assert_eq!(update(v("0.2.0"), v("0.1.0")), Update::UpToDate);
        assert_eq!(update(v("0.1.0"), v("0.1.0")), Update::UpToDate);
        assert_eq!(update(v("0.1.0"), v("0.1.1")), Update::Available(v("0.1.1")));
    }

    #[test]
    fn the_bundle_decides_the_install_kind() {
        let exe = Path::new("/Applications/Yard.app/Contents/Helpers/yard");
        let app = PathBuf::from("/Applications/Yard.app");
        assert_eq!(install_kind(exe, false), InstallKind::Bundle { app: app.clone() });
        assert_eq!(install_kind(exe, true), InstallKind::Brew { app });
        assert_eq!(install_kind(Path::new("/x/target/release/yard"), true), InstallKind::Dev);
        assert_eq!(install_kind(Path::new("/x/Yard.app/Contents/MacOS/yard"), false), InstallKind::Dev);
        assert_eq!(install_kind(Path::new("/x/Yard/Contents/Helpers/yard"), false), InstallKind::Dev);
    }

    #[test]
    fn latest_json_round_trips() {
        let json = r#"{"version":"0.1.1","zip":"https://x/Yard-0.1.1-arm64.zip","sha256":"ab","notes_url":"https://x/n","extra":1}"#;
        let l: Latest = serde_json::from_str(json).unwrap();
        assert_eq!(l.version, "0.1.1");
        assert_eq!(l.zip, "https://x/Yard-0.1.1-arm64.zip");
        assert_eq!((l.sha256.as_str(), l.notes_url.as_str()), ("ab", "https://x/n"));
        let again: Latest = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(again, l);
    }

    #[test]
    fn the_uninstall_plan_follows_the_facts() {
        use UninstallStep::*;
        let full = UninstallFacts {
            kind: InstallKind::Brew { app: "/Applications/Yard.app".into() },
            agent_installed: true,
            ca_exists: true,
            helper_installed: true,
            keep_data: false,
            data_exists: true,
        };
        assert_eq!(
            uninstall_plan(&full),
            [StopDaemon, Untrust, HelperUninstall, RemoveData, RemovePrefs, RemoveBrewCask]
        );
        let curl =
            UninstallFacts { kind: InstallKind::Bundle { app: "/Applications/Yard.app".into() }, ..full };
        assert_eq!(uninstall_plan(&curl).last(), Some(&RemoveBundle));
        let bare = UninstallFacts {
            kind: InstallKind::Dev,
            agent_installed: false,
            ca_exists: false,
            helper_installed: false,
            keep_data: true,
            data_exists: true,
        };
        assert_eq!(uninstall_plan(&bare), [RemovePrefs]);
        let no_data = UninstallFacts { data_exists: false, keep_data: false, ..bare };
        assert_eq!(uninstall_plan(&no_data), [RemovePrefs], "a data root that is not there is skipped");
    }

    #[test]
    fn the_release_url_takes_the_override() {
        assert_eq!(release_url_from(None), RELEASE_URL);
        assert_eq!(release_url_from(Some("file:///tmp/r/".into())), "file:///tmp/r");
        assert_eq!(release_url_from(Some(String::new())), RELEASE_URL);
    }
}
