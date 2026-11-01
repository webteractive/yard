//! Asking the emailulator binary about itself and its LaunchAgent (emailulator spec §5.2).
//! Yard never starts, stops or installs it here: `--version` and `service status --json` only
//! read.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use yard_core::emailulator::{AgentStatus, MIN_VERSION, parse_status, parse_version};

/// How long one call may take before Yard stops waiting and keeps what it knew.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// What Yard knows about emailulator after asking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub bin: PathBuf,
    /// `None` when `--version` failed or printed something else.
    pub version: Option<(u32, u32, u32)>,
    /// The agent, or the last one known when `status_error` is set.
    pub agent: Option<AgentStatus>,
    /// Why the last `service status --json` could not be read: it timed out, did not parse
    /// or exited with a code emailulator does not use. Doctor warns with it.
    pub status_error: Option<String>,
}

/// Asks `bin` for its version and, from 0.1.3, its agent. A status that cannot be read keeps
/// `previous`'s agent, so a slow or broken answer never moves mail elsewhere.
pub async fn probe(bin: &Path, timeout: Duration, previous: Option<&Probe>) -> Probe {
    let version = match run(bin, &["--version"], timeout).await {
        Ok((0, stdout)) => parse_version(&stdout),
        _ => None,
    };
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
