//! What the `php`/`composer`/`pecl` shims and every PHP command Yard runs see (spec §4.5).
//! Herd puts its own `php` and `composer` on PATH, so nothing Yard starts may find them.

use crate::brew::Brew;
use crate::config::{SitesConfig, YardConfig};
use crate::{Paths, PhpVersion, project};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

/// The commands `bin/` holds, each a link to the one `yard-shim` binary.
pub const SHIMS: [&str; 5] = ["php", "composer", "pecl", "phpize", "php-config"];

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
    let innermost: Vec<_> = containing.iter().filter(|(.., depth)| Some(*depth) == deepest).collect();
    for (name, _, path, _) in innermost.iter().rev() {
        let file = paths.managed_project_file(name);
        if let Some(text) = read(&file) {
            let (managed, project) = project::parse_managed(&text?, &file)?;
            if project.as_deref() == Some(*path) {
                return Ok((managed.php.unwrap_or(yard.default_php), Source::Managed(file)));
            }
        }
    }
    if let Some((name, entry, ..)) = innermost.last()
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
    use crate::{Paths, PhpVersion};
    use std::collections::BTreeMap;
    use yard_proto::ErrorCode;

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
