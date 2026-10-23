//! Brew sources and the trust gate (spec M4b §8). Brew is the only record of what is
//! trusted: Yard reads it with `brew trust --json=v1`, changes it only by running `brew
//! trust` / `brew untrust`, and never reads or writes brew's trust file or sets a
//! `HOMEBREW_NO_*` variable.

use crate::jobs::JobCtx;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use yard_core::brew::Brew;
use yard_core::config::ServicesConfig;
use yard_core::config::yard::{ExtSource, YardConfig};
use yard_core::services::{Engine, brew_info};
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
pub(crate) async fn brew_stdout(brew: &Brew, args: &[&str]) -> Result<(bool, String, String), YardError> {
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

/// How long doctor and the MinIO keg check wait for brew to answer one question.
pub const BREW_TIMEOUT: Duration = Duration::from_secs(30);

/// What `brew --version` and `brew trust --json=v1` say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustRead {
    /// Brew's major version is below `TRUST_SINCE`: no `brew trust`, nothing to gate.
    Predates {
        major: u32,
    },
    Store(TrustStore),
}

/// Asks brew its version and, from 6.0, what it trusts.
pub async fn read_trust_state(brew: &Brew) -> Result<TrustRead, YardError> {
    let (_, version, _) = brew_stdout(brew, &["--version"]).await?;
    if let Some(major) = sources::brew_major(&version).filter(|major| *major < TRUST_SINCE) {
        return Ok(TrustRead::Predates { major });
    }
    let (ok, out, err) = brew_stdout(brew, &["trust", "--json=v1"]).await?;
    if !ok {
        let tail = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
        return Err(YardError::new(ErrorCode::JobFailed, format!("brew trust --json=v1 failed: {tail}")));
    }
    sources::parse_trust(&out).map(TrustRead::Store).map_err(YardError::internal)
}

/// What brew trusts, or `None` when brew predates tap trust (before 6.0), which has
/// nothing to read and nothing to gate.
pub async fn read_trust(brew: &Brew) -> Result<Option<TrustStore>, YardError> {
    Ok(match read_trust_state(brew).await? {
        TrustRead::Predates { .. } => None,
        TrustRead::Store(store) => Some(store),
    })
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

/// The most of `formula_renames.json` Yard reads: the real one is a few KiB.
const MAX_RENAMES: u64 = 1 << 20;

/// Whether the tap that `formula` names lists it, by what is in the tap's directory
/// under brew's prefix: a formula file (in `Formula/`, a sharded `Formula/<x>/`, or the
/// tap's root), an alias, or a rename in `formula_renames.json`, looked up by the
/// lower-cased name as brew does. It mirrors the flat or one-shard tap layout plus
/// `Aliases/` and `formula_renames.json`, and not `HomebrewFormula/` or
/// `tap_migrations.json`. brew's `search` finds real names only, so it cannot say that
/// `redis@8.4` is `phpredis@8.4`. A tap that is not there, or a file Yard cannot read
/// (or that is over `MAX_RENAMES`, or not UTF-8), counts as listing it: the gate holds.
/// Symlinked taps are followed. Blocking reads: run it with `spawn_blocking`.
fn tap_lists(prefix: &Path, formula: &str) -> bool {
    use std::io::{ErrorKind, Read};
    let Some(dir) = sources::tap_dir(formula) else { return true };
    let root = prefix.join("Library/Taps").join(dir);
    if !root.is_dir() {
        return true;
    }
    let name = sources::tap_name(formula);
    if sources::listing_paths(&name).iter().any(|path| root.join(path).exists()) {
        return true;
    }
    match std::fs::read_dir(root.join("Formula")) {
        Ok(shards) => {
            if shards.flatten().any(|shard| shard.path().join(format!("{name}.rb")).exists()) {
                return true;
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(_) => return true,
    }
    let file = match std::fs::File::open(root.join("formula_renames.json")) {
        Ok(file) => file,
        Err(e) => return e.kind() != ErrorKind::NotFound,
    };
    let mut text = String::new();
    match file.take(MAX_RENAMES + 1).read_to_string(&mut text) {
        Ok(read) if read as u64 <= MAX_RENAMES => sources::renames_name(&text, &name),
        _ => true,
    }
}

/// `require_trusted` for a formula the install job installs only when brew has it (an
/// extension: with no formula the job builds it with pecl, which needs no tap). An
/// untrusted formula the tap does not list is let through; one it lists is refused.
pub async fn require_trusted_if_listed(brew: &Brew, formula: &str) -> Result<(), YardError> {
    let Err(refusal) = require_trusted(brew, formula).await else { return Ok(()) };
    if refusal.code != ErrorCode::FormulaUntrusted {
        return Err(refusal);
    }
    let (prefix, formula) = (brew.prefix.clone(), formula.to_string());
    // A task that panicked or was cancelled counts as listing it.
    let listed = tokio::task::spawn_blocking(move || tap_lists(&prefix, &formula)).await.unwrap_or(true);
    if listed { Err(refusal) } else { Ok(()) }
}

/// MinIO's formula in the `minio/stable` tap. homebrew/core's `minio` shares its keg
/// (`opt/minio`), so the keg alone does not say which one is installed.
pub const MINIO_TAP_FORMULA: &str = "minio/stable/minio";

/// Whether the MinIO keg at `opt/minio` is the tap's: when `services.toml` has a MinIO
/// instance, the formula it records says; with none, brew is asked (`brew info --json=v2
/// minio`, which describes the formula of the installed keg, within `limit`) and anything it
/// does not say is the tap's is not: a brew that fails or does not answer in time included.
/// The tap's formula is never reported for a core MinIO.
async fn minio_from_tap(brew: &Brew, services: Option<&ServicesConfig>, limit: Duration) -> bool {
    if !brew.prefix.join("opt/minio").exists() {
        return false;
    }
    let recorded: Vec<&str> = services
        .into_iter()
        .flat_map(|s| s.services.values())
        .filter(|entry| entry.engine == Engine::Minio)
        .map(|entry| entry.formula.as_str())
        .collect();
    if !recorded.is_empty() {
        return recorded.contains(&MINIO_TAP_FORMULA);
    }
    let asked = tokio::time::timeout(limit, brew_stdout(brew, &["info", "--json=v2", "minio"])).await;
    let Ok(Ok((true, out, _))) = asked else { return false };
    brew_info::parse(&out)
        .ok()
        .and_then(|formulae| formulae.into_iter().next())
        .is_some_and(|f| sources::tap_of(&f.full_name) == sources::tap_of(MINIO_TAP_FORMULA))
}

/// The formulae from Yard's taps that Yard knows are installed: the PHP versions' kegs,
/// the extensions `yard.toml` records from brew, and MinIO's when it is the tap's.
pub async fn installed(
    brew: &Brew,
    yard: &YardConfig,
    services: Option<&ServicesConfig>,
) -> BTreeSet<String> {
    installed_within(brew, yard, services, BREW_TIMEOUT).await
}

/// `installed`, with `limit` as the time brew gets to say whose MinIO keg it is.
pub async fn installed_within(
    brew: &Brew,
    yard: &YardConfig,
    services: Option<&ServicesConfig>,
    limit: Duration,
) -> BTreeSet<String> {
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let mut found: BTreeSet<String> =
        brew.installed_php(&exists, &read_link).into_iter().map(yard_core::php::formula).collect();
    for (v, exts) in &yard.extensions {
        found.extend(
            exts.iter().filter(|(_, e)| e.source == ExtSource::Brew).map(|(m, _)| ext_formula(m, *v)),
        );
    }
    if minio_from_tap(brew, services, limit).await {
        found.insert(MINIO_TAP_FORMULA.to_string());
    }
    found
}

/// The rows of `sources.list`.
pub async fn list(
    brew: &Brew,
    yard: &YardConfig,
    services: Option<&ServicesConfig>,
) -> Result<Vec<SourceInfo>, YardError> {
    let trust = read_trust(brew).await?;
    let installed = installed(brew, yard, services).await;
    let tapped = |tap: &str| {
        let dir = tap.split_once('/').map(|(owner, repo)| format!("{owner}/homebrew-{repo}"));
        dir.is_some_and(|d| brew.prefix.join("Library/Taps").join(d).is_dir())
    };
    Ok(sources::rows(trust.as_ref(), &tapped, &installed))
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
