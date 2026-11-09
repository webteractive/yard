//! `yard self-update`: replaces the whole Yard.app from the latest release (M7 spec §6.1).

use super::{Ctx, Outcome, confirm, interactive};
use crate::commands::lifecycle::{agent_plist, launchctl, uid};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use yard_core::brew::Brew;
use yard_core::install::{
    InstallKind, Latest, Update, Version, install_kind, release_url, sha256_hex, update,
};
use yard_core::paths::{DAEMON_LABEL, HELPER_SOCKET};
use yard_core::render::launchd::read_agent;
use yard_daemon::helper_client::HelperClient;
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
    pub helper_changed: bool,
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

    swap(&app, &staged)?;

    let restarted = restart_yardd_in(&app)?;
    let helper_changed = helper_changed(&app).await;
    let done = Updated {
        from: installed.to_string(),
        to: newest.to_string(),
        restarted_yardd: restarted.is_none(),
        helper_changed,
    };
    ctx.emit(&done, || {
        let mut out = format!("Updated Yard {installed} to {newest}.\n");
        match &restarted {
            None => out.push_str("yardd restarted on the new build.\n"),
            Some(Some(other)) => {
                out.push_str(&format!("yardd runs {}, so it was left alone.\n", other.display()))
            }
            Some(None) => {}
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

/// Replaces `app` with `staged`, which sits beside it (one volume, so both moves are renames).
/// If the new one cannot move into place, the old one is put back.
fn swap(app: &Path, staged: &Path) -> Result<(), YardError> {
    let name = app.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let old = app.with_file_name(format!("{name}.old-{}", std::process::id()));
    std::fs::rename(app, &old).map_err(|e| io(app, e))?;
    if let Err(e) = std::fs::rename(staged, app) {
        let _ = std::fs::rename(&old, app);
        return Err(io(staged, e));
    }
    std::fs::remove_dir_all(&old).map_err(|e| io(&old, e))?;
    Ok(())
}

/// Restarts the LaunchAgent's yardd when it runs from `app`. `None` when restarted;
/// `Some(Some(path))` when the agent runs another yardd (a dev build), left alone; `Some(None)`
/// when no agent is installed.
fn restart_yardd_in(app: &Path) -> Result<Option<Option<PathBuf>>, YardError> {
    let Ok(bytes) = std::fs::read(agent_plist()?) else { return Ok(Some(None)) };
    let agent = read_agent(&bytes)?;
    if !agent.program.starts_with(app) {
        return Ok(Some(Some(agent.program)));
    }
    launchctl(&["kickstart", "-k", &format!("gui/{}/{DAEMON_LABEL}", uid())])?;
    Ok(None)
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
    fn swap_replaces_the_app_and_removes_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_with(dir.path(), "Yard.app", "old");
        let staged = app_with(dir.path(), ".Yard.app.new-1", "new");
        swap(&app, &staged).unwrap();
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
    fn the_staged_app_must_be_the_newer_release_the_feed_names() {
        let v = |s: &str| s.parse::<Version>().unwrap();
        assert!(staged_version_ok(v("0.2.0"), v("0.2.0"), v("0.1.0")));
        assert!(!staged_version_ok(v("0.1.5"), v("0.2.0"), v("0.1.0")), "not what the feed named");
        assert!(!staged_version_ok(v("0.1.0"), v("0.1.0"), v("0.1.0")), "not newer");
        assert!(!staged_version_ok(v("0.0.9"), v("0.0.9"), v("0.1.0")), "a downgrade");
    }
}
