//! What the `php`/`composer`/`pecl` shims and every PHP command Yard runs see (spec §4.5),
//! and which Node the `node`/`npm`/`npx` shims run (M4c spec §5–§6). Herd puts its own
//! `php`, `composer` and `node` on PATH, so nothing Yard starts may find them.

use crate::brew::Brew;
use crate::config::{SiteEntry, SitesConfig, YardConfig};
use crate::node::{DEFAULT_SPEC, NodeSpec, NodeVersion, parse_spec, spec_line};
use crate::{Paths, PhpVersion, project};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

pub const PHP_SHIMS: [&str; 5] = ["php", "composer", "pecl", "phpize", "php-config"];
pub const NODE_SHIMS: [&str; 3] = ["node", "npm", "npx"];
/// The shims that wake their site's sleeping services before they run (M4e spec §5.2):
/// what talks to a database. pecl, phpize, php-config and Node never do.
pub const WAKE_SHIMS: [&str; 2] = ["php", "composer"];
/// The commands `bin/` holds, each a link to the one `yard-shim` binary.
pub const SHIMS: [&str; 8] = ["php", "composer", "pecl", "phpize", "php-config", "node", "npm", "npx"];

/// Why the shim chose a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The nearest `.yard`: its `php`, or the default when it names none.
    Project(PathBuf),
    /// The managed `.yard` of the linked site containing the cwd: its `php`, or the
    /// default when it names none.
    Managed(PathBuf),
    /// The isolation of the linked site containing the cwd.
    Site(String),
    /// The nearest `.yard-php`.
    File(PathBuf),
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Project(file) | Source::Managed(file) => write!(f, "{}", file.display()),
            Source::Site(name) => write!(f, "site {name} is isolated to it"),
            Source::File(file) => write!(f, "{}", file.display()),
            Source::Default => write!(f, "the default"),
        }
    }
}

/// The version a shim runs in `cwd` (spec §4.5, project-file spec §5, managed-file spec
/// §4): the nearest `.yard` (its `php`, else the default), else the managed `.yard` of the
/// innermost linked site containing it while that file is for the site's path, else that
/// site's isolation, else the nearest `.yard-php`, else the default.
///
/// `read` gives a file's text, `None` when nothing is there, and an error for one that is
/// there but cannot be used (a FIFO, a dangling link): that stops the search, as it puts the
/// site on its setup page, rather than falling through to other settings. `canonical`
/// resolves symlinks, since `cwd` is the real path and `sites.toml` may spell one through a
/// link. When linked sites share the innermost path, the one whose managed file is for it
/// wins, else the last in `sites.toml`'s order, as isolation always took.
pub fn resolve_version(
    cwd: &Path,
    yard: &YardConfig,
    sites: &SitesConfig,
    paths: &Paths,
    read: &dyn Fn(&Path) -> Option<Result<String, YardError>>,
    canonical: &dyn Fn(&Path) -> PathBuf,
) -> Result<(PhpVersion, Source), YardError> {
    for dir in cwd.ancestors() {
        let file = dir.join(project::FILE);
        if let Some(text) = read(&file) {
            let php = project::parse(&text?, &file)?.php;
            return Ok((php.unwrap_or(yard.default_php), Source::Project(file)));
        }
    }
    let innermost = innermost(cwd, sites, canonical);
    for (name, _, path) in innermost.iter().rev() {
        let file = paths.managed_project_file(name);
        if let Some(text) = read(&file) {
            let (managed, project) =
                text.and_then(|text| project::parse_managed(&text, &file)).map_err(naming(&file))?;
            if project.as_deref() == Some(*path) {
                return Ok((managed.php.unwrap_or(yard.default_php), Source::Managed(file)));
            }
        }
    }
    if let Some((name, entry, _)) = innermost.last()
        && let Some(v) = entry.php
    {
        return Ok((v, Source::Site((*name).clone())));
    }
    for dir in cwd.ancestors() {
        let file = dir.join(".yard-php");
        if let Some(text) = read(&file) {
            let v: PhpVersion = text?.trim().parse().map_err(|e: YardError| {
                YardError::new(ErrorCode::InvalidArgument, format!("{}: {}", file.display(), e.message))
                    .with_hint(format!("put a version such as 8.4 in {}", file.display()))
            })?;
            return Ok((v, Source::File(file)));
        }
    }
    Ok((yard.default_php, Source::Default))
}

/// The linked site whose path holds `cwd` most closely: the last of [`innermost`].
pub fn site_at(cwd: &Path, sites: &SitesConfig, canonical: &dyn Fn(&Path) -> PathBuf) -> Option<String> {
    innermost(cwd, sites, canonical).last().map(|(name, ..)| (*name).clone())
}

/// The linked sites whose path holds `cwd` most closely, in `sites.toml`'s order. Several
/// only when they share that path.
fn innermost<'a>(
    cwd: &Path,
    sites: &'a SitesConfig,
    canonical: &dyn Fn(&Path) -> PathBuf,
) -> Vec<(&'a String, &'a SiteEntry, &'a Path)> {
    let containing: Vec<_> = sites
        .sites
        .iter()
        .filter(|(name, _)| crate::sites::is_site_name(name))
        .filter_map(|(name, e)| Some((name, e, e.path.as_deref()?)))
        .filter_map(|(name, e, path)| {
            let depth = canonical(path).components().count();
            (cwd.starts_with(canonical(path)) || cwd.starts_with(path)).then_some((name, e, path, depth))
        })
        .collect();
    let deepest = containing.iter().map(|(.., depth)| *depth).max();
    containing
        .into_iter()
        .filter(|(.., depth)| Some(*depth) == deepest)
        .map(|(name, e, path, _)| (name, e, path))
        .collect()
}

/// Why a Node shim chose a spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeSource {
    /// The nearest `.yard` that sets `node`.
    Project(PathBuf),
    /// The managed `.yard` of the linked site containing the cwd.
    Managed(PathBuf),
    /// `yard node use`, in `sites.toml`.
    Site(String),
    Nvmrc(PathBuf),
    NodeVersionFile(PathBuf),
    Default,
}

impl fmt::Display for NodeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeSource::Project(file)
            | NodeSource::Managed(file)
            | NodeSource::Nvmrc(file)
            | NodeSource::NodeVersionFile(file) => write!(f, "{}", file.display()),
            NodeSource::Site(name) => write!(f, "site {name} uses it"),
            NodeSource::Default => write!(f, "the default"),
        }
    }
}

/// The Node spec a shim runs in `cwd` (M4c spec §5): the nearest pin wins, walking up from
/// `cwd`. In one directory a `.yard` that sets `node` comes first, then, in the innermost
/// linked site's root, the site's own setting (its managed `.yard` when that is for the
/// site's path, else `sites.toml`), then `.nvmrc`, then `.node-version`; with none, the
/// default. A committed `.yard` at the site's root skips the site's setting even without
/// `node`, as it overrules both; a managed file for the site that has no `node` gives
/// nothing, so `sites.toml` is not consulted behind it.
///
/// `read` and `canonical` are [`resolve_version`]'s. A file that cannot be used stops the
/// search: a shim never falls through a pin it cannot meet.
pub fn resolve_node(
    cwd: &Path,
    yard: &YardConfig,
    sites: &SitesConfig,
    paths: &Paths,
    read: &dyn Fn(&Path) -> Option<Result<String, YardError>>,
    canonical: &dyn Fn(&Path) -> PathBuf,
) -> Result<(NodeSpec, NodeSource), YardError> {
    let innermost = innermost(cwd, sites, canonical);
    let mut site_root = innermost.last().map(|(.., path)| (*path, canonical(path)));
    for dir in cwd.ancestors() {
        let file = dir.join(project::FILE);
        if let Some(text) = read(&file)
            && let Some(spec) =
                project::parse(&text.map_err(naming(&file))?, &file).map_err(naming(&file))?.node
        {
            return Ok((spec, NodeSource::Project(file)));
        }
        if site_root.as_ref().is_some_and(|(path, real)| dir == *path || canonical(dir) == *real) {
            site_root = None;
            if let Some(found) = site_node(&innermost, paths, read)? {
                return Ok(found);
            }
        }
        for (name, source) in [
            (".nvmrc", NodeSource::Nvmrc as fn(PathBuf) -> NodeSource),
            (".node-version", NodeSource::NodeVersionFile),
        ] {
            let file = dir.join(name);
            if let Some(text) = read(&file) {
                let spec = text.and_then(|text| pin_file_spec(&text, &file)).map_err(naming(&file))?;
                return Ok((spec, source(file)));
            }
        }
    }
    // The walk never met the site's root (a path only its canonical form contains).
    if site_root.is_some()
        && let Some(found) = site_node(&innermost, paths, read)?
    {
        return Ok(found);
    }
    Ok((yard.default_node.clone().unwrap_or(DEFAULT_SPEC), NodeSource::Default))
}

/// The site's own setting in [`resolve_node`]: `None` when the innermost site sets nothing.
fn site_node(
    innermost: &[(&String, &SiteEntry, &Path)],
    paths: &Paths,
    read: &dyn Fn(&Path) -> Option<Result<String, YardError>>,
) -> Result<Option<(NodeSpec, NodeSource)>, YardError> {
    let Some((name, entry, path)) = innermost.last() else { return Ok(None) };
    if read(&path.join(project::FILE)).is_some() {
        return Ok(None);
    }
    for (name, _, path) in innermost.iter().rev() {
        let file = paths.managed_project_file(name);
        if let Some(text) = read(&file) {
            let (managed, project) = project::parse_managed(&text?, &file)?;
            if project.as_deref() == Some(*path) {
                return Ok(managed.node.map(|spec| (spec, NodeSource::Managed(file))));
            }
        }
    }
    Ok(entry.node.clone().map(|spec| (spec, NodeSource::Site((*name).clone()))))
}

/// `e` with `file` in its data, unless it has data already, so [`failed_pin`] can say which
/// pin [`resolve_node`] could not use.
fn naming(file: &Path) -> impl FnOnce(YardError) -> YardError + '_ {
    move |e| match e.data {
        Some(_) => e,
        None => e.with_data(serde_json::json!({ "file": file.display().to_string() })),
    }
}

/// The pin a [`resolve_node`] error is about, as the source it would have been.
pub fn failed_pin(e: &YardError, paths: &Paths) -> Option<NodeSource> {
    let file = PathBuf::from(e.data.as_ref()?.get("file")?.as_str()?);
    Some(match file.file_name()?.to_str()? {
        ".nvmrc" => NodeSource::Nvmrc(file),
        ".node-version" => NodeSource::NodeVersionFile(file),
        project::FILE if file.starts_with(paths.managed_sites_dir()) => NodeSource::Managed(file),
        project::FILE => NodeSource::Project(file),
        _ => return None,
    })
}

/// The spec in an `.nvmrc` or `.node-version`, `INVALID_ARGUMENT` naming the file when it
/// has none Yard accepts.
fn pin_file_spec(text: &str, file: &Path) -> Result<NodeSpec, YardError> {
    let message = match spec_line(text) {
        None => format!("{} names no version", file.display()),
        Some((line, spec)) => match parse_spec(spec) {
            Ok(spec) => return Ok(spec),
            Err(why) => format!("{} line {line}: {why}", file.display()),
        },
    };
    Err(YardError::new(ErrorCode::InvalidArgument, message)
        .with_hint(format!("put a version such as 22 in {}", file.display())))
}

/// Whether the site at `root` shows a Node part: a `package.json`, its own setting, or a pin
/// file in its root. A pin above the root, such as `~/.nvmrc`, still picks the version, but
/// alone does not make a PHP-only site a Node one.
pub fn uses_node(source: &NodeSource, root: &Path, has_package_json: bool) -> bool {
    has_package_json
        || match source {
            NodeSource::Site(_) | NodeSource::Managed(_) => true,
            NodeSource::Project(f) | NodeSource::Nvmrc(f) | NodeSource::NodeVersionFile(f) => {
                f.parent() == Some(root)
            }
            NodeSource::Default => false,
        }
}

/// What a shim `exec`s: the program, the arguments to put before the user's, and the
/// environment.
pub type ShimCommand = (PathBuf, Vec<OsString>, Vec<(String, String)>);

/// The command for shim `name`. `opt` is the version's keg; `inherited` is the caller's PATH.
pub fn command(
    name: &str,
    v: PhpVersion,
    opt: &Path,
    brew: &Brew,
    paths: &Paths,
    inherited: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<ShimCommand, YardError> {
    let (program, pre) = match name {
        "php" | "pecl" | "phpize" | "php-config" => (Brew::php_bin(opt, name), vec![]),
        "composer" => {
            let phar = brew.prefix.join("bin/composer");
            if !exists(&phar) {
                return Err(YardError::new(ErrorCode::FormulaMissing, "composer is not installed")
                    .with_hint("brew install composer"));
            }
            (Brew::php_bin(opt, "php"), vec![phar.into_os_string()])
        }
        other => {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!("yard-shim has no {other} command"),
            )
            .with_hint(format!("run it as one of {}", SHIMS.join(", "))));
        }
    };
    let mut env = vec![];
    if exists(&paths.php_ini(v)) {
        env.push(("PHPRC".to_string(), paths.php_dir(v).display().to_string()));
        env.push(("PHP_INI_SCAN_DIR".to_string(), paths.php_conf_d(v).display().to_string()));
    }
    // Yard's bin right after the keg, so a composer script's own `php` or `composer`
    // reaches a shim again.
    let keg_bin = opt.join("bin");
    let bin = paths.bin_dir();
    let mut entries: Vec<PathBuf> =
        std::env::split_paths(&child_path(Some(&keg_bin), inherited)).filter(|p| *p != bin).collect();
    entries.insert(1.min(entries.len()), bin);
    let path = std::env::join_paths(entries).unwrap_or_default();
    env.push(("PATH".to_string(), path.to_string_lossy().into_owned()));
    Ok((program, pre, env))
}

/// The command for Node shim `name` running `v`: the version's own binary, with its `bin`
/// first on PATH (npm scripts call `node` by name), then Yard's, then `inherited` without
/// Herd. `NODE_EXTRA_CA_CERTS` points at Yard's CA so `https://*.yard` works from Node,
/// unless the user set their own (`extra_ca`, the inherited value; empty counts as unset).
pub fn node_command(
    name: &str,
    v: NodeVersion,
    paths: &Paths,
    inherited: &OsStr,
    extra_ca: Option<&OsStr>,
    exists: &dyn Fn(&Path) -> bool,
) -> ShimCommand {
    let program = paths.node_bin(v, name);
    let version_bin = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let bin = paths.bin_dir();
    let without_herd = child_path(None, inherited);
    let rest: Vec<PathBuf> =
        std::env::split_paths(&without_herd).filter(|p| *p != version_bin && *p != bin).collect();
    let entries: Vec<PathBuf> = [version_bin, bin].into_iter().chain(rest).collect();
    let path = std::env::join_paths(entries).unwrap_or_default();
    let mut env = vec![("PATH".to_string(), path.to_string_lossy().into_owned())];
    let ca = paths.ca_cert();
    if extra_ca.is_none_or(OsStr::is_empty) && exists(&ca) {
        env.push(("NODE_EXTRA_CA_CERTS".to_string(), ca.display().to_string()));
    }
    (program, vec![], env)
}

/// The PATH for a child: `keg_bin` first, then `inherited` without Herd's directories
/// (any entry with a `Herd` component), empty entries or a second `keg_bin`.
pub fn child_path(keg_bin: Option<&Path>, inherited: &OsStr) -> OsString {
    let herd = |p: &Path| p.components().any(|c| c == Component::Normal(OsStr::new("Herd")));
    let rest = std::env::split_paths(inherited)
        .filter(|p| !p.as_os_str().is_empty() && !herd(p) && Some(p.as_path()) != keg_bin);
    let all: Vec<PathBuf> = keg_bin.map(Path::to_path_buf).into_iter().chain(rest).collect();
    std::env::join_paths(all).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brew::Brew;
    use crate::config::sites::parse_sites;
    use crate::config::yard::parse_yard;
    use crate::node::NodeSpec;
    use crate::{Paths, PhpVersion};
    use std::collections::BTreeMap;
    use yard_proto::{ErrorCode, YardError};

    /// No symlinks: every path is its own canonical form.
    fn same(p: &Path) -> PathBuf {
        p.to_path_buf()
    }

    fn v(s: &str) -> PhpVersion {
        s.parse().unwrap()
    }

    fn configs(sites: &str) -> (crate::config::YardConfig, crate::config::SitesConfig) {
        let (yard, _) = parse_yard(Some("default_php = \"8.4\"\n"), Path::new("/r/yard.toml")).unwrap();
        (yard, parse_sites(Some(sites), Path::new("/r/sites.toml")).unwrap())
    }

    const SITES: &str = r#"
[sites.old]
path = "/code/old"
php = "7.1"
[sites.plain]
path = "/code/plain"
[sites.inner]
path = "/code/old/vendor/inner"
php = "7.4"
"#;

    #[test]
    fn resolution_order() {
        let (yard, sites) = configs(SITES);
        let files = BTreeMap::from([(PathBuf::from("/code/.yard-php"), "7.3\n".to_string())]);
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        let at = |cwd: &str| {
            resolve_version(Path::new(cwd), &yard, &sites, &Paths::new("/r"), &read, &same).unwrap()
        };
        assert_eq!(at("/code/old/app"), (v("7.1"), Source::Site("old".into())));
        assert_eq!(at("/code/old/vendor/inner/src"), (v("7.4"), Source::Site("inner".into())));
        assert_eq!(at("/code/plain/src"), (v("7.3"), Source::File("/code/.yard-php".into())));
        assert_eq!(at("/elsewhere"), (v("8.4"), Source::Default));
        // A path that only shares a prefix is not inside the site.
        assert_eq!(at("/code/older").1, Source::File("/code/.yard-php".into()));
    }

    #[test]
    fn yard_php_file_errors_name_the_file() {
        let (yard, sites) = configs("");
        for bad in ["5.6", "banana"] {
            let read = |p: &Path| (p == Path::new("/w/.yard-php")).then(|| Ok(bad.to_string()));
            let e = resolve_version(Path::new("/w/x"), &yard, &sites, &Paths::new("/r"), &read, &same)
                .unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{bad}");
            assert!(e.message.contains("/w/.yard-php"), "{}", e.message);
        }
    }

    #[test]
    fn project_file_beats_isolation_and_yard_php() {
        let (yard, sites) = configs(SITES);
        let files = BTreeMap::from([
            (PathBuf::from("/code/.yard-php"), "7.2\n".to_string()),
            (PathBuf::from("/code/old/.yard"), "php = \"7.3\"\n".to_string()),
        ]);
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        let got = resolve_version(Path::new("/code/old/app"), &yard, &sites, &Paths::new("/r"), &read, &same)
            .unwrap();
        assert_eq!(got, (v("7.3"), Source::Project("/code/old/.yard".into())));
        assert_eq!(got.1.to_string(), "/code/old/.yard");
    }

    #[test]
    fn project_file_without_php_is_the_default() {
        let (yard, sites) = configs(SITES);
        let files = BTreeMap::from([(PathBuf::from("/code/old/.yard"), "docroot = \"web\"\n".to_string())]);
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        let got = resolve_version(Path::new("/code/old/app"), &yard, &sites, &Paths::new("/r"), &read, &same)
            .unwrap();
        assert_eq!(got, (v("8.4"), Source::Project("/code/old/.yard".into())));
    }

    #[test]
    fn nearest_project_file_wins() {
        let (yard, sites) = configs("");
        let files = BTreeMap::from([
            (PathBuf::from("/code/a/.yard"), "php = \"7.3\"\n".to_string()),
            (PathBuf::from("/code/a/sub/.yard"), "php = \"7.4\"\n".to_string()),
        ]);
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        let got = resolve_version(Path::new("/code/a/sub/x"), &yard, &sites, &Paths::new("/r"), &read, &same)
            .unwrap();
        assert_eq!(got, (v("7.4"), Source::Project("/code/a/sub/.yard".into())));
    }

    #[test]
    fn managed_file_sits_between_committed_and_isolation() {
        let (yard, sites) = configs(SITES);
        let managed = |project: &str| format!("php = \"7.3\"\nproject = {project:?}\n");
        let mut files = BTreeMap::from([(PathBuf::from("/r/sites/old/.yard"), managed("/code/old"))]);
        let at = |files: &BTreeMap<PathBuf, String>, cwd: &str| {
            let read = |p: &Path| files.get(p).cloned().map(Ok);
            resolve_version(Path::new(cwd), &yard, &sites, &Paths::new("/r"), &read, &same).unwrap()
        };
        let got = at(&files, "/code/old/app");
        assert_eq!(got, (v("7.3"), Source::Managed("/r/sites/old/.yard".into())));
        assert_eq!(got.1.to_string(), "/r/sites/old/.yard");
        // The innermost site's file: `inner` has none, so its isolation.
        assert_eq!(at(&files, "/code/old/vendor/inner/src"), (v("7.4"), Source::Site("inner".into())));
        // A committed .yard nearer the cwd wins.
        files.insert(PathBuf::from("/code/old/app/.yard"), "php = \"7.4\"\n".to_string());
        assert_eq!(at(&files, "/code/old/app"), (v("7.4"), Source::Project("/code/old/app/.yard".into())));
        files.remove(Path::new("/code/old/app/.yard"));
        // A file for another directory is not used: the site's isolation is.
        files.insert(PathBuf::from("/r/sites/old/.yard"), managed("/code/elsewhere"));
        assert_eq!(at(&files, "/code/old/app"), (v("7.1"), Source::Site("old".into())));
        // One without php is the default.
        files.insert(PathBuf::from("/r/sites/old/.yard"), "project = \"/code/old\"\n".to_string());
        assert_eq!(at(&files, "/code/old/app"), (v("8.4"), Source::Managed("/r/sites/old/.yard".into())));
    }

    const NODE_SITES: &str = r#"
[sites.old]
path = "/code/old"
node = "18"
[sites.plain]
path = "/code/plain"
"#;

    fn spec(s: &str) -> NodeSpec {
        crate::node::parse_spec(s).unwrap()
    }

    fn node_at(
        files: &BTreeMap<PathBuf, String>,
        yard: &crate::config::YardConfig,
        sites: &crate::config::SitesConfig,
        cwd: &str,
    ) -> Result<(NodeSpec, NodeSource), YardError> {
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        resolve_node(Path::new(cwd), yard, sites, &Paths::new("/r"), &read, &same)
    }

    #[test]
    fn node_resolution_order() {
        let (yard, sites) = configs(NODE_SITES);
        let mut files = BTreeMap::from([
            (PathBuf::from("/code/.nvmrc"), "16.14.0\n".to_string()),
            (PathBuf::from("/code/old/.yard"), "php = \"7.1\"\n".to_string()),
            (PathBuf::from("/r/sites/plain/.yard"), "project = \"/code/plain\"\nnode = \"20\"\n".to_string()),
        ]);
        // old's .yard has no node, so step 1 passes over it; that the file exists at all
        // skips old's sites.toml setting, which the committed file overrules.
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/old/app").unwrap(),
            (spec("16.14.0"), NodeSource::Nvmrc("/code/.nvmrc".into()))
        );
        files.remove(Path::new("/code/old/.yard"));
        let got = node_at(&files, &yard, &sites, "/code/old/app").unwrap();
        assert_eq!(got, (spec("18"), NodeSource::Site("old".into())));
        assert_eq!(got.1.to_string(), "site old uses it");
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/plain/src").unwrap(),
            (spec("20"), NodeSource::Managed("/r/sites/plain/.yard".into()))
        );
        let got = node_at(&files, &yard, &sites, "/elsewhere").unwrap();
        assert_eq!(got, (spec("22"), NodeSource::Default));
        assert_eq!(got.1.to_string(), "the default");
        let (yard24, _) = parse_yard(Some("default_node = \"24\"\n"), Path::new("/r/yard.toml")).unwrap();
        assert_eq!(node_at(&files, &yard24, &sites, "/elsewhere").unwrap().0, spec("24"));
    }

    #[test]
    fn a_managed_file_without_node_hides_sites_toml() {
        let (yard, sites) = configs(NODE_SITES);
        let files = BTreeMap::from([
            (PathBuf::from("/code/.nvmrc"), "16.14.0\n".to_string()),
            (PathBuf::from("/r/sites/old/.yard"), "php = \"7.1\"\nproject = \"/code/old\"\n".to_string()),
        ]);
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/old/app").unwrap(),
            (spec("16.14.0"), NodeSource::Nvmrc("/code/.nvmrc".into()))
        );
    }

    #[test]
    fn a_yard_with_node_beats_everything() {
        let (yard, sites) = configs(NODE_SITES);
        let files = BTreeMap::from([
            (PathBuf::from("/code/.nvmrc"), "16.14.0\n".to_string()),
            (PathBuf::from("/code/old/.yard"), "node = \"25\"\n".to_string()),
        ]);
        let got = node_at(&files, &yard, &sites, "/code/old/app").unwrap();
        assert_eq!(got, (spec("25"), NodeSource::Project("/code/old/.yard".into())));
        assert_eq!(got.1.to_string(), "/code/old/.yard");
    }

    #[test]
    fn nvmrc_beats_node_version_in_one_dir_and_nearer_wins() {
        let (yard, sites) = configs("");
        let mut files = BTreeMap::from([
            (PathBuf::from("/code/a/.nvmrc"), "20\n".to_string()),
            (PathBuf::from("/code/a/.node-version"), "18\n".to_string()),
        ]);
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/a/x").unwrap(),
            (spec("20"), NodeSource::Nvmrc("/code/a/.nvmrc".into()))
        );
        files.insert(PathBuf::from("/code/a/sub/.node-version"), "24\n".to_string());
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/a/sub/x").unwrap(),
            (spec("24"), NodeSource::NodeVersionFile("/code/a/sub/.node-version".into()))
        );
    }

    #[test]
    fn an_unusable_nvmrc_stops_the_search() {
        let (yard, sites) = configs("");
        let with = |text: &str| {
            BTreeMap::from([
                (PathBuf::from("/code/.nvmrc"), text.to_string()),
                (PathBuf::from("/.node-version"), "20\n".to_string()),
            ])
        };
        assert_eq!(node_at(&with("lts/*\n"), &yard, &sites, "/code/x").unwrap().0, NodeSpec::LtsLatest);
        for bad in ["node\n", "# x\n"] {
            let e = node_at(&with(bad), &yard, &sites, "/code/x").unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{bad}");
            assert!(e.message.contains("/code/.nvmrc"), "{}", e.message);
            assert_eq!(e.hint.as_deref(), Some("put a version such as 22 in /code/.nvmrc"));
            assert_eq!(failed_pin(&e, &Paths::new("/r")), Some(NodeSource::Nvmrc("/code/.nvmrc".into())));
        }
        let files = with("20\n");
        let read = |p: &Path| {
            if p == Path::new("/code/.nvmrc") {
                Some(Err(YardError::new(ErrorCode::InvalidArgument, "/code/.nvmrc is a FIFO")))
            } else {
                files.get(p).cloned().map(Ok)
            }
        };
        let e =
            resolve_node(Path::new("/code/x"), &yard, &sites, &Paths::new("/r"), &read, &same).unwrap_err();
        assert_eq!(e.message, "/code/.nvmrc is a FIFO");
        assert_eq!(failed_pin(&e, &Paths::new("/r")), Some(NodeSource::Nvmrc("/code/.nvmrc".into())));
    }

    #[test]
    fn the_nearest_pin_wins_and_in_one_dir_yard_then_site_then_nvmrc() {
        let (yard, sites) = configs(NODE_SITES);
        let mut files = BTreeMap::from([
            (PathBuf::from("/code/old/.nvmrc"), "16\n".to_string()),
            (PathBuf::from("/code/old/web/.nvmrc"), "20\n".to_string()),
        ]);
        let web = (spec("20"), NodeSource::Nvmrc("/code/old/web/.nvmrc".into()));
        // In the site's root its own setting beats the .nvmrc beside it, and a subdirectory's
        // own .nvmrc is nearer than both.
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/old/x").unwrap(),
            (spec("18"), NodeSource::Site("old".into()))
        );
        assert_eq!(node_at(&files, &yard, &sites, "/code/old/web/x").unwrap(), web);
        files.insert(PathBuf::from("/code/old/.yard"), "node = \"25\"\n".to_string());
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/old/x").unwrap(),
            (spec("25"), NodeSource::Project("/code/old/.yard".into()))
        );
        assert_eq!(node_at(&files, &yard, &sites, "/code/old/web/x").unwrap(), web);
        // A .yard above the site is farther than the site's setting.
        files.remove(Path::new("/code/old/.yard"));
        files.remove(Path::new("/code/old/web/.nvmrc"));
        files.insert(PathBuf::from("/code/.yard"), "node = \"24\"\n".to_string());
        assert_eq!(
            node_at(&files, &yard, &sites, "/code/old/web/x").unwrap(),
            (spec("18"), NodeSource::Site("old".into()))
        );
    }

    #[test]
    fn uses_node_table() {
        let root = Path::new("/p");
        assert!(!uses_node(&NodeSource::Default, root, false));
        assert!(uses_node(&NodeSource::Default, root, true));
        assert!(uses_node(&NodeSource::Nvmrc("/p/.nvmrc".into()), root, false));
        assert!(uses_node(&NodeSource::Site("p".into()), root, false));
        assert!(uses_node(&NodeSource::Managed("/r/sites/p/.yard".into()), root, false));
        // A pin above the root picks the version, but alone does not make a Node site.
        assert!(!uses_node(&NodeSource::Nvmrc("/.nvmrc".into()), root, false));
        assert!(!uses_node(&NodeSource::Project("/.yard".into()), root, false));
        assert!(uses_node(&NodeSource::Nvmrc("/.nvmrc".into()), root, true));
    }

    #[test]
    fn node_command_puts_the_version_then_yard_first_and_drops_herd() {
        let paths = Paths::new("/r");
        let v16: crate::node::NodeVersion = "16.14.0".parse().unwrap();
        let inherited = OsStr::new("/Users/x/Library/Application Support/Herd/bin:/usr/bin");
        let ca = |p: &Path| p == Path::new("/r/ca/yard-ca.pem");
        let env_of = |extra: Option<&OsStr>, exists: &dyn Fn(&Path) -> bool| {
            let (program, pre, env) = node_command("npm", v16, &paths, inherited, extra, exists);
            assert_eq!(program, Path::new("/r/node/node-versions/v16.14.0/installation/bin/npm"));
            assert!(pre.is_empty());
            env.into_iter().collect::<BTreeMap<String, String>>()
        };
        let env = env_of(None, &ca);
        assert_eq!(env["PATH"], "/r/node/node-versions/v16.14.0/installation/bin:/r/bin:/usr/bin");
        assert_eq!(env["NODE_EXTRA_CA_CERTS"], "/r/ca/yard-ca.pem");
        assert!(!env_of(Some(OsStr::new("/mine.pem")), &ca).contains_key("NODE_EXTRA_CA_CERTS"));
        assert_eq!(env_of(Some(OsStr::new("")), &ca)["NODE_EXTRA_CA_CERTS"], "/r/ca/yard-ca.pem");
        assert!(!env_of(None, &|_| false).contains_key("NODE_EXTRA_CA_CERTS"));
    }

    #[test]
    fn site_at_is_the_innermost_site() {
        let (_, sites) =
            configs("[sites.old]\npath = \"/code/old\"\n[sites.api]\npath = \"/code/old/api\"\n");
        assert_eq!(site_at(Path::new("/code/old/api/src"), &sites, &same).as_deref(), Some("api"));
        assert_eq!(site_at(Path::new("/code/old/x"), &sites, &same).as_deref(), Some("old"));
        assert_eq!(site_at(Path::new("/elsewhere"), &sites, &same), None);
    }

    #[test]
    fn wake_shims_are_php_shims() {
        assert!(WAKE_SHIMS.iter().all(|s| PHP_SHIMS.contains(s)));
    }

    #[test]
    fn shims_are_php_s_then_node_s() {
        assert_eq!(SHIMS[..5], PHP_SHIMS);
        assert_eq!(SHIMS[5..], NODE_SHIMS);
    }

    #[test]
    fn bad_managed_file_names_it() {
        let (yard, sites) = configs(SITES);
        let read = |p: &Path| (p == Path::new("/r/sites/old/.yard")).then(|| Ok("php = 7.3\n".to_string()));
        let e = resolve_version(Path::new("/code/old/app"), &yard, &sites, &Paths::new("/r"), &read, &same)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert!(e.message.contains("/r/sites/old/.yard"), "{}", e.message);
    }

    #[test]
    fn an_unusable_file_stops_the_search() {
        let (yard, sites) = configs(SITES);
        let unusable = |file: &str| {
            let file = file.to_string();
            move |p: &Path| {
                (p == Path::new(&file))
                    .then(|| Err(YardError::new(ErrorCode::ProjectFileInvalid, format!("{file} is a FIFO"))))
            }
        };
        for file in ["/code/old/app/.yard", "/r/sites/old/.yard"] {
            let read = unusable(file);
            let e =
                resolve_version(Path::new("/code/old/app/x"), &yard, &sites, &Paths::new("/r"), &read, &same)
                    .unwrap_err();
            assert_eq!(e.message, format!("{file} is a FIFO"));
        }
    }

    #[test]
    fn sites_sharing_a_path_use_the_matching_managed_file() {
        let (yard, sites) =
            configs("[sites.ee]\npath = \"/code/ee\"\n[sites.ee-2]\npath = \"/code/ee\"\nphp = \"7.1\"\n");
        let files = BTreeMap::from([(
            PathBuf::from("/r/sites/ee/.yard"),
            "php = \"7.3\"\nproject = \"/code/ee\"\n".to_string(),
        )]);
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        let got = resolve_version(Path::new("/code/ee/app"), &yard, &sites, &Paths::new("/r"), &read, &same)
            .unwrap();
        assert_eq!(got, (v("7.3"), Source::Managed("/r/sites/ee/.yard".into())));
    }

    #[test]
    fn a_site_path_spelled_through_a_symlink_holds_the_real_cwd() {
        let (yard, sites) = configs("[sites.old]\npath = \"/link/old\"\nphp = \"7.1\"\n");
        let canonical = |p: &Path| match p.strip_prefix("/link") {
            Ok(rest) => Path::new("/code").join(rest),
            Err(_) => p.to_path_buf(),
        };
        let mut files = BTreeMap::new();
        let at = |files: &BTreeMap<PathBuf, String>| {
            let read = |p: &Path| files.get(p).cloned().map(Ok);
            resolve_version(Path::new("/code/old/app"), &yard, &sites, &Paths::new("/r"), &read, &canonical)
                .unwrap()
        };
        assert_eq!(at(&files), (v("7.1"), Source::Site("old".into())));
        files.insert(
            PathBuf::from("/r/sites/old/.yard"),
            "php = \"7.3\"\nproject = \"/link/old\"\n".to_string(),
        );
        assert_eq!(at(&files), (v("7.3"), Source::Managed("/r/sites/old/.yard".into())));
    }

    #[test]
    fn a_name_that_is_not_a_site_name_reads_no_managed_file() {
        let (yard, sites) = configs("[sites.\"/etc\"]\npath = \"/code/x\"\n");
        let read = |p: &Path| {
            (p == Path::new("/etc/.yard")).then(|| Ok("php = \"7.3\"\nproject = \"/code/x\"\n".to_string()))
        };
        let got = resolve_version(Path::new("/code/x/app"), &yard, &sites, &Paths::new("/r"), &read, &same)
            .unwrap();
        assert_eq!(got, (v("8.4"), Source::Default));
    }

    #[test]
    fn bad_project_file_names_it() {
        let (yard, sites) = configs("");
        let read = |p: &Path| (p == Path::new("/w/.yard")).then(|| Ok("php = 7.3\n".to_string()));
        let e =
            resolve_version(Path::new("/w/x"), &yard, &sites, &Paths::new("/r"), &read, &same).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert!(e.message.contains("/w/.yard"), "{}", e.message);
    }

    fn env_of(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn composer_runs_the_phar_under_php() {
        let (paths, brew) = (Paths::new("/r"), Brew::new("/b"));
        let opt = Path::new("/b/opt/php@7.1");
        let exists = |p: &Path| p == Path::new("/b/bin/composer");
        let (program, pre, _) =
            command("composer", v("7.1"), opt, &brew, &paths, "/usr/bin".as_ref(), &exists).unwrap();
        assert_eq!(program, Path::new("/b/opt/php@7.1/bin/php"));
        assert_eq!(pre, [OsString::from("/b/bin/composer")]);

        let none = |_: &Path| false;
        let e = command("composer", v("7.1"), opt, &brew, &paths, "/usr/bin".as_ref(), &none).unwrap_err();
        assert_eq!(e.code, ErrorCode::FormulaMissing);
        assert_eq!(e.hint.as_deref(), Some("brew install composer"));

        let (program, pre, env) =
            command("pecl", v("7.1"), opt, &brew, &paths, "/x/Herd/bin:/usr/bin".as_ref(), &none).unwrap();
        assert_eq!(program, Path::new("/b/opt/php@7.1/bin/pecl"));
        assert!(pre.is_empty());
        assert_eq!(env_of(&env, "PATH").as_deref(), Some("/b/opt/php@7.1/bin:/r/bin:/usr/bin"));
        assert!(command("perl", v("7.1"), opt, &brew, &paths, "".as_ref(), &none).is_err());
    }

    #[test]
    fn ini_env_only_when_rendered() {
        let (paths, brew) = (Paths::new("/r"), Brew::new("/b"));
        let opt = Path::new("/b/opt/php@7.1");
        let none = |_: &Path| false;
        let (_, _, env) = command("php", v("7.1"), opt, &brew, &paths, "".as_ref(), &none).unwrap();
        assert_eq!(env_of(&env, "PHPRC"), None);
        let ini = paths.php_ini(v("7.1"));
        let rendered = |p: &Path| p == ini;
        let (_, _, env) = command("php", v("7.1"), opt, &brew, &paths, "".as_ref(), &rendered).unwrap();
        assert_eq!(env_of(&env, "PHPRC").as_deref(), Some("/r/php/7.1"));
        assert_eq!(env_of(&env, "PHP_INI_SCAN_DIR").as_deref(), Some("/r/php/7.1/conf.d"));
    }

    #[test]
    fn child_path_drops_herd_entries() {
        let inherited = "/Users/g/Library/Application Support/Herd/bin/:/opt/homebrew/bin:/usr/bin";
        let keg = Path::new("/opt/homebrew/opt/php@7.1/bin");
        assert_eq!(
            child_path(Some(keg), inherited.as_ref()),
            OsString::from("/opt/homebrew/opt/php@7.1/bin:/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn child_path_without_a_keg_only_filters() {
        let inherited = "/usr/bin::/x/Herd:/opt/php@7.1/bin";
        assert_eq!(child_path(None, inherited.as_ref()), OsString::from("/usr/bin:/opt/php@7.1/bin"));
        let keg = Path::new("/opt/php@7.1/bin");
        assert_eq!(child_path(Some(keg), inherited.as_ref()), OsString::from("/opt/php@7.1/bin:/usr/bin"));
    }

    #[test]
    fn herd_in_a_longer_name_is_kept() {
        assert_eq!(child_path(None, "/opt/Herdsman/bin".as_ref()), OsString::from("/opt/Herdsman/bin"));
    }
}
