//! Brew sources and the trust gate (spec M4b §8). Brew is the only record of what is
//! trusted: Yard reads it with `brew trust --json=v1`, changes it only by running `brew
//! trust` / `brew untrust`, and never reads or writes brew's trust file or sets a
//! `HOMEBREW_NO_*` variable.

use crate::jobs::JobCtx;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use yard_core::brew::Brew;
use yard_core::config::yard::{ExtSource, YardConfig};
use yard_core::shim::child_path;
use yard_core::sources::{self, TRUST_SINCE, TrustStore, YARD_TAPS};
use yard_proto::{ErrorCode, SourceInfo, YardError};

/// `shivammathur/extensions/<module>@<v>`: the formula `ext add` installs from brew.
pub fn ext_formula(module: &str, v: yard_core::PhpVersion) -> String {
    format!("shivammathur/extensions/{module}@{v}")
}

/// Runs `brew <args>` outside a job, for the read-only questions the gate asks. Stdout,
/// and whether it exited 0 (stderr is kept for the error). Brew's environment is yardd's
/// own: nothing is added.
async fn brew_stdout(brew: &Brew, args: &[&str]) -> Result<(bool, String, String), YardError> {
    let program = brew.brew_bin();
    let shown = format!("brew {}", args.join(" "));
    let mut cmd = Command::new(&program);
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", child_path(None, &inherited))
        .kill_on_drop(true);
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let out = cmd
        .output()
        .await
        .map_err(|e| YardError::new(ErrorCode::JobFailed, format!("cannot run {shown}: {e}")))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// What brew trusts, or `None` when brew predates tap trust (before 6.0), which has
/// nothing to read and nothing to gate.
pub async fn read_trust(brew: &Brew) -> Result<Option<TrustStore>, YardError> {
    let (_, version, _) = brew_stdout(brew, &["--version"]).await?;
    if sources::brew_major(&version).is_some_and(|major| major < TRUST_SINCE) {
        return Ok(None);
    }
    let (ok, out, err) = brew_stdout(brew, &["trust", "--json=v1"]).await?;
    if !ok {
        let tail = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
        return Err(YardError::new(ErrorCode::JobFailed, format!("brew trust --json=v1 failed: {tail}")));
    }
    sources::parse_trust(&out).map(Some).map_err(YardError::internal)
}

/// `FORMULA_UNTRUSTED` unless brew would load `formula`: homebrew/core, or it or its tap
/// trusted. Runs before the install job starts, so the client gets an ordinary error it
/// can answer with `yard source trust`, and brew is never asked to install.
pub async fn require_trusted(brew: &Brew, formula: &str) -> Result<(), YardError> {
    let Some(store) = read_trust(brew).await? else { return Ok(()) };
    if store.trusts(formula) {
        return Ok(());
    }
    let tap = sources::tap_of(formula).unwrap_or_default();
    Err(YardError::new(
        ErrorCode::FormulaUntrusted,
        format!("{formula} comes from the {tap} tap, which brew does not trust yet"),
    )
    .with_hint(format!("yard source trust {formula}"))
    .with_data(serde_json::json!({ "formula": formula, "tap": tap })))
}

/// The formulae from Yard's taps that Yard knows are installed: the PHP versions' kegs,
/// the extensions `yard.toml` records from brew, and MinIO's keg.
fn installed(brew: &Brew, yard: &YardConfig) -> BTreeSet<String> {
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let mut found: BTreeSet<String> =
        brew.installed_php(&exists, &read_link).into_iter().map(yard_core::php::formula).collect();
    for (v, exts) in &yard.extensions {
        found.extend(
            exts.iter().filter(|(_, e)| e.source == ExtSource::Brew).map(|(m, _)| ext_formula(m, *v)),
        );
    }
    if brew.prefix.join("opt/minio").exists() {
        found.insert("minio/stable/minio".to_string());
    }
    found
}

/// The rows of `sources.list`.
pub async fn list(brew: &Brew, yard: &YardConfig) -> Result<Vec<SourceInfo>, YardError> {
    let trust = read_trust(brew).await?;
    let tapped = |tap: &str| {
        let dir = tap.split_once('/').map(|(owner, repo)| format!("{owner}/homebrew-{repo}"));
        dir.is_some_and(|d| brew.prefix.join("Library/Taps").join(d).is_dir())
    };
    Ok(sources::rows(trust.as_ref(), &tapped, &installed(brew, yard)))
}

/// A target `brew trust` takes as a name: not empty, not a flag, no spaces.
pub fn check_target(target: &str) -> Result<(), YardError> {
    if target.is_empty() || target.starts_with('-') || target.chars().any(char::is_whitespace) {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{target:?} is not a formula or tap name"),
        )
        .with_hint(format!("a formula is tap/repo/name, a tap is {}", YARD_TAPS[0])));
    }
    Ok(())
}

/// `brew trust --formula <target>`, or `--tap` with `tap`, as a job: brew's output
/// streams, and a refusal arrives as `JOB_FAILED` with brew's own message.
pub async fn trust(job: &JobCtx, brew: &Brew, target: &str, tap: bool) -> Result<(), YardError> {
    change(job, brew, "trust", target, tap).await
}

/// `brew untrust`, as `trust` runs `brew trust`.
pub async fn untrust(job: &JobCtx, brew: &Brew, target: &str, tap: bool) -> Result<(), YardError> {
    change(job, brew, "untrust", target, tap).await
}

async fn change(job: &JobCtx, brew: &Brew, verb: &str, target: &str, tap: bool) -> Result<(), YardError> {
    let kind = if tap { "--tap" } else { "--formula" };
    job.run(&brew.brew_bin(), &[verb, kind, target], &[]).await
}
