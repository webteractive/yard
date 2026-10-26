//! A site's Node, read from the files on disk on every request (M4c spec §5–§7). Nothing
//! here runs fnm: only the `node.install` and `node.uninstall` jobs and
//! `node.list --available` do.

use crate::project::read_project_file;
use crate::sources::BREW_TIMEOUT;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::{SitesConfig, YardConfig, load_sites, load_yard};
use yard_core::node::{
    DEFAULT_SPEC, LtsMap, NodeSpec, NodeVersion, lts_from, matches, newest_per_line, parse_ls_remote,
    parse_spec, pick,
};
use yard_core::shim::{NodeSource, child_path, resolve_node, uses_node};
use yard_proto::{ErrorCode, NodeRelease, SettingSource, SiteInfo, SiteKind, SiteNode, YardError};

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
/// `package.json`, its own setting, or a pin file in its root ([`uses_node`]). A pin that cannot be read is
/// the site's `node.error`, except a `.yard` that does not parse when the site already
/// reports it or has no `package.json`. Nothing is filled while `yard.toml` or `sites.toml` does not
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
        let has_package_json = root.join("package.json").is_file();
        site.node = match resolve_at(&root, &yard, &config, paths) {
            Ok((spec, source)) if uses_node(&source, &root, has_package_json) => {
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
            // A `.yard` that does not parse is the site's `project_error` already, and says
            // nothing about Node to a site without a `package.json`.
            Err(e)
                if e.code == ErrorCode::ProjectFileInvalid
                    && (site.project_error.is_some() || !has_package_json) =>
            {
                None
            }
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

/// What fnm runs with: Yard's `node/` as its directory, and no colour or progress noise.
pub fn fnm_env(paths: &Paths) -> Vec<(String, String)> {
    vec![
        ("FNM_DIR".to_string(), paths.node_dir().display().to_string()),
        ("FNM_LOGLEVEL".to_string(), "info".to_string()),
    ]
}

/// Records the LTS codenames a release listing named, for shims to resolve `lts/<name>`.
pub fn write_lts(paths: &Paths, map: &LtsMap) -> std::io::Result<()> {
    yard_core::generated::write_atomic(&paths.lts_toml(), &map.render())
}

/// `node.list --available`: the newest release of each line on nodejs.org, from
/// `fnm ls-remote`, run outside a job since it only reads. Refreshes `lts.toml`.
pub async fn available(
    brew: &Brew,
    paths: &Paths,
    major: Option<u32>,
) -> Result<Vec<NodeRelease>, YardError> {
    let fnm = brew.fnm_bin();
    if !fnm.exists() {
        return Err(YardError::new(ErrorCode::FormulaMissing, "fnm is not installed")
            .with_hint("brew install fnm, or yard node install 22, which installs it"));
    }
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut cmd = tokio::process::Command::new(&fnm);
    cmd.arg("ls-remote")
        .envs(fnm_env(paths))
        .env("PATH", child_path(None, &inherited))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let out = tokio::time::timeout(BREW_TIMEOUT, cmd.output())
        .await
        .map_err(|_| YardError::new(ErrorCode::JobFailed, "fnm ls-remote did not answer within 30 s"))?
        .map_err(|e| YardError::new(ErrorCode::JobFailed, format!("cannot run {}: {e}", fnm.display())))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
        return Err(YardError::new(ErrorCode::JobFailed, format!("fnm ls-remote failed: {tail}"))
            .with_hint("check the network; nodejs.org lists the releases"));
    }
    let releases = parse_ls_remote(&String::from_utf8_lossy(&out.stdout));
    if let Err(e) = write_lts(paths, &lts_from(&releases)) {
        tracing::warn!("cannot write {}: {e}", paths.lts_toml().display());
    }
    Ok(newest_per_line(&releases, major)
        .into_iter()
        .map(|r| NodeRelease { version: r.version.to_string(), lts: r.lts })
        .collect())
}

/// `NODE_IN_USE` when `v` is the only installed version the default spec, or the spec of a
/// linked site that uses Node, matches: removing it would leave them nothing to run. `sites`
/// is the snapshot's; their Node part is filled here.
pub fn in_use(v: NodeVersion, mut sites: Vec<SiteInfo>, paths: &Paths) -> Result<(), YardError> {
    fill_node(&mut sites, paths);
    let (installed, lts) = (installed(paths), lts(paths));
    let only_v = |spec: &NodeSpec| {
        let matching: Vec<&NodeVersion> = installed.iter().filter(|i| matches(spec, **i, &lts)).collect();
        matching == [&v]
    };
    let default = load_yard(paths)?.0.default_node.unwrap_or(DEFAULT_SPEC);
    if only_v(&default) {
        let hint = match installed.iter().rev().find(|o| **o != v) {
            Some(other) => format!("yard node default {other}"),
            None => "yard node list --available".to_string(),
        };
        return Err(YardError::new(
            ErrorCode::NodeInUse,
            format!("Node {v} is the only Node the default ({default}) runs"),
        )
        .with_hint(hint));
    }
    let using: Vec<&SiteInfo> = sites.iter().filter(|s| uses(s).is_some_and(|spec| only_v(&spec))).collect();
    let Some(first) = using.first() else { return Ok(()) };
    let names: Vec<&str> = using.iter().map(|s| s.name.as_str()).collect();
    let node = first.node.as_ref();
    let hint = match (node.map(|n| n.source), node.and_then(|n| n.file.as_deref())) {
        (Some(SettingSource::SitesToml | SettingSource::ManagedFile), _) | (_, None) => {
            format!("yard node unuse --site {}", first.name)
        }
        (_, Some(file)) => format!("edit {file}"),
    };
    Err(YardError::new(ErrorCode::NodeInUse, format!("Node {v} runs {}", names.join(", "))).with_hint(hint))
}
