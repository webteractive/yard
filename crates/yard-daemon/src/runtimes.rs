//! Runtime versions Yard installs (runtimes spec §6): a recipe's `download` source, from its
//! pinned `[versions]` or its vendor's index, each version in `installs/<key>/<version>/`
//! with a generated `.yard-install.toml`, built in a stage beside it and renamed into place.

use crate::jobs::JobCtx;
use crate::programs::{fetch, stage_in};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_core::Paths;
use yard_core::config::{SiteEntry, SitesConfig, YardConfig, load_sites, load_yard};
use yard_core::project::{FileKind, ProjectFile};
use yard_core::runtimes::recipe::{IndexDef, RuntimeRecipe, SourceDef, VersionEntry, expand_version};
use yard_core::runtimes::resolve::{PinSource, failed_source, pin_file_version, resolve_pin, uses_runtime};
use yard_core::runtimes::select::{
    InstallRecord, composer_channel, composer_channels, fill_candidate, fitting, index_version, needs_pin,
    offer_spec, parse_sha256sum, requires_php,
};
use yard_core::runtimes::shim::{Pick, pick_composer};
use yard_core::runtimes::version::{RuntimeVersion, VersionSpec, pick};
use yard_core::runtimes::{RuntimeKind, Runtimes};
use yard_core::services::engines::LoadOptions;
use yard_proto::{ErrorCode, SettingSource, SiteInfo, SiteKind, SiteRuntime, YardError};

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
    // An answer Yard cannot read (a captive portal's page) is as good as none.
    match get(job, &ix.url, opts).await.and_then(|json| composer_channels(&json)) {
        Ok(channels) => {
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
            remove(&root, &dir)?;
        }
        std::fs::rename(&unpacked, &root).map_err(|e| io("move the download to", &root, e))?;
        // Sealed once in place: macOS will not move a read-only directory to another parent.
        seal(&root).inspect_err(|_| {
            let _ = remove(&root, &dir);
        })
    }
    .await;
    let _ = unseal(&stage);
    let _ = std::fs::remove_dir_all(&stage);
    result?;
    job.line(format!("Installed {} {version}", recipe.name));
    Ok(version)
}

/// Makes an install read-only (files 0444, directories 0555): Composer's self-update refuses
/// a phar it cannot write, so nothing replaces the file Yard checked.
fn seal(dir: &Path) -> Result<(), YardError> {
    set_modes(dir, 0o444, 0o555)
}

/// Gives the user write access back, so the install can be moved and removed.
fn unseal(dir: &Path) -> Result<(), YardError> {
    set_modes(dir, 0o644, 0o755)
}

/// Directories get `dirs` after their contents, so a sealed one is still walked.
fn set_modes(dir: &Path, files: u32, dirs: u32) -> Result<(), YardError> {
    use std::os::unix::fs::PermissionsExt;
    let set = |p: &Path, mode: u32| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
            .map_err(|e| io("change the mode of", p, e))
    };
    // Open the directory before reading it, in case it is sealed already.
    set(dir, 0o755)?;
    for entry in std::fs::read_dir(dir).map_err(|e| io("read", dir, e))?.flatten() {
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| io("read", &path, e))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            set_modes(&path, files, dirs)?;
        } else {
            set(&path, files)?;
        }
    }
    set(dir, dirs)
}

/// Moves `root` out of the way into a stage in `dir`, at once, then deletes it: a shim sees
/// the whole version or none of it.
fn remove(root: &Path, dir: &Path) -> Result<(), YardError> {
    unseal(root)?;
    let gone = stage_in(dir);
    std::fs::rename(root, &gone).map_err(|e| io("move away", root, e))?;
    std::fs::remove_dir_all(&gone).map_err(|e| io("remove", &gone, e))
}

fn io(what: &str, path: &Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("cannot {what} {}: {e}", path.display()))
}

/// `RUNTIME_MISSING` for a spec nothing installed meets, hinting at the install.
pub fn missing_spec(recipe: &RuntimeRecipe, spec: &VersionSpec) -> YardError {
    yard_core::runtimes::shim::missing(recipe, spec, &PinSource::Default)
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
    let alone = |spec: &VersionSpec| v.prefix_matches(spec) && pick(spec, &others).is_none();
    for site in sites {
        for rt in site.runtimes.iter().filter(|rt| rt.key == recipe.key) {
            let (Some(spec), Some(version)) = (&rt.spec, &rt.version) else { continue };
            if *version != v.to_string() {
                continue;
            }
            let Ok(spec) = VersionSpec::parse(spec) else { continue };
            if alone(&spec) {
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
    // An exact default (what Yard writes) holds its version whatever else is installed.
    if let Some(spec) = default
        && spec.is_exact()
        && RuntimeVersion(spec.parts.clone()) == *v
    {
        let hint = match fill_candidate(recipe, &others).or_else(|| others.last().cloned()) {
            Some(o) => format!("yard runtime default {} {o}", recipe.key),
            None => format!("yard runtime install {}", recipe.key),
        };
        return Err(YardError::new(
            ErrorCode::RuntimeInUse,
            format!("{} {v} is the default in {}", recipe.name, paths.yard_toml().display()),
        )
        .with_hint(hint));
    }
    if let Some(spec) = default
        && alone(spec)
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
    remove(&root, &paths.installs_dir().join(&recipe.key))?;
    job.line(format!("Removed {} {v}", recipe.name));
    Ok(())
}

/// Brew's `composer`, which the shim falls back to for one release (runtimes spec §9.3).
pub fn brew_composer(brew: &yard_core::brew::Brew) -> Option<PathBuf> {
    Some(brew.prefix.join("bin/composer")).filter(|p| p.exists())
}

/// Whether the `composer` shim runs brew's Composer for a project on PHP `php_full`: Yard has
/// none installed, brew's is there and the PHP is new enough for it. A pin changes nothing
/// while nothing is installed, so none is passed.
pub fn brew_composer_runs(
    paths: &Paths,
    brew: &yard_core::brew::Brew,
    recipe: &RuntimeRecipe,
    php_full: &RuntimeVersion,
) -> bool {
    let picked =
        pick_composer(recipe, None, php_full, "", &installed(paths, &recipe.key), brew_composer(brew), paths);
    matches!(picked, Ok(Pick::BrewComposer(_)))
}

/// The full version of the PHP keg `v` (`7.1.33`), or `x.y.0` when it cannot be read.
pub fn php_full(brew: &yard_core::brew::Brew, v: yard_core::PhpVersion) -> RuntimeVersion {
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let full = brew
        .php_opt(v, &|p: &Path| p.exists(), &read_link)
        .ok()
        .and_then(|opt| read_link(&opt))
        .and_then(|target| yard_core::php::full_version(&target.to_string_lossy()));
    yard_core::runtimes::select::php_full_or_zero(full.as_deref(), v)
}

fn setting(source: &PinSource) -> (SettingSource, Option<String>) {
    match source {
        PinSource::Project(f) => (SettingSource::ProjectFile, Some(f.display().to_string())),
        PinSource::Managed(f) => (SettingSource::ManagedFile, Some(f.display().to_string())),
        PinSource::Site(_) => (SettingSource::SitesToml, None),
        PinSource::PinFile(f) => (SettingSource::PinFile, Some(f.display().to_string())),
        PinSource::Default => (SettingSource::Default, None),
    }
}

/// The pin `dir` resolves to for a recipe runtime, as its shim run there would see it; `None`
/// when nothing pins it there.
pub fn resolve_at(
    dir: &Path,
    recipe: &RuntimeRecipe,
    sites: &SitesConfig,
    paths: &Paths,
) -> Result<Option<(VersionSpec, PinSource)>, YardError> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let key = recipe.key.as_str();
    resolve_pin(
        dir,
        &recipe.pin_files,
        sites,
        paths,
        &crate::node::read_pin,
        &canonical,
        &|f: &ProjectFile| f.runtimes.get(key).cloned(),
        &|s: &SiteEntry| s.runtimes.get(key).cloned(),
        &pin_file_version,
    )
}

/// What a site runs: its pin, else `yard.toml`'s default when the site's PHP `php_full` runs
/// it (explicit-defaults spec §2.1). `None`: nothing written applies, and the shim refuses.
pub fn pin_or_default(
    recipe: &RuntimeRecipe,
    pin: Option<(VersionSpec, PinSource)>,
    yard: &YardConfig,
    installed: &[RuntimeVersion],
    php_full: &RuntimeVersion,
) -> Option<(VersionSpec, PinSource)> {
    pin.or_else(|| {
        let default = yard.runtime_defaults.get(&recipe.key)?;
        (!needs_pin(recipe, Some(default), installed, php_full))
            .then(|| (default.clone(), PinSource::Default))
    })
}

/// The spec a site's runtime resolves to and where from: [`pin_or_default`], else what Yard
/// offers it to install ([`offer_spec`]: `2.2` for a PHP below the boundary, else `2`).
pub fn spec_for(
    recipe: &RuntimeRecipe,
    pin: Option<(VersionSpec, PinSource)>,
    yard: &YardConfig,
    installed: &[RuntimeVersion],
    php_full: &RuntimeVersion,
) -> Option<(VersionSpec, PinSource)> {
    pin_or_default(recipe, pin, yard, installed, php_full).or_else(|| {
        let below = needs_pin(recipe, yard.runtime_defaults.get(&recipe.key), installed, php_full);
        offer_spec(recipe, php_full, below).map(|s| (s, PinSource::Default))
    })
}

/// A committed `.yard` whose PHP cannot run the default: the line to add, as the version that
/// fits (installed, else the one Yard offers), in `data.needs_pin`.
fn committed_pin_error(
    recipe: &RuntimeRecipe,
    installed: &[RuntimeVersion],
    php_full: &RuntimeVersion,
    site: &SiteInfo,
) -> YardError {
    let fit = fitting(recipe, installed, php_full)
        .map(|v| v.to_string())
        .or_else(|| offer_spec(recipe, php_full, true).map(|s| s.to_string()))
        .unwrap_or_default();
    let file = site.project_file.clone().unwrap_or_else(|| ".yard".into());
    YardError::new(
        ErrorCode::RuntimeMissing,
        format!(
            "{}'s PHP {php_full} cannot run the {} default, and its .yard pins none",
            site.name, recipe.name
        ),
    )
    .with_hint(format!("add [runtimes] {} = \"{fit}\" to {file}", recipe.key))
    .with_data(serde_json::json!({ "needs_pin": fit }))
}

/// Fills `runtimes` on each linked PHP site whose root exists (runtimes spec §7): every recipe
/// runtime the site uses (one of its `uses_when` files in the root, or its own pin), with the
/// spec it resolves to and the installed version that runs, or why none does; and every
/// `[runtimes]` key of its `.yard` or `sites.toml` entry no runtime defines, as
/// `RUNTIME_UNKNOWN`. Nothing is filled while `yard.toml` or `sites.toml` does not parse.
pub fn fill_runtimes(
    sites: &mut [SiteInfo],
    paths: &Paths,
    runtimes: &Runtimes,
    brew: &yard_core::brew::Brew,
) {
    let (Ok((yard, _)), Ok(config)) = (load_yard(paths), load_sites(paths)) else { return };
    let (projects, _) = yard_core::project::load(&config, paths, &crate::project::read_project_file);
    for site in sites {
        site.runtimes = vec![];
        let SiteKind::Php { path, php, .. } = &site.kind else { continue };
        let Ok(root) = std::fs::canonicalize(path) else { continue };
        if !root.is_dir() {
            continue;
        }
        let php_full = match php.parse() {
            Ok(v) => php_full(brew, v),
            Err(_) => continue,
        };
        for rt in runtimes.iter().filter(|r| r.kind() == RuntimeKind::Recipe) {
            let recipe = rt.recipe();
            let has_uses = recipe.uses_when.iter().any(|f| root.join(f).is_file());
            let entry = match resolve_at(&root, recipe, &config, paths) {
                Ok(pin) => {
                    let source = pin.as_ref().map_or(PinSource::Default, |(_, s)| s.clone());
                    if !uses_runtime(&source, &root, has_uses) {
                        continue;
                    }
                    let installs = installed(paths, &recipe.key);
                    let versions: Vec<RuntimeVersion> = installs.iter().map(|(v, _)| v.clone()).collect();
                    let Some((spec, source)) = spec_for(recipe, pin.clone(), &yard, &versions, &php_full)
                    else {
                        continue;
                    };
                    let (setting, file) = setting(&source);
                    // yard.toml's default counts as a pin where the site's PHP runs it.
                    let has_own_pin = pin.is_some();
                    let pin = pin_or_default(recipe, pin, &yard, &versions, &php_full);
                    let picked = pick_composer(
                        recipe,
                        pin,
                        &php_full,
                        &format!("site {}", site.name),
                        &installs,
                        None,
                        paths,
                    );
                    // A committed `.yard` that needs its own pin is the user's to edit: say which
                    // line (explicit-defaults spec §2.1). Yard pins any other site itself.
                    let committed = site.project_file_kind == Some(yard_proto::ProjectFileKind::Project);
                    let needs_own = committed
                        && !has_own_pin
                        && needs_pin(recipe, yard.runtime_defaults.get(&recipe.key), &versions, &php_full);
                    let (version, error) = match picked {
                        _ if needs_own => {
                            (None, Some(committed_pin_error(recipe, &versions, &php_full, site)))
                        }
                        Ok(Pick::Run(v, _)) => (Some(v.to_string()), None),
                        Ok(Pick::BrewComposer(_)) => (None, None),
                        Err(e) if e.code == ErrorCode::RuntimeMissing => (None, None),
                        Err(e) => (None, Some(e)),
                    };
                    SiteRuntime {
                        key: recipe.key.clone(),
                        name: recipe.name.clone(),
                        spec: Some(spec.to_string()),
                        source: setting,
                        file,
                        version,
                        error,
                    }
                }
                // A `.yard` that does not parse is the site's `project_error` already.
                Err(e)
                    if e.code == ErrorCode::ProjectFileInvalid
                        && (site.project_error.is_some() || !has_uses) =>
                {
                    continue;
                }
                Err(e) => {
                    let (source, file) =
                        failed_source(&e, paths).map_or((SettingSource::Default, None), |s| setting(&s));
                    SiteRuntime {
                        key: recipe.key.clone(),
                        name: recipe.name.clone(),
                        spec: None,
                        source,
                        file,
                        version: None,
                        error: Some(e),
                    }
                }
            };
            site.runtimes.push(entry);
        }
        // Keys no runtime defines: the site's file owns them, else its sites.toml entry.
        let (keys, source, file) = match projects.get(&site.name) {
            Some(loaded) => {
                let Some(parsed) = loaded.ok() else { continue };
                let source = match loaded.kind {
                    FileKind::Managed => SettingSource::ManagedFile,
                    FileKind::Committed => SettingSource::ProjectFile,
                };
                (parsed.runtimes.clone(), source, Some(loaded.file.display().to_string()))
            }
            None => match config.sites.get(&site.name) {
                Some(entry) => (entry.runtimes.clone(), SettingSource::SitesToml, None),
                None => continue,
            },
        };
        for (key, spec) in keys.into_iter().filter(|(k, _)| runtimes.get(k).is_none()) {
            site.runtimes.push(SiteRuntime {
                name: key.clone(),
                spec: Some(spec.to_string()),
                source,
                file: file.clone(),
                version: None,
                error: Some(unknown(&key)),
                key,
            });
        }
    }
}

/// `RUNTIME_UNKNOWN` for a key no runtime defines.
pub fn unknown(key: &str) -> YardError {
    YardError::new(ErrorCode::RuntimeUnknown, format!("no runtime is called {key}"))
        .with_hint(format!("add ~/.yard/runtimes/{key}.toml"))
}

/// `fill_node` and `fill_runtimes`: what every answer that carries a site fills per request.
pub fn fill_sites(sites: &mut [SiteInfo], paths: &Paths, runtimes: &Runtimes, brew: &yard_core::brew::Brew) {
    crate::node::fill_node(sites, paths);
    fill_runtimes(sites, paths, runtimes, brew);
}
