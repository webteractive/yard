//! Where Homebrew keeps what Yard runs. Yard reads these paths and never writes under them.

use crate::PhpVersion;
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

pub const DEFAULT_PREFIX: &str = "/opt/homebrew";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brew {
    pub prefix: PathBuf,
}

impl Brew {
    pub fn new(prefix: impl Into<PathBuf>) -> Self {
        Self { prefix: prefix.into() }
    }

    /// `YARD_BREW_PREFIX` if set, which tests use to point Yard at a fake brew, else
    /// [`Brew::detect`].
    pub fn from_env(exists: &dyn Fn(&Path) -> bool) -> Result<Brew, YardError> {
        Self::from_var(std::env::var_os("YARD_BREW_PREFIX").map(PathBuf::from), exists)
    }

    pub fn from_var(prefix: Option<PathBuf>, exists: &dyn Fn(&Path) -> bool) -> Result<Brew, YardError> {
        match prefix {
            Some(prefix) => Ok(Brew::new(prefix)),
            None => Self::detect(exists),
        }
    }

    pub fn detect(exists: &dyn Fn(&Path) -> bool) -> Result<Brew, YardError> {
        let brew = Brew::new(DEFAULT_PREFIX);
        if exists(&brew.brew_bin()) {
            Ok(brew)
        } else {
            Err(YardError::new(ErrorCode::BrewMissing, "Homebrew is not installed at /opt/homebrew")
                .with_hint("install Homebrew from https://brew.sh"))
        }
    }

    /// The environment every `brew` job runs with: no sweep of the formulae that depend on
    /// the one being installed or removed, which can rebuild unrelated ones for minutes.
    pub fn job_env() -> Vec<(String, String)> {
        vec![("HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK".into(), "1".into())]
    }

    /// `brew` itself, run by absolute path like every other binary Yard starts.
    pub fn brew_bin(&self) -> PathBuf {
        self.prefix.join("bin/brew")
    }

    pub fn nginx_bin(&self) -> PathBuf {
        self.prefix.join("opt/nginx/bin/nginx")
    }

    pub fn dnsmasq_bin(&self) -> PathBuf {
        self.prefix.join("opt/dnsmasq/sbin/dnsmasq")
    }

    pub fn nginx_mime_types(&self) -> PathBuf {
        self.prefix.join("etc/nginx/mime.types")
    }

    pub fn nginx_fastcgi_params(&self) -> PathBuf {
        self.prefix.join("etc/nginx/fastcgi_params")
    }

    pub fn php_conf_d(&self, v: PhpVersion) -> PathBuf {
        self.prefix.join(format!("etc/php/{v}/conf.d"))
    }

    /// The keg for `v`: `opt/php@<v>`, else the plain `opt/php` keg when its Cellar
    /// version has the same major.minor (that is how the tap ships 8.5).
    pub fn php_opt(
        &self,
        v: PhpVersion,
        exists: &dyn Fn(&Path) -> bool,
        read_link: &dyn Fn(&Path) -> Option<PathBuf>,
    ) -> Result<PathBuf, YardError> {
        let versioned = self.prefix.join(format!("opt/php@{v}"));
        if exists(&versioned) {
            return Ok(versioned);
        }
        let plain = self.prefix.join("opt/php");
        let plain_version = read_link(&plain)
            .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()))
            .and_then(|full| {
                let mut parts = full.split('.');
                Some(format!("{}.{}", parts.next()?, parts.next()?))
            });
        if exists(&plain) && plain_version.as_deref() == Some(v.to_string().as_str()) {
            return Ok(plain);
        }
        Err(YardError::new(ErrorCode::PhpNotInstalled, format!("PHP {v} is not installed"))
            .with_hint(format!("yard php install {v}")))
    }

    /// Every supported version whose keg `php_opt` finds, oldest first. Read from brew
    /// rather than recorded, so a version installed with brew directly counts too.
    pub fn installed_php(
        &self,
        exists: &dyn Fn(&Path) -> bool,
        read_link: &dyn Fn(&Path) -> Option<PathBuf>,
    ) -> Vec<PhpVersion> {
        crate::php::SUPPORTED.into_iter().filter(|v| self.php_opt(*v, exists, read_link).is_ok()).collect()
    }

    /// A binary in a version's keg: `php`, `pecl`, `phpize`, `php-config`.
    pub fn php_bin(opt: &Path, name: &str) -> PathBuf {
        opt.join("bin").join(name)
    }

    pub fn php_fpm(opt: &Path) -> PathBuf {
        opt.join("sbin/php-fpm")
    }

    /// `FORMULA_MISSING` when `bin` is absent. A Cellar directory without the binary
    /// is a broken install, which `brew install` would skip, so its hint is reinstall.
    pub fn require_formula(
        &self,
        name: &str,
        bin: &Path,
        exists: &dyn Fn(&Path) -> bool,
    ) -> Result<(), YardError> {
        if exists(bin) {
            return Ok(());
        }
        let verb = if exists(&self.prefix.join("Cellar").join(name)) { "reinstall" } else { "install" };
        Err(YardError::new(
            ErrorCode::FormulaMissing,
            format!("{name} is not installed ({} is missing)", bin.display()),
        )
        .with_hint(format!("brew {verb} {name}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhpVersion;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use yard_proto::ErrorCode;

    fn v(s: &str) -> PhpVersion {
        s.parse().unwrap()
    }

    fn exists_in(set: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = set.iter().map(PathBuf::from).collect();
        move |p| set.contains(p)
    }

    fn links(map: &[(&str, &str)]) -> impl Fn(&Path) -> Option<PathBuf> {
        let map: HashMap<PathBuf, PathBuf> =
            map.iter().map(|(a, b)| (PathBuf::from(a), PathBuf::from(b))).collect();
        move |p| map.get(p).cloned()
    }

    fn brew() -> Brew {
        Brew::new("/opt/homebrew")
    }

    #[test]
    fn fixed_paths() {
        let b = brew();
        assert_eq!(b.nginx_bin(), Path::new("/opt/homebrew/opt/nginx/bin/nginx"));
        assert_eq!(b.dnsmasq_bin(), Path::new("/opt/homebrew/opt/dnsmasq/sbin/dnsmasq"));
        assert_eq!(b.nginx_mime_types(), Path::new("/opt/homebrew/etc/nginx/mime.types"));
        assert_eq!(b.nginx_fastcgi_params(), Path::new("/opt/homebrew/etc/nginx/fastcgi_params"));
        assert_eq!(b.php_conf_d(v("8.4")), Path::new("/opt/homebrew/etc/php/8.4/conf.d"));
    }

    #[test]
    fn installed_php_lists_resolvable_kegs() {
        let ex = exists_in(&["/opt/homebrew/opt/php@7.1", "/opt/homebrew/opt/php"]);
        let ln = links(&[("/opt/homebrew/opt/php", "../Cellar/php/8.5.1")]);
        assert_eq!(brew().installed_php(&ex, &ln), vec![v("7.1"), v("8.5")]);
        assert_eq!(Brew::php_bin(Path::new("/o/php@7.1"), "pecl"), Path::new("/o/php@7.1/bin/pecl"));
    }

    #[test]
    fn missing_php_hint_is_yard_php_install() {
        let e = brew().php_opt(v("7.3"), &exists_in(&[]), &links(&[])).unwrap_err();
        assert_eq!(e.code, ErrorCode::PhpNotInstalled);
        assert_eq!(e.hint.as_deref(), Some("yard php install 7.3"));
    }

    #[test]
    fn versioned_keg_wins() {
        let ex = exists_in(&["/opt/homebrew/opt/php@8.4"]);
        let opt = brew().php_opt(v("8.4"), &ex, &links(&[])).unwrap();
        assert_eq!(opt, Path::new("/opt/homebrew/opt/php@8.4"));
        assert_eq!(Brew::php_fpm(&opt), Path::new("/opt/homebrew/opt/php@8.4/sbin/php-fpm"));
    }

    #[test]
    fn php_8_5_resolves_via_plain_php_keg() {
        let ex = exists_in(&["/opt/homebrew/opt/php"]);
        let ln = links(&[("/opt/homebrew/opt/php", "../Cellar/php/8.5.11")]);
        assert_eq!(brew().php_opt(v("8.5"), &ex, &ln).unwrap(), Path::new("/opt/homebrew/opt/php"));
    }

    #[test]
    fn php_8_5_does_not_match_a_different_plain_keg() {
        let ex = exists_in(&["/opt/homebrew/opt/php"]);
        let ln = links(&[("/opt/homebrew/opt/php", "../Cellar/php/8.6.0")]);
        let e = brew().php_opt(v("8.5"), &ex, &ln).unwrap_err();
        assert_eq!(e.code, ErrorCode::PhpNotInstalled);
        assert_eq!(e.hint.as_deref(), Some("yard php install 8.5"));
    }

    #[test]
    fn yard_brew_prefix_overrides_detection() {
        let b = Brew::from_var(Some(PathBuf::from("/fake")), &exists_in(&[])).unwrap();
        assert_eq!(b.prefix, Path::new("/fake"));
        let e = Brew::from_var(None, &exists_in(&[])).unwrap_err();
        assert_eq!(e.code, ErrorCode::BrewMissing);
    }

    #[test]
    fn missing_brew_is_reported() {
        let e = Brew::detect(&exists_in(&[])).unwrap_err();
        assert_eq!(e.code, ErrorCode::BrewMissing);
        assert!(e.hint.unwrap().contains("https://brew.sh"));
        assert_eq!(
            Brew::detect(&exists_in(&["/opt/homebrew/bin/brew"])).unwrap().prefix,
            Path::new("/opt/homebrew")
        );
    }

    #[test]
    fn missing_formula_hint_says_install() {
        let b = brew();
        let e = b.require_formula("nginx", &b.nginx_bin(), &exists_in(&[])).unwrap_err();
        assert_eq!(e.code, ErrorCode::FormulaMissing);
        assert_eq!(e.hint.as_deref(), Some("brew install nginx"));
        assert!(
            b.require_formula("nginx", &b.nginx_bin(), &exists_in(&["/opt/homebrew/opt/nginx/bin/nginx"]))
                .is_ok()
        );
    }

    #[test]
    fn empty_keg_hint_says_reinstall() {
        let b = brew();
        let e = b
            .require_formula("dnsmasq", &b.dnsmasq_bin(), &exists_in(&["/opt/homebrew/Cellar/dnsmasq"]))
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::FormulaMissing);
        assert_eq!(e.hint.as_deref(), Some("brew reinstall dnsmasq"));
    }
}
