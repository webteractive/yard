//! What the `php`/`composer`/`pecl` shims and every PHP command Yard runs see (spec §4.5).
//! Herd puts its own `php` and `composer` on PATH, so nothing Yard starts may find them.

use crate::brew::Brew;
use crate::config::{SitesConfig, YardConfig};
use crate::{Paths, PhpVersion};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

/// The commands `bin/` holds, each a link to the one `yard-shim` binary.
pub const SHIMS: [&str; 5] = ["php", "composer", "pecl", "phpize", "php-config"];

/// Why the shim chose a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The isolation of the linked site containing the cwd.
    Site(String),
    /// The nearest `.yard-php`.
    File(PathBuf),
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Site(name) => write!(f, "site {name} is isolated to it"),
            Source::File(file) => write!(f, "{}", file.display()),
            Source::Default => write!(f, "the default"),
        }
    }
}

/// The version a shim runs in `cwd` (spec §4.5): the isolation of the innermost linked
/// site containing it, else the nearest `.yard-php`, else the default.
pub fn resolve_version(
    cwd: &Path,
    yard: &YardConfig,
    sites: &SitesConfig,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Result<(PhpVersion, Source), YardError> {
    let site = sites
        .sites
        .iter()
        .filter_map(|(name, e)| Some((name, e, e.path.as_deref()?)))
        .filter(|(_, _, path)| cwd.starts_with(path))
        .max_by_key(|(_, _, path)| path.components().count());
    if let Some((name, entry, _)) = site
        && let Some(v) = entry.php
    {
        return Ok((v, Source::Site(name.clone())));
    }
    for dir in cwd.ancestors() {
        let file = dir.join(".yard-php");
        if let Some(text) = read(&file) {
            let v: PhpVersion = text.trim().parse().map_err(|e: YardError| {
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
        let read = |p: &Path| files.get(p).cloned();
        let at = |cwd: &str| resolve_version(Path::new(cwd), &yard, &sites, &read).unwrap();
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
            let read = |p: &Path| (p == Path::new("/w/.yard-php")).then(|| bad.to_string());
            let e = resolve_version(Path::new("/w/x"), &yard, &sites, &read).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{bad}");
            assert!(e.message.contains("/w/.yard-php"), "{}", e.message);
        }
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
