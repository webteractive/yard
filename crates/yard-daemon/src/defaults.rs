//! Filling in missing defaults (explicit-defaults spec §2.2): a runtime's default is an exact
//! version written in `yard.toml`, and when one is not written yardd writes it from what is
//! installed. A written default is never moved here. On the same check, a linked site whose PHP
//! cannot run the Composer default gets its own pin (spec §2.1).
//!
//! The check runs at the start of every reconcile, so a fresh Mac, a deleted line, an install
//! and a site moved to an old PHP are all the same case. Its own writes cause one more
//! reconcile, which finds nothing missing.

use crate::managed::{SiteFile, set_site_runtime, site_file};
use crate::runtimes::{installed, php_full, resolve_at};
use std::path::Path;
use toml_edit::value;
use yard_core::brew::Brew;
use yard_core::config::{SitesConfig, TomlFile, YardConfig, load_sites, load_yard};
use yard_core::node::{FIRST_DEFAULT as FIRST_NODE, pick as pick_node};
use yard_core::runtimes::recipe::RuntimeRecipe;
use yard_core::runtimes::select::{fill_candidate, fitting, needs_pin};
use yard_core::runtimes::version::{RuntimeVersion, VersionSpec};
use yard_core::runtimes::{RuntimeKind, Runtimes};
use yard_core::{Paths, PhpVersion};
use yard_proto::YardError;

/// One value the check wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The runtime: `php`, `node`, or a recipe's key.
    pub key: String,
    /// The site the pin was written for; `None` for a default in `yard.toml`.
    pub site: Option<String>,
    /// The value as written: `default_php = "8.4"`, `[runtimes.composer] default = "2.11.2"`,
    /// or `[runtimes] composer = "2.2.32"` for a site.
    pub line: String,
    /// The version written.
    pub version: String,
}

/// Writes every missing default, then every old-PHP pin, and answers what it wrote. Does
/// nothing while `yard.toml` or `sites.toml` does not parse; a write that fails is logged and
/// skipped, never failing the reconcile.
pub fn fill(paths: &Paths, runtimes: &Runtimes, brew: &Brew) -> Vec<Written> {
    let (Ok((mut yard, _)), Ok(sites)) = (load_yard(paths), load_sites(paths)) else { return vec![] };
    let mut written = vec![];
    if yard.default_php.is_none()
        && let Some(v) = php_candidate(brew)
        && let Some(w) = write_default(paths, &["default_php"], "php", "default_php", &v.to_string())
    {
        yard.default_php = Some(v);
        written.push(w);
    }
    if yard.default_node.is_none()
        && let Some(v) = node_candidate(paths)
        && let Some(w) = write_default(paths, &["default_node"], "node", "default_node", &v)
    {
        yard.default_node = yard_core::node::parse_spec(&v).ok();
        written.push(w);
    }
    for rt in runtimes.iter().filter(|r| r.kind() == RuntimeKind::Recipe) {
        let key = rt.key();
        if yard.runtime_defaults.contains_key(key) {
            continue;
        }
        let Some(v) = fill_candidate(rt.recipe(), &versions(paths, key)) else { continue };
        let line = format!("[runtimes.{key}] default");
        if let Some(w) = write_default(paths, &["runtimes", key, "default"], key, &line, &v.to_string())
            && let Ok(spec) = VersionSpec::parse(&w.version)
        {
            yard.runtime_defaults.insert(key.to_string(), spec);
            written.push(w);
        }
    }
    for rt in runtimes.iter().filter(|r| r.kind() == RuntimeKind::Recipe && !r.recipe().requires.is_empty()) {
        written.extend(pin_old_sites(paths, brew, rt.recipe(), &yard, &sites));
    }
    written
}

/// PHP's first default (8.4) when it is installed, else the newest installed line.
fn php_candidate(brew: &Brew) -> Option<PhpVersion> {
    let installed = brew.installed_php(&|p: &Path| p.exists(), &|p: &Path| std::fs::read_link(p).ok());
    if installed.contains(&PhpVersion::FIRST_DEFAULT) {
        return Some(PhpVersion::FIRST_DEFAULT);
    }
    installed.last().copied()
}

/// Node's newest installed 22.x, else the newest installed version, exact.
fn node_candidate(paths: &Paths) -> Option<String> {
    let installed = crate::node::installed(paths);
    let lts = crate::node::lts(paths);
    pick_node(&FIRST_NODE, &installed, &lts)
        .or_else(|| installed.iter().max().copied())
        .map(|v| v.to_string())
}

fn versions(paths: &Paths, key: &str) -> Vec<RuntimeVersion> {
    installed(paths, key).into_iter().map(|(v, _)| v).collect()
}

/// Sets `keys` in `yard.toml` to `version` unless something is there already (written by a
/// command since `yard.toml` was read), keeping every comment.
fn write_default(paths: &Paths, keys: &[&str], key: &str, line: &str, version: &str) -> Option<Written> {
    let result = TomlFile::edit(&paths.yard_toml(), |file| {
        let mut item = Some(file.doc().as_item());
        for k in keys {
            item = item.and_then(|i| i.get(k));
        }
        if item.is_some() {
            return Ok(false);
        }
        file.set(keys, value(version));
        Ok(true)
    });
    match result {
        Ok(true) => {
            let line = format!("{line} = \"{version}\"");
            tracing::info!("wrote {line} to {}", paths.yard_toml().display());
            Some(Written { key: key.to_string(), site: None, line, version: version.to_string() })
        }
        Ok(false) => None,
        Err(e) => {
            tracing::warn!("could not write {line} to yard.toml: {}", e.message);
            None
        }
    }
}

/// The pins §2.1 writes: for each linked site that uses `recipe` (one of its `uses_when` files
/// in the root), has no pin of its own and no committed `.yard`, and whose PHP cannot run the
/// written default, the newest installed version it can run.
fn pin_old_sites(
    paths: &Paths,
    brew: &Brew,
    recipe: &RuntimeRecipe,
    yard: &YardConfig,
    sites: &SitesConfig,
) -> Vec<Written> {
    let installed = versions(paths, &recipe.key);
    if installed.is_empty() {
        return vec![];
    }
    let default = yard.runtime_defaults.get(&recipe.key);
    let mut written = vec![];
    for (name, entry) in &sites.sites {
        let Some(path) = &entry.path else { continue };
        let Ok(root) = std::fs::canonicalize(path) else { continue };
        if !recipe.uses_when.iter().any(|f| root.join(f).is_file()) {
            continue;
        }
        if !matches!(resolve_at(&root, recipe, sites, paths), Ok(None)) {
            continue;
        }
        if !matches!(site_file(paths, name), Ok(SiteFile::Managed(_) | SiteFile::SitesToml)) {
            continue;
        }
        let Some(php) = site_php(paths, yard, sites, &root) else { continue };
        let php = php_full(brew, php);
        if !needs_pin(recipe, default, &installed, &php) {
            continue;
        }
        let Some(fit) = fitting(recipe, &installed, &php) else { continue };
        let line = format!("[runtimes] {} = \"{fit}\"", recipe.key);
        match set_site_runtime(paths, name, &recipe.key, Some(&fit.to_string())) {
            Ok(()) => {
                tracing::info!("wrote {line} for site {name}, whose PHP {php} cannot run the default");
                written.push(Written {
                    key: recipe.key.clone(),
                    site: Some(name.clone()),
                    line,
                    version: fit.to_string(),
                });
            }
            Err(e) => tracing::warn!("could not pin {} for site {name}: {}", recipe.key, e.message),
        }
    }
    written
}

/// The PHP a site's root runs, as the shim resolves it.
fn site_php(paths: &Paths, yard: &YardConfig, sites: &SitesConfig, root: &Path) -> Option<PhpVersion> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let resolved: Result<_, YardError> =
        yard_core::shim::resolve_version(root, yard, sites, paths, &crate::node::read_pin, &canonical);
    resolved.ok().map(|(v, _)| v)
}

impl Written {
    /// What a person reads about the write: in an install's output and doctor's info line.
    pub fn describe(&self) -> String {
        match &self.site {
            None => format!(
                "wrote {} to yard.toml; yard runtime default {} <version> changes it",
                self.line, self.key
            ),
            Some(site) => format!(
                "wrote {} for site {site}, whose PHP cannot run the default; yard runtime unuse {} --site {site} removes it",
                self.line, self.key
            ),
        }
    }
}

/// The reconcile an install job ends with (explicit-defaults spec §2.2): the fill-in may now
/// write a default or a pin, and the job's output says which.
pub async fn reconcile_after_install(
    job: &crate::jobs::JobCtx,
    reconciler: &crate::reconcile::Reconciler,
) -> Result<(), YardError> {
    let before = reconciler.state().await.defaults_written.len();
    reconciler.reconcile().await?;
    for w in reconciler.state().await.defaults_written.iter().skip(before) {
        job.line(w.describe());
    }
    Ok(())
}
