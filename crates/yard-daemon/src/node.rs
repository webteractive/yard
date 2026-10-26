//! A site's Node, read from the files on disk on every request (M4c spec §5–§7). Nothing
//! here runs fnm: only the `node.install` and `node.uninstall` jobs and
//! `node.list --available` do.

use crate::project::read_project_file;
use std::path::{Path, PathBuf};
use yard_core::Paths;
use yard_core::config::{SitesConfig, YardConfig, load_sites, load_yard};
use yard_core::node::{LtsMap, NodeSpec, NodeVersion, parse_spec, pick};
use yard_core::shim::{NodeSource, resolve_node, uses_node};
use yard_proto::{ErrorCode, SettingSource, SiteInfo, SiteKind, SiteNode, YardError};

/// The versions fnm has fully installed under `node/`, oldest first.
pub fn installed(paths: &Paths) -> Vec<NodeVersion> {
    let list = |dir: &Path| {
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    };
    yard_core::node::installed(&paths.node_versions_dir(), &list, &|p: &Path| p.exists())
}

/// `lts.toml` when it parses, else the built-in map.
pub fn lts(paths: &Paths) -> LtsMap {
    std::fs::read_to_string(paths.lts_toml())
        .ok()
        .and_then(|t| LtsMap::parse(&t))
        .unwrap_or_else(LtsMap::builtin)
}

/// A `.yard`, `.nvmrc` or `.node-version` as the shim reads it: `None` when absent or a
/// directory, and an error for one that is there but unusable, `INVALID_ARGUMENT` unless it
/// is a `.yard`.
pub fn read_pin(file: &Path) -> Option<Result<String, YardError>> {
    if file.is_dir() {
        return None;
    }
    let read = read_project_file(file)?;
    if file.file_name().is_some_and(|n| n == yard_core::project::FILE) {
        return Some(read);
    }
    Some(read.map_err(|e| YardError { code: ErrorCode::InvalidArgument, ..e }))
}

/// The spec `dir` resolves to, as a shim run there would.
pub fn resolve_at(
    dir: &Path,
    yard: &YardConfig,
    sites: &SitesConfig,
    paths: &Paths,
) -> Result<(NodeSpec, NodeSource), YardError> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    resolve_node(dir, yard, sites, paths, &read_pin, &canonical)
}

/// Fills `node` on each linked PHP site whose root exists and uses Node: it has a
/// `package.json`, or a pin from anywhere but the default. A pin that cannot be read is
/// the site's `node.error`. Nothing is filled while `yard.toml` or `sites.toml` does not
/// parse; doctor reports those.
pub fn fill_node(sites: &mut [SiteInfo], paths: &Paths) {
    let (Ok((yard, _)), Ok(config)) = (load_yard(paths), load_sites(paths)) else { return };
    let (installed, lts) = (installed(paths), lts(paths));
    for site in sites {
        site.node = None;
        let SiteKind::Php { path, .. } = &site.kind else { continue };
        let Ok(root) = std::fs::canonicalize(path) else { continue };
        if !root.is_dir() {
            continue;
        }
        site.node = match resolve_at(&root, &yard, &config, paths) {
            Ok((spec, source)) if uses_node(&source, root.join("package.json").is_file()) => {
                let (source, file) = setting(source);
                Some(SiteNode {
                    version: pick(&spec, &installed, &lts).map(|v| v.to_string()),
                    spec: Some(spec.to_string()),
                    source,
                    file: file.map(|f| f.display().to_string()),
                    error: None,
                })
            }
            Ok(_) => None,
            // The error names the file at fault; `source` has nothing better to say.
            Err(e) => Some(SiteNode {
                spec: None,
                source: SettingSource::Default,
                file: None,
                version: None,
                error: Some(e),
            }),
        };
    }
}

fn setting(source: NodeSource) -> (SettingSource, Option<PathBuf>) {
    match source {
        NodeSource::Project(f) => (SettingSource::ProjectFile, Some(f)),
        NodeSource::Managed(f) => (SettingSource::ManagedFile, Some(f)),
        NodeSource::Site(_) => (SettingSource::SitesToml, None),
        NodeSource::Nvmrc(f) => (SettingSource::Nvmrc, Some(f)),
        NodeSource::NodeVersionFile(f) => (SettingSource::NodeVersionFile, Some(f)),
        NodeSource::Default => (SettingSource::Default, None),
    }
}

/// The spec of a site whose `node` has one.
pub fn uses(site: &SiteInfo) -> Option<NodeSpec> {
    parse_spec(site.node.as_ref()?.spec.as_deref()?).ok()
}
