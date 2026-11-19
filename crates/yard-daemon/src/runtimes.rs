//! Runtime versions Yard installs (runtimes spec §6): a recipe's `download` source, from its
//! pinned `[versions]` or its vendor's index, each version in `installs/<key>/<version>/`
//! with a generated `.yard-install.toml`, built in a stage beside it and renamed into place.

use crate::jobs::JobCtx;
use crate::programs::{fetch, stage_in};
use std::collections::BTreeMap;
use std::path::Path;
use yard_core::Paths;
use yard_core::runtimes::Runtimes;
use yard_core::runtimes::recipe::{IndexDef, RuntimeRecipe, SourceDef, VersionEntry, expand_version};
use yard_core::runtimes::select::{
    InstallRecord, composer_channel, composer_channels, index_version, parse_sha256sum, requires_php,
};
use yard_core::runtimes::version::{RuntimeVersion, VersionSpec, pick};
use yard_core::services::engines::LoadOptions;
use yard_proto::{ErrorCode, SettingSource, SiteInfo, YardError};

const CURL: &str = "/usr/bin/curl";

/// The runtimes this yardd knows: the carried recipes (user recipes come later).
pub fn registry() -> Runtimes {
    Runtimes::carried()
}

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

/// The installed versions of runtime `key`, oldest first, as the shims see them.
pub fn installed(paths: &Paths, key: &str) -> Vec<(RuntimeVersion, InstallRecord)> {
    yard_core::runtimes::select::installed(paths, key, &list_dir, &|p| std::fs::read_to_string(p).ok())
}

fn protocols(opts: &LoadOptions) -> &'static str {
    if opts.allow_file_urls { "=https,file" } else { "=https" }
}

/// `url`'s body, through curl.
async fn get(job: &JobCtx, url: &str, opts: &LoadOptions) -> Result<String, String> {
    let (ok, out) = job
        .capture_stdout(Path::new(CURL), &["-fsSL", "--proto", protocols(opts), url], &[])
        .await
        .map_err(|e| e.message)?;
    if !ok {
        let why = out.lines.last().cloned().unwrap_or_else(|| "curl failed".into());
        return Err(why);
    }
    Ok(out.stdout.join("\n"))
}

fn not_listed(recipe: &RuntimeRecipe, spec: &VersionSpec) -> YardError {
    let listed: Vec<String> = recipe.versions.keys().map(ToString::to_string).collect();
    YardError::new(
        ErrorCode::InvalidArgument,
        format!("{} {spec} is not a version its recipe lists ({})", recipe.name, listed.join(", ")),
    )
    .with_hint(format!("yard runtime install {} <one of those>", recipe.key))
}

/// The newest `[versions]` entry with a hash that meets `spec`, kept to `spec`'s own channel
/// when the recipe has an index, so `2` never falls back to the 2.2 LTS.
fn pinned(recipe: &RuntimeRecipe, spec: &VersionSpec, index: bool) -> Option<(RuntimeVersion, String)> {
    recipe
        .versions
        .iter()
        .filter(|(v, _)| v.prefix_matches(spec))
        .filter(|(v, _)| !index || spec.is_exact() || composer_channel(v) == spec.to_string())
        .filter_map(|(v, e)| match e {
            VersionEntry::Sha256(h) => Some((v.clone(), h.clone())),
            VersionEntry::Path(_) => None,
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

/// The version `spec` installs, its URL and the SHA-256 it must have. With an index: an
/// exact version is itself, a channel its newest release (or the recipe's pinned one when
/// the index is unreachable or drops the channel), any other major.minor is refused before
/// anything is fetched. The hash is the recipe's for a pinned version, else the vendor's
/// `sha256_url`. Without one: the recipe's `[versions]` only.
pub async fn resolve_install(
    job: &JobCtx,
    recipe: &RuntimeRecipe,
    spec: &VersionSpec,
    opts: &LoadOptions,
) -> Result<(RuntimeVersion, String, String), YardError> {
    let SourceDef::Download { url, index, .. } = &recipe.source else {
        return Err(YardError::internal(format!("{} is not installed by download", recipe.name)));
    };
    let (version, hash) = match index {
        None => pinned(recipe, spec, false).ok_or_else(|| not_listed(recipe, spec))?,
        Some(ix) => {
            let version = index_pick(job, recipe, ix, spec, opts).await?;
            let hash = match recipe.versions.get(&version) {
                Some(VersionEntry::Sha256(h)) => h.clone(),
                _ => {
                    let sum_url = expand_version(&ix.sha256_url, &version.to_string());
                    let text = get(job, &sum_url, opts).await.map_err(|why| {
                        YardError::new(ErrorCode::JobFailed, format!("cannot read {sum_url}: {why}"))
                            .with_hint("check the network, then try again")
                    })?;
                    parse_sha256sum(&text)
                        .map_err(|why| YardError::new(ErrorCode::JobFailed, format!("{sum_url}: {why}")))?
                }
            };
            (version, hash)
        }
    };
    Ok((version.clone(), expand_version(url, &version.to_string()), hash))
}

async fn index_pick(
    job: &JobCtx,
    recipe: &RuntimeRecipe,
    ix: &IndexDef,
    spec: &VersionSpec,
    opts: &LoadOptions,
) -> Result<RuntimeVersion, YardError> {
    // Refused before anything is fetched: the answer does not depend on the index.
    if let Err(why) = index_version(spec, &BTreeMap::new()) {
        return Err(YardError::new(ErrorCode::InvalidArgument, why)
            .with_hint(format!("yard runtime install {} 2.7.7", recipe.key)));
    }
    if spec.is_exact() {
        return Ok(RuntimeVersion(spec.parts.clone()));
    }
    let fallback = || pinned(recipe, spec, true).map(|(v, _)| v);
    match get(job, &ix.url, opts).await {
        Ok(json) => {
            let channels = composer_channels(&json)
                .map_err(|why| YardError::new(ErrorCode::JobFailed, format!("{}: {why}", ix.url)))?;
            let found = index_version(spec, &channels)
                .map_err(|why| YardError::new(ErrorCode::InvalidArgument, why))?;
            found.or_else(fallback).ok_or_else(|| {
                YardError::new(
                    ErrorCode::InvalidArgument,
                    format!("{} lists no {} {spec}", ix.url, recipe.name),
                )
                .with_hint(format!("name an exact version: yard runtime install {} 2.7.7", recipe.key))
            })
        }
        Err(why) => fallback().ok_or_else(|| {
            YardError::new(ErrorCode::JobFailed, format!("cannot read {}: {why}", ix.url)).with_hint(format!(
                "check the network, or name an exact version: yard runtime install {} 2.7.7",
                recipe.key
            ))
        }),
    }
}

/// Installs the version `spec` names, unless it is installed: downloads it into a stage in
/// `installs/<key>/`, checks it, writes its `.yard-install.toml` and renames it into place.
/// A failed or cancelled install leaves no stage and no version.
pub async fn install(
    job: &JobCtx,
    paths: &Paths,
    recipe: &RuntimeRecipe,
    spec: VersionSpec,
    opts: &LoadOptions,
) -> Result<RuntimeVersion, YardError> {
    let have = installed(paths, &recipe.key);
    if spec.is_exact() && have.iter().any(|(v, _)| v.prefix_matches(&spec)) {
        job.line(format!("{} {spec} is installed", recipe.name));
        return Ok(RuntimeVersion(spec.parts.clone()));
    }
    let (version, url, sha256) = resolve_install(job, recipe, &spec, opts).await?;
    if have.iter().any(|(v, _)| *v == version) {
        job.line(format!("{} {version} is installed", recipe.name));
        return Ok(version);
    }
    let SourceDef::Download { strip, file, .. } = &recipe.source else {
        return Err(YardError::internal(format!("{} is not installed by download", recipe.name)));
    };
    let dir = paths.installs_dir().join(&recipe.key);
    let stage = stage_in(&dir);
    let root = paths.install_root(&recipe.key, &version.to_string());
    let result = async {
        let unpacked = fetch(job, &stage, &url, &sha256, *strip, file.as_deref(), opts).await?;
        let record = InstallRecord {
            version: version.clone(),
            url: url.clone(),
            sha256: sha256.clone(),
            requires_php: requires_php(recipe, &version),
            installed_at: crate::services::rfc3339(time::OffsetDateTime::now_utc()).unwrap_or_default(),
        };
        let record_file = unpacked.join(".yard-install.toml");
        std::fs::write(&record_file, record.render()).map_err(|e| io("write", &record_file, e))?;
        // A directory there without its record is not an install (a half-deleted one).
        if root.exists() {
            std::fs::remove_dir_all(&root).map_err(|e| io("remove", &root, e))?;
        }
        std::fs::rename(&unpacked, &root).map_err(|e| io("move the download to", &root, e))
    }
    .await;
    let _ = std::fs::remove_dir_all(&stage);
    result?;
    job.line(format!("Installed {} {version}", recipe.name));
    Ok(version)
}

fn io(what: &str, path: &Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("cannot {what} {}: {e}", path.display()))
}

/// `RUNTIME_IN_USE` when `v` is the only installed version a linked site's runtime (from
/// `sites`' `runtimes`) or the default `default` resolves to, naming what to change.
pub fn in_use(
    paths: &Paths,
    recipe: &RuntimeRecipe,
    v: &RuntimeVersion,
    sites: &[SiteInfo],
    default: Option<&VersionSpec>,
) -> Result<(), YardError> {
    let others: Vec<RuntimeVersion> =
        installed(paths, &recipe.key).into_iter().map(|(iv, _)| iv).filter(|iv| iv != v).collect();
    let alone = |spec: &VersionSpec, exclude: &[VersionSpec]| {
        v.prefix_matches(spec) && pick(spec, exclude, &others).is_none()
    };
    for site in sites {
        for rt in site.runtimes.iter().filter(|rt| rt.key == recipe.key) {
            let (Some(spec), Some(version)) = (&rt.spec, &rt.version) else { continue };
            if *version != v.to_string() {
                continue;
            }
            let Ok(spec) = VersionSpec::parse(spec) else { continue };
            let exclude = match rt.source {
                SettingSource::Default => recipe
                    .defaults
                    .iter()
                    .find(|d| d.spec == spec)
                    .map(|d| d.exclude.clone())
                    .unwrap_or_default(),
                _ => vec![],
            };
            if alone(&spec, &exclude) {
                let hint = match (&rt.source, &rt.file) {
                    (SettingSource::ProjectFile, Some(file)) => {
                        format!("edit {file}, or install another {} {spec} first", recipe.name)
                    }
                    (SettingSource::ManagedFile | SettingSource::SitesToml, _) => {
                        format!("yard runtime unuse {} --site {}", recipe.key, site.name)
                    }
                    _ => format!("yard runtime install {} {spec} first", recipe.key),
                };
                return Err(YardError::new(
                    ErrorCode::RuntimeInUse,
                    format!(
                        "{} {v} is the only {} {spec} installed, and site {} runs it",
                        recipe.name, recipe.name, site.name
                    ),
                )
                .with_hint(hint));
            }
        }
    }
    if let Some(spec) = default
        && alone(spec, &[])
    {
        return Err(YardError::new(
            ErrorCode::RuntimeInUse,
            format!("{} {v} is the only {} {spec} installed, the default", recipe.name, recipe.name),
        )
        .with_hint(format!("yard runtime default {} <another installed version>", recipe.key)));
    }
    Ok(())
}

/// Removes `installs/<key>/<v>/`. A `path` version is the user's and never removed.
pub async fn uninstall(
    job: &JobCtx,
    paths: &Paths,
    recipe: &RuntimeRecipe,
    v: &RuntimeVersion,
) -> Result<(), YardError> {
    if let Some(VersionEntry::Path(p)) = recipe.versions.get(v) {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{} {v} is {}, which Yard did not install", recipe.name, p.display()),
        ));
    }
    if !installed(paths, &recipe.key).iter().any(|(iv, _)| iv == v) {
        return Err(YardError::new(
            ErrorCode::RuntimeMissing,
            format!("{} {v} is not installed", recipe.name),
        )
        .with_hint(format!("yard runtime list {}", recipe.key)));
    }
    let root = paths.install_root(&recipe.key, &v.to_string());
    std::fs::remove_dir_all(&root).map_err(|e| io("remove", &root, e))?;
    job.line(format!("Removed {} {v}", recipe.name));
    Ok(())
}
