//! `yard self-update`: replaces the whole Yard.app from the latest release (M7 spec §6.1).

use super::{Ctx, Outcome, confirm, interactive};
use crate::commands::lifecycle::{agent_plist, launchctl, uid};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use yard_core::brew::Brew;
use yard_core::install::{
    InstallKind, Latest, Update, Version, install_kind, release_url, sha256_hex, update,
};
use yard_core::paths::{DAEMON_LABEL, HELPER_SOCKET};
use yard_core::render::launchd::{AgentFacts, read_agent};
use yard_daemon::helper_client::HelperClient;
use yard_daemon::holder::{parent_of, pid_path};
use yard_proto::{ErrorCode, VersionInfo, YardError};

/// What `--check` reports, for people and for Yard.app (`--json`).
#[derive(Debug, Serialize)]
pub struct UpdateCheck {
    pub installed: String,
    pub latest: String,
    pub available: bool,
    pub kind: &'static str,
    pub notes_url: String,
}

/// What an update did.
#[derive(Debug, Serialize)]
pub struct Updated {
    pub from: String,
    pub to: String,
    pub restarted_yardd: bool,
    /// The yardd the agent runs instead of this app's (a dev build), left alone.
    pub yardd_left: Option<PathBuf>,
    pub helper_changed: bool,
    /// Yard.app was open on the old bundle, so it was quit and opened again on the new one.
    pub relaunched_app: bool,
    /// What went wrong after the new app was in place; the update itself stands.
    pub warnings: Vec<String>,
}

pub async fn self_update(ctx: &Ctx, check: bool, yes: bool) -> Outcome {
    let exe = std::env::current_exe().map_err(|e| YardError::internal(format!("cannot locate yard: {e}")))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let caskroom =
        Brew::from_env(&|p: &Path| p.exists()).is_ok_and(|b| b.prefix.join("Caskroom/yard").exists());
    let kind = install_kind(&exe, caskroom);
    let installed: Version = env!("CARGO_PKG_VERSION").parse()?;
    if !check {
        refuse_unless_bundle(&kind)?;
    }
    let latest = fetch_latest()?;
    let newest: Version = latest.version.parse()?;
    let available = update(installed, newest);

    if check {
        let report = UpdateCheck {
            installed: installed.to_string(),
            latest: newest.to_string(),
            available: matches!(available, Update::Available(_)),
            kind: kind_name(&kind),
            notes_url: latest.notes_url.clone(),
        };
        ctx.emit(&report, || match available {
            Update::Available(v) => {
                format!("Yard {v} is available (this is {installed}): {}\n", report.notes_url)
            }
            Update::UpToDate => format!("Yard {installed} is up to date.\n"),
        });
        return Ok(0);
    }

    let app = refuse_unless_bundle(&kind)?;
    if available == Update::UpToDate {
        ctx.emit(&serde_json::json!({ "installed": installed.to_string(), "available": false }), || {
            format!("Yard {installed} is up to date.\n")
        });
        return Ok(0);
    }
    if !yes {
        if !interactive(ctx) {
            return Err(YardError::new(ErrorCode::NeedsConfirmation, "updating Yard needs confirmation")
                .with_hint("yard self-update --yes")
                .into());
        }
        if !confirm(&format!("Update Yard {installed} to {newest}? [y/N] ")) {
            return Ok(1);
        }
    }

    let parent = app.parent().ok_or_else(|| YardError::internal("the app has no parent directory"))?;
    // An update that was killed left its work directory behind.
    for stale in stale_work(parent, &alive) {
        let _ = std::fs::remove_dir_all(stale);
    }
    let work = parent.join(format!(".yard-update-{}", std::process::id()));
    let _cleanup = RemoveOnDrop(work.clone());
    std::fs::create_dir_all(&work).map_err(|e| io(&work, e))?;
    let zip = work.join("Yard.zip");
    curl(&latest.zip, Some(&zip))?;
    let bytes = std::fs::read(&zip).map_err(|e| io(&zip, e))?;
    if sha256_hex(&bytes) != latest.sha256.to_lowercase() {
        return Err(YardError::internal(format!(
            "the download of Yard {newest} does not match its checksum"
        ))
        .with_hint("yard self-update")
        .into());
    }
    run("ditto", &["-x", "-k", &zip.display().to_string(), &work.display().to_string()])?;
    let staged = work.join("Yard.app");
    verify(&app, &staged, newest, installed)?;

    // Read before the swap: once the old bundle is gone its processes have no path to match.
    let open_apps = apps_to_relaunch(&yard_processes(), &app.join(APP_EXECUTABLE), &ancestors());

    // From here on the new app is in place: nothing below fails the update.
    let mut warnings: Vec<String> = swap(&app, &staged)?.into_iter().collect();
    let relaunched_app = match relaunch(&app, &open_apps) {
        Ok(relaunched) => relaunched,
        Err(e) => {
            warnings.push(format!("Yard.app still runs the old version: {e}; quit and open it again"));
            false
        }
    };
    let (yardd, warning) = restart_yardd_in(&app);
    warnings.extend(warning);
    let helper_changed = helper_changed(&app).await;
    let done = Updated {
        from: installed.to_string(),
        to: newest.to_string(),
        restarted_yardd: yardd == YarddAfterUpdate::Restart && done_restart(&warnings),
        yardd_left: match &yardd {
            YarddAfterUpdate::LeftAlone(p) => Some(p.clone()),
            _ => None,
        },
        helper_changed,
        relaunched_app,
        warnings,
    };
    ctx.emit(&done, || {
        let mut out = format!("Updated Yard {installed} to {newest}.\n");
        match &yardd {
            YarddAfterUpdate::Restart if done.restarted_yardd => {
                out.push_str("yardd restarted on the new build.\n")
            }
            YarddAfterUpdate::LeftAlone(other) => {
                out.push_str(&format!("yardd runs {}, so it was left alone.\n", other.display()))
            }
            YarddAfterUpdate::NotRunning => {
                out.push_str("yardd is not running; yard start runs the new build.\n")
            }
            _ => {}
        }
        if done.relaunched_app {
            out.push_str("Yard.app reopened on the new version.\n");
        }
        for w in &done.warnings {
            out.push_str(&format!("warning: {w}\n"));
        }
        if helper_changed {
            out.push_str("The new helper needs installing: sudo yard helper install\n");
        }
        out
    });
    Ok(0)
}

/// Only an `install.sh` bundle updates itself (spec §6.2).
fn refuse_unless_bundle(kind: &InstallKind) -> Result<PathBuf, YardError> {
    match kind {
        InstallKind::Dev => {
            Err(YardError::new(ErrorCode::InvalidArgument, "this yard is a development build")
                .with_hint("cargo build --release"))
        }
        InstallKind::Brew { .. } => {
            Err(YardError::new(ErrorCode::InvalidArgument, "Yard was installed with Homebrew")
                .with_hint("brew upgrade --cask yard"))
        }
        InstallKind::Bundle { app } => Ok(app.clone()),
    }
}

fn kind_name(kind: &InstallKind) -> &'static str {
    match kind {
        InstallKind::Dev => "dev",
        InstallKind::Bundle { .. } => "bundle",
        InstallKind::Brew { .. } => "brew",
    }
}

fn fetch_latest() -> Result<Latest, YardError> {
    let url = format!("{}/latest.json", release_url());
    let body = curl(&url, None)?;
    serde_json::from_slice(&body).map_err(|e| {
        YardError::internal(format!("{url} is not a release feed: {e}")).with_hint("yard self-update --check")
    })
}

/// `curl -fsSL`, to `out` or returning the body.
fn curl(url: &str, out: Option<&Path>) -> Result<Vec<u8>, YardError> {
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.args(["-fsSL", "--proto", "=https,file", url]);
    if let Some(out) = out {
        cmd.arg("-o").arg(out);
    }
    let o = cmd.output().map_err(|e| YardError::internal(format!("cannot run curl: {e}")))?;
    if !o.status.success() {
        return Err(YardError::internal(format!(
            "cannot download {url}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))
        .with_hint("check the network, then yard self-update"));
    }
    Ok(o.stdout)
}

fn run(program: &str, args: &[&str]) -> Result<(), YardError> {
    let o = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| YardError::internal(format!("cannot run {program}: {e}")))?;
    if o.status.success() {
        return Ok(());
    }
    Err(YardError::internal(format!(
        "{program} {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&o.stderr).trim()
    )))
}

/// The new app must satisfy this app's own designated requirement (the same bundle id, a
/// Developer ID, the same team), which `codesign` checks itself rather than Yard reading its
/// human output; Gatekeeper must accept it (notarized); and it must be the release the feed
/// named, newer than this one.
fn verify(app: &Path, staged: &Path, newest: Version, installed: Version) -> Result<(), YardError> {
    let refuse = |why: String| {
        YardError::internal(format!("the downloaded Yard {newest} {why}; not installing it"))
            .with_hint("yard self-update --check")
    };
    let requirement = designated_requirement(app)?;
    let staged_arg = staged.display().to_string();
    run("codesign", &["--verify", "--deep", "--strict", &format!("-R={requirement}"), &staged_arg])
        .map_err(|e| refuse(format!("is not signed like this Yard ({})", e.message)))?;
    run("spctl", &["--assess", "--type", "execute", &staged_arg])
        .map_err(|e| refuse(format!("is not accepted by Gatekeeper ({})", e.message)))?;
    let plist = staged.join("Contents/Info.plist");
    let o = Command::new("plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .map_err(|e| YardError::internal(format!("cannot run plutil: {e}")))?;
    let version: Version = String::from_utf8_lossy(&o.stdout).trim().parse()?;
    if !staged_version_ok(version, newest, installed) {
        return Err(refuse(format!("says it is {version}")));
    }
    Ok(())
}

fn staged_version_ok(staged: Version, newest: Version, installed: Version) -> bool {
    staged == newest && staged > installed
}

/// This app's designated requirement, from `codesign -d -r-`.
fn designated_requirement(app: &Path) -> Result<String, YardError> {
    let o = Command::new("codesign")
        .args(["-d", "-r-"])
        .arg(app)
        .output()
        .map_err(|e| YardError::internal(format!("cannot run codesign: {e}")))?;
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    designated_requirement_in(&text)
        .map_err(|e| YardError::internal(format!("{}: {}", app.display(), e.message)))
}

/// The explicit `designated => …` line (an ad hoc signature's implicit one starts with `#`),
/// accepted only when it pins a Developer ID team and Yard.app's bundle id.
fn designated_requirement_in(codesign: &str) -> Result<String, YardError> {
    let req =
        codesign.lines().find_map(|l| l.strip_prefix("designated => ")).map(str::trim).ok_or_else(|| {
            YardError::internal("it has no explicit designated requirement (not Developer ID signed)")
        })?;
    let pins = req.contains("anchor apple generic")
        && req.contains("certificate leaf[subject.OU] = ")
        && req.contains(&format!("identifier \"{BUNDLE_ID}\""));
    if !pins {
        return Err(YardError::internal(format!(
            "its requirement does not pin a Developer ID team and {BUNDLE_ID}"
        )));
    }
    Ok(req.to_string())
}

/// Yard.app's bundle id (M6 spec §14.1).
const BUNDLE_ID: &str = "dev.more.yard.app";

/// Whether the restart went through: its failure is the one warning that starts "yardd could not".
fn done_restart(warnings: &[String]) -> bool {
    !warnings.iter().any(|w| w.starts_with("yardd could not"))
}

/// Replaces `app` with `staged`, which sits beside it (one volume, so both moves are renames).
/// If the new one cannot move into place, the old one is put back; a warning when the old one
/// cannot be removed afterwards.
fn swap(app: &Path, staged: &Path) -> Result<Option<String>, YardError> {
    let name = app.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let old = app.with_file_name(format!("{name}.old-{}", std::process::id()));
    std::fs::rename(app, &old).map_err(|e| io(app, e))?;
    if let Err(e) = std::fs::rename(staged, app) {
        let _ = std::fs::rename(&old, app);
        return Err(io(staged, e));
    }
    Ok(std::fs::remove_dir_all(&old)
        .err()
        .map(|e| format!("the old app is left at {} ({e}): delete it", old.display())))
}

/// What happens to yardd after an update.
#[derive(Debug, Clone, PartialEq, Eq)]
enum YarddAfterUpdate {
    /// The loaded agent runs this app's yardd: restart it on the new build.
    Restart,
    /// The agent runs another yardd (a dev build), left alone.
    LeftAlone(PathBuf),
    /// The agent runs this app's yardd but is not loaded (`yard stop`): nothing to restart.
    NotRunning,
    /// No agent is installed.
    NoAgent,
}

fn restart_decision(app: &Path, agent: Option<AgentFacts>, loaded: bool) -> YarddAfterUpdate {
    match agent {
        None => YarddAfterUpdate::NoAgent,
        Some(a) if !a.program.starts_with(app) => YarddAfterUpdate::LeftAlone(a.program),
        Some(_) if loaded => YarddAfterUpdate::Restart,
        Some(_) => YarddAfterUpdate::NotRunning,
    }
}

/// Restarts yardd when its loaded agent runs this app. The update itself is done by then, so a
/// failure here is a warning, never the command's failure.
fn restart_yardd_in(app: &Path) -> (YarddAfterUpdate, Option<String>) {
    let agent = agent_plist().ok().and_then(|f| std::fs::read(f).ok()).and_then(|b| read_agent(&b).ok());
    let service = format!("gui/{}/{DAEMON_LABEL}", uid());
    let loaded =
        Command::new("launchctl").args(["print", &service]).output().is_ok_and(|o| o.status.success());
    let decision = restart_decision(app, agent, loaded);
    let warning = (decision == YarddAfterUpdate::Restart)
        .then(|| launchctl(&["kickstart", "-k", &service]).err())
        .flatten()
        .map(|e| format!("yardd could not be restarted ({}): yard stop && yard start", e.message));
    (decision, warning)
}

/// Whether the new bundle's yard needs another helper revision than the one installed. No
/// helper installed is no change to report: `yard doctor` already says to install one.
async fn helper_changed(app: &Path) -> bool {
    let Ok(status) = HelperClient::new(HELPER_SOCKET).status().await else { return false };
    let Ok(o) = Command::new(app.join("Contents/Helpers/yard")).args(["version", "--json"]).output() else {
        return false;
    };
    let needs = serde_json::from_slice::<VersionInfo>(&o.stdout).ok().and_then(|v| v.needs_helper_revision);
    needs.is_some_and(|r| r != status.revision)
}

/// Yard.app's executable inside the bundle.
const APP_EXECUTABLE: &str = "Contents/MacOS/Yard";

/// `.yard-update-<pid>` directories in `dir` whose process is gone.
fn stale_work(dir: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut stale: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name();
            let pid: u32 = name.to_str()?.strip_prefix(".yard-update-")?.parse().ok()?;
            (!alive(pid)).then(|| e.path())
        })
        .collect();
    stale.sort();
    stale
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists.
    i32::try_from(pid).is_ok_and(|p| unsafe { libc::kill(p, 0) } == 0)
}

/// The pids of `running` (pid, executable path) that run `exe`, leaving out this process's
/// ancestors: Yard.app's own update item runs `yard self-update` and reopens itself.
fn apps_to_relaunch(running: &[(u32, Option<String>)], exe: &Path, ancestors: &BTreeSet<u32>) -> Vec<u32> {
    running
        .iter()
        .filter(|(pid, path)| {
            path.as_deref().is_some_and(|p| Path::new(p) == exe) && !ancestors.contains(pid)
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// Every process named Yard, with its executable path.
fn yard_processes() -> Vec<(u32, Option<String>)> {
    let Ok(out) = Command::new("/usr/bin/pgrep").args(["-x", "Yard"]).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .map(|pid| (pid, pid_path(pid)))
        .collect()
}

/// This process's parent, its parent, and so on up to launchd.
fn ancestors() -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    let mut pid = std::process::id();
    while let Some(parent) = parent_of(pid).filter(|p| *p > 1 && found.len() < 64) {
        found.insert(parent);
        pid = parent;
    }
    found
}

/// Quits the apps in `pids` (SIGTERM, then SIGKILL after 5 s) and opens `app` once, so Yard.app
/// runs the bundle now on disk rather than the one just deleted. `Ok(false)` when none was open.
fn relaunch(app: &Path, pids: &[u32]) -> Result<bool, YardError> {
    if pids.is_empty() {
        return Ok(false);
    }
    for &pid in pids {
        let Ok(raw) = i32::try_from(pid) else { continue };
        // SAFETY: kill(2) on a Yard.app process found running this bundle.
        unsafe { libc::kill(raw, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if alive(pid) {
            // SAFETY: as above.
            unsafe { libc::kill(raw, libc::SIGKILL) };
        }
    }
    run("/usr/bin/open", &[&app.display().to_string()])?;
    Ok(true)
}

fn io(path: &Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("{}: {e}", path.display()))
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(dir: &Path, name: &str, marker: &str) -> PathBuf {
        let app = dir.join(name);
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("marker"), marker).unwrap();
        app
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".old-"))
            .collect()
    }

    #[test]
    fn a_killed_update_s_work_directory_is_stale_once_its_process_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        for name in [".yard-update-100", ".yard-update-200", ".yard-update-x", "Yard.app"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }

        let stale = stale_work(dir.path(), &|pid| pid == 200);

        assert_eq!(stale, vec![dir.path().join(".yard-update-100")], "200 still runs; the rest are not ours");
    }

    #[test]
    fn the_app_is_relaunched_only_where_it_runs_this_bundle_and_did_not_start_the_update() {
        let exe = Path::new("/Applications/Yard.app/Contents/MacOS/Yard");
        let running = [
            (10, Some("/Applications/Yard.app/Contents/MacOS/Yard".to_string())),
            (11, Some("/Users/me/Applications/Yard.app/Contents/MacOS/Yard".to_string())),
            (12, Some("/Applications/Yard.app/Contents/MacOS/Yard".to_string())),
            (13, None),
        ];
        let ancestors = BTreeSet::from([12]);

        assert_eq!(apps_to_relaunch(&running, exe, &ancestors), vec![10]);
    }

    #[test]
    fn swap_replaces_the_app_and_removes_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_with(dir.path(), "Yard.app", "old");
        let staged = app_with(dir.path(), ".Yard.app.new-1", "new");
        assert_eq!(swap(&app, &staged).unwrap(), None);
        assert_eq!(std::fs::read_to_string(app.join("marker")).unwrap(), "new");
        assert!(!staged.exists());
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn swap_puts_the_old_app_back_when_the_new_one_cannot_move() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_with(dir.path(), "Yard.app", "old");
        let missing = dir.path().join(".Yard.app.new-2");
        assert!(swap(&app, &missing).is_err());
        assert_eq!(std::fs::read_to_string(app.join("marker")).unwrap(), "old");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn the_designated_requirement_must_pin_a_developer_id_and_the_app() {
        let good = "Executable=/Applications/Yard.app/Contents/MacOS/Yard\ndesignated => anchor apple generic and identifier \"dev.more.yard.app\" and (certificate leaf[field.1.2.840.113635.100.6.1.9] /* exists */ or certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = ABCDE12345)\n";
        let req = designated_requirement_in(good).unwrap();
        assert!(req.starts_with("anchor apple generic and identifier"), "{req}");
        assert!(req.ends_with("= ABCDE12345)"), "{req}");
        let adhoc = "# designated => cdhash H\"b3424d9e5b2f1e2873fe305782768722501c6b94\"\n";
        assert!(designated_requirement_in(adhoc).is_err());
        let other_app = good.replace("dev.more.yard.app", "com.example.other");
        assert!(designated_requirement_in(&other_app).is_err());
        let no_team = "designated => identifier \"dev.more.yard.app\" and anchor apple\n";
        assert!(designated_requirement_in(no_team).is_err());
    }

    #[test]
    fn yardd_is_restarted_only_when_its_loaded_agent_runs_this_app() {
        let app = Path::new("/Applications/Yard.app");
        let ours = || AgentFacts { program: app.join("Contents/Helpers/yardd"), root: None };
        assert_eq!(restart_decision(app, Some(ours()), true), YarddAfterUpdate::Restart);
        assert_eq!(restart_decision(app, Some(ours()), false), YarddAfterUpdate::NotRunning);
        let dev = AgentFacts { program: "/repo/target/release/yardd".into(), root: None };
        assert_eq!(
            restart_decision(app, Some(dev), true),
            YarddAfterUpdate::LeftAlone("/repo/target/release/yardd".into())
        );
        assert_eq!(restart_decision(app, None, false), YarddAfterUpdate::NoAgent);
    }

    #[test]
    fn the_staged_app_must_be_the_newer_release_the_feed_names() {
        let v = |s: &str| s.parse::<Version>().unwrap();
        assert!(staged_version_ok(v("0.2.0"), v("0.2.0"), v("0.1.0")));
        assert!(!staged_version_ok(v("0.1.5"), v("0.2.0"), v("0.1.0")), "not what the feed named");
        assert!(!staged_version_ok(v("0.1.0"), v("0.1.0"), v("0.1.0")), "not newer");
        assert!(!staged_version_ok(v("0.0.9"), v("0.0.9"), v("0.1.0")), "a downgrade");
    }
}
