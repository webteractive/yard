//! What `download` and `npm` recipes install, and the `path` program check (recipes spec §5):
//! each version in its own `programs/<key>/<version>/`, built in a stage beside it and renamed
//! into place, so a failed or cancelled install leaves nothing.

use crate::jobs::JobCtx;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use yard_core::Paths;
use yard_core::config::ServicesConfig;
use yard_core::services::Engines;
use yard_core::services::engines::{LoadOptions, ServiceEngine, SourceSpec, recipe::path_allowed};
use yard_proto::{ErrorCode, YardError};

const CURL: &str = "/usr/bin/curl";
const TAR: &str = "/usr/bin/tar";

/// Makes what `engine` runs available, before an instance is added or moved: a `path` program
/// must exist and be allowed, a `download` or `npm` pin is installed when it is not. Answers
/// the version the instance records (the pin), or `None` for a `path` one. A brew engine is
/// the add job's own business.
pub async fn install(
    job: &JobCtx,
    paths: &Paths,
    home: &Path,
    engine: &dyn ServiceEngine,
    opts: &LoadOptions,
) -> Result<Option<String>, YardError> {
    match engine.source() {
        SourceSpec::Path { program } => {
            path_allowed(&program, home)?;
            if !program.is_file() {
                return Err(YardError::new(
                    ErrorCode::EngineNotInstalled,
                    format!("{} is not there: a path recipe runs a program you installed", program.display()),
                )
                .with_hint(format!(
                    "install it at {}, or fix program in the {} recipe",
                    program.display(),
                    engine.key()
                )));
            }
            Ok(None)
        }
        SourceSpec::Download { version, url, sha256, strip } => {
            let root = paths.program_root(engine.key(), &version);
            if root.is_dir() {
                job.line(format!("{} {version} is installed", engine.display()));
            } else {
                download(job, paths, engine.key(), &version, &url, &sha256, strip, opts).await?;
            }
            Ok(Some(version))
        }
        SourceSpec::Npm { version, .. } => Err(YardError::new(
            ErrorCode::EngineNotInstalled,
            format!("{} {version} comes from npm, which this Yard cannot install yet", engine.display()),
        )),
        SourceSpec::Brew { .. } | SourceSpec::BrewLines(_) => {
            Err(YardError::internal("a brew engine installs through brew"))
        }
    }
}

/// `programs/<key>/.stage-<pid>-<n>`, a fresh directory beside the install roots.
fn stage_dir(paths: &Paths, key: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    paths.programs_dir().join(key).join(format!(".stage-{}-{n}", std::process::id()))
}

fn io(what: &str, path: &Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("cannot {what} {}: {e}", path.display()))
}

/// Downloads `url` with curl, checks its SHA-256, unpacks it (or keeps a bare program) and
/// renames the stage to the install root. The stage goes on any failure or cancel.
#[allow(clippy::too_many_arguments)]
async fn download(
    job: &JobCtx,
    paths: &Paths,
    key: &str,
    version: &str,
    url: &str,
    sha256: &str,
    strip: u32,
    opts: &LoadOptions,
) -> Result<(), YardError> {
    let stage = stage_dir(paths, key);
    let result = fetch_into(job, &stage, url, sha256, strip, opts).await.and_then(|unpacked| {
        let root = paths.program_root(key, version);
        std::fs::rename(&unpacked, &root).map_err(|e| io("move the download to", &root, e))
    });
    let _ = std::fs::remove_dir_all(&stage);
    result
}

async fn fetch_into(
    job: &JobCtx,
    stage: &Path,
    url: &str,
    sha256: &str,
    strip: u32,
    opts: &LoadOptions,
) -> Result<PathBuf, YardError> {
    std::fs::create_dir_all(stage).map_err(|e| io("create", stage, e))?;
    let file = stage.join("download");
    let protocols = if opts.allow_file_urls { "=https,file" } else { "=https" };
    job.line(format!("downloading {url}"));
    let out = file.display().to_string();
    job.run(Path::new(CURL), &["-fsSL", "--proto", protocols, "-o", &out, url], &[]).await?;
    job.check_cancelled()?;
    let bytes = std::fs::read(&file).map_err(|e| io("read", &file, e))?;
    let got = yard_core::install::sha256_hex(&bytes);
    if got != sha256 {
        return Err(YardError::new(
            ErrorCode::DownloadHashMismatch,
            format!("{url} has SHA-256 {got}, not the {sha256} its recipe names"),
        )
        .with_hint("check the file, then correct sha256 in the recipe"));
    }
    let root = stage.join("root");
    std::fs::create_dir_all(&root).map_err(|e| io("create", &root, e))?;
    let archive = bytes.starts_with(&[0x1f, 0x8b])
        || bytes.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a])
        || bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04]);
    if archive {
        let strip = strip.to_string();
        let (to, from) = (root.display().to_string(), file.display().to_string());
        job.run(Path::new(TAR), &["-xf", &from, "-C", &to, "--strip-components", &strip], &[]).await?;
    } else {
        // A bare program, kept under its URL's last segment.
        let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("program");
        let program = root.join(name);
        std::fs::rename(&file, &program).map_err(|e| io("move", &program, e))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| io("make executable", &program, e))?;
    }
    job.check_cancelled()?;
    Ok(root)
}

/// Whether the program `engine` pins is there now: its keg for brew, its file for `path`, its
/// install root for `download` and `npm`.
pub fn installed(engine: &dyn ServiceEngine, brew_prefix: &Path, paths: &Paths) -> bool {
    match engine.source() {
        SourceSpec::Brew { formula } => {
            brew_prefix.join("opt").join(formula.rsplit('/').next().unwrap_or(&formula)).is_dir()
        }
        SourceSpec::BrewLines(spec) => std::fs::read_dir(brew_prefix.join("opt")).is_ok_and(|dir| {
            dir.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name == spec.base_formula || name.starts_with(&format!("{}@", spec.base_formula))
            })
        }),
        SourceSpec::Path { program } => program.is_file(),
        SourceSpec::Download { version, .. } | SourceSpec::Npm { version, .. } => {
            paths.program_root(engine.key(), &version).is_dir()
        }
    }
}

/// Deletes the install roots no instance records and no recipe pins, and answers what it
/// deleted. Stages a killed yardd left go too.
pub fn sweep(paths: &Paths, services: &ServicesConfig, engines: &Engines) -> Vec<PathBuf> {
    let mut keep: BTreeSet<(String, String)> =
        services.services.values().filter_map(|e| Some((e.engine.to_string(), e.version.clone()?))).collect();
    for engine in engines.all() {
        if let SourceSpec::Download { version, .. } | SourceSpec::Npm { version, .. } = engine.source() {
            keep.insert((engine.key().to_string(), version));
        }
    }
    let mut removed = vec![];
    let Ok(keys) = std::fs::read_dir(paths.programs_dir()) else { return removed };
    for key_dir in keys.flatten().filter(|e| e.path().is_dir()) {
        let key = key_dir.file_name().to_string_lossy().into_owned();
        for version_dir in std::fs::read_dir(key_dir.path()).into_iter().flatten().flatten() {
            let version = version_dir.file_name().to_string_lossy().into_owned();
            let stale_stage = version.starts_with(".stage-")
                && !version.starts_with(&format!(".stage-{}-", std::process::id()));
            if (stale_stage || !version.starts_with('.'))
                && !keep.contains(&(key.clone(), version))
                && std::fs::remove_dir_all(version_dir.path()).is_ok()
            {
                removed.push(version_dir.path());
            }
        }
    }
    removed
}
