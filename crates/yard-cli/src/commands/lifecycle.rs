//! `yard start` and `yard stop`: yardd's LaunchAgent in the user's launchd domain.

use super::{Ctx, Outcome};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use yard_core::paths::DAEMON_LABEL;
use yard_core::render::launchd::daemon_plist;
use yard_proto::{ErrorCode, YardError};

const START_WAIT: Duration = Duration::from_secs(5);

pub async fn start(ctx: &Ctx) -> Outcome {
    let yardd = sibling("yardd")?;
    let log = ctx.paths.log("yardd");
    std::fs::create_dir_all(ctx.paths.logs_dir()).map_err(|e| io(&ctx.paths.logs_dir(), e))?;
    let root = ctx.root_overridden.then(|| ctx.paths.root());
    let plist = daemon_plist(&yardd, root, &log);
    let file = agent_plist()?;
    let changed = std::fs::read(&file).ok().as_deref() != Some(plist.as_slice());
    if changed {
        std::fs::create_dir_all(file.parent().unwrap_or(&file)).map_err(|e| io(&file, e))?;
        std::fs::write(&file, &plist).map_err(|e| io(&file, e))?;
    }

    let domain = format!("gui/{}", uid());
    let service = format!("{domain}/{DAEMON_LABEL}");
    let loaded =
        Command::new("launchctl").args(["print", &service]).output().is_ok_and(|o| o.status.success());
    // A loaded agent keeps the plist it was loaded with, so a changed one is reloaded.
    if loaded && changed {
        launchctl(&["bootout", &service])?;
        unloaded(&service).await;
    }
    if loaded && !changed {
        launchctl(&["kickstart", &service])?;
    } else {
        launchctl(&["bootstrap", &domain, &file.display().to_string()])?;
    }

    let deadline = Instant::now() + START_WAIT;
    let client = loop {
        match ctx.client().await {
            Ok(client) => break client,
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(_) => {
                return Err(YardError::internal("yardd did not answer within 5 s of starting")
                    .with_hint(format!("tail -50 {:?}", log.display().to_string()))
                    .into());
            }
        }
    };
    ctx.emit(&client.hello, || {
        format!("yardd {} is running. yard sites lists what it serves.\n", client.hello.daemon_version)
    });
    Ok(0)
}

pub async fn stop(ctx: &Ctx) -> Outcome {
    let service = format!("gui/{}/{DAEMON_LABEL}", uid());
    let out = Command::new("launchctl")
        .args(["bootout", &service])
        .output()
        .map_err(|e| YardError::internal(format!("cannot run launchctl: {e}")))?;
    let was_running = out.status.success();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // 3 (ESRCH) and 113 mean the agent was not loaded.
    if !was_running && !matches!(out.status.code(), Some(3 | 113)) {
        return Err(YardError::internal(format!("launchctl bootout {service} failed: {}", stderr.trim()))
            .with_hint(format!("launchctl print {service}"))
            .into());
    }
    if was_running {
        // Otherwise a `yard start` straight after finds the agent still loaded and only kicks it.
        unloaded(&service).await;
    }
    ctx.emit(&serde_json::json!({ "stopped": was_running }), || {
        if was_running { "yardd stopped.\n".into() } else { "yardd was not running.\n".into() }
    });
    Ok(0)
}

/// `bootout` returns before launchd lets go of the label; a `bootstrap` in that window fails.
pub async fn unloaded(service: &str) {
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline
        && Command::new("launchctl").args(["print", service]).output().is_ok_and(|o| o.status.success())
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A binary installed next to this `yard`, as `cargo build` and the release both lay out.
/// A `yard` symlinked onto PATH looks beside the binary it points at.
pub fn sibling(name: &str) -> Result<PathBuf, YardError> {
    let exe = std::env::current_exe().map_err(|e| YardError::internal(format!("cannot locate yard: {e}")))?;
    sibling_of(&exe, name)
}

fn sibling_of(exe: &Path, name: &str) -> Result<PathBuf, YardError> {
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let path = exe.with_file_name(name);
    if path.exists() {
        Ok(path)
    } else {
        Err(YardError::new(ErrorCode::InvalidArgument, format!("{} is missing", path.display()))
            .with_hint("cargo build --release, then run target/release/yard"))
    }
}

fn agent_plist() -> Result<PathBuf, YardError> {
    let home = std::env::var_os("HOME").ok_or_else(|| YardError::internal("HOME is not set"))?;
    Ok(PathBuf::from(home).join(format!("Library/LaunchAgents/{DAEMON_LABEL}.plist")))
}

fn uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

pub fn launchctl(args: &[&str]) -> Result<(), YardError> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| YardError::internal(format!("cannot run launchctl: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    Err(YardError::internal(format!(
        "launchctl {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

fn io(path: &std::path::Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::sibling_of;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_symlinked_yard_finds_its_siblings_beside_the_real_binary() {
        let dir = tempfile::tempdir().unwrap();
        let (real, bin) = (dir.path().join("release"), dir.path().join("bin"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(real.join("yard"), "").unwrap();
        std::fs::write(real.join("yardd"), "").unwrap();
        symlink(real.join("yard"), bin.join("yard")).unwrap();

        let found = sibling_of(&bin.join("yard"), "yardd").unwrap();
        assert_eq!(found, std::fs::canonicalize(real.join("yardd")).unwrap());
    }
}
