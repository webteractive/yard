//! Asking the emailulator binary about itself and its LaunchAgent (emailulator spec §5.2).
//! Yard never starts, stops or installs it here: `--version` and `service status --json` only
//! read.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use yard_core::emailulator::{AgentStatus, MIN_VERSION, Owner, owner, parse_status, parse_version};

/// How long one call may take before Yard stops waiting and keeps what it knew.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// What Yard knows about emailulator after asking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub bin: PathBuf,
    /// The version, or the last one known when `status_error` is set.
    pub version: Option<(u32, u32, u32)>,
    /// The agent, or the last one known when `status_error` is set.
    pub agent: Option<AgentStatus>,
    /// Why the binary could not be read this time: `--version` or `service status --json`
    /// timed out, printed something else or exited with a code emailulator does not use.
    /// Doctor warns with it; nobody takes it for an emailulator that went away.
    pub status_error: Option<String>,
}

/// Asks `bin` for its version and, from 0.1.3, its agent. An answer that cannot be read keeps
/// `previous`'s version and agent, so a slow or broken answer (a binary macOS is still scanning
/// after a self-update, say) never moves mail elsewhere or puts a site on the setup page.
pub async fn probe(bin: &Path, timeout: Duration, previous: Option<&Probe>) -> Probe {
    let unreadable = |reason: String| Probe {
        bin: bin.to_path_buf(),
        version: previous.and_then(|prev| prev.version),
        agent: previous.and_then(|prev| prev.agent.clone()),
        status_error: Some(reason),
    };
    let version = match run(bin, &["--version"], timeout).await {
        Ok((0, stdout)) => match parse_version(&stdout) {
            Some(version) => version,
            None => return unreadable(format!("emailulator --version printed {:?}", stdout.trim())),
        },
        Ok((code, _)) => return unreadable(format!("emailulator --version exited with {code}")),
        Err(reason) => return unreadable(reason),
    };
    let version = Some(version);
    let mut p = Probe { bin: bin.to_path_buf(), version, agent: None, status_error: None };
    if version.is_none_or(|v| v < MIN_VERSION) {
        // No `service status` before 0.1.3: it has no agent Yard can read.
        return p;
    }
    let answer = run(bin, &["service", "status", "--json"], timeout)
        .await
        .and_then(|(code, stdout)| parse_status(code, &stdout));
    match answer {
        Ok(agent) => p.agent = agent,
        Err(reason) => {
            p.agent = previous.and_then(|prev| prev.agent.clone());
            p.status_error = Some(reason);
        }
    }
    p
}

/// Runs `bin args` with no `EMAILULATOR_*` variable, so a shell setting cannot change what it
/// reports, and kills it after `timeout`.
async fn run(bin: &Path, args: &[&str], timeout: Duration) -> Result<(i32, String), String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("EMAILULATOR_") {
            cmd.env_remove(key);
        }
    }
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let what = format!("emailulator {}", args.join(" "));
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("{what} did not answer within {} s", timeout.as_secs()))?
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    let code = out.status.code().ok_or_else(|| format!("{what} was killed by a signal"))?;
    Ok((code, String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// An executable regular file, as the installer leaves emailulator.
pub fn is_exec(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Finds emailulator, the recorded path first, and asks it.
pub async fn find_and_probe(recorded: Option<&Path>, home: &Path, previous: Option<&Probe>) -> Option<Probe> {
    let bin = yard_core::emailulator::locate(recorded, home, &is_exec).ok()?;
    Some(probe(&bin, TIMEOUT, previous).await)
}

/// What `services.list`, `env.print` and doctor show of the probe: when it changes, the port
/// scan reconciles so they follow a crashed, restarted or moved agent within one scan.
pub fn shown(p: Option<&Probe>) -> impl PartialEq + use<'_> {
    let agent = p
        .and_then(|p| p.agent.as_ref())
        .map(|a| (a.installed, a.loaded, a.answering, a.pid, a.smtp, a.http, a.tls));
    (agent, p.map(|p| p.version), p.is_some_and(|p| p.status_error.is_some()))
}

/// The agent that serves mail, when one is installed and loaded.
pub fn serving_agent(p: Option<&Probe>) -> Option<&AgentStatus> {
    p.and_then(|p| p.agent.as_ref()).filter(|a| matches!(owner(Some(a), false), Owner::Agent(_)))
}
