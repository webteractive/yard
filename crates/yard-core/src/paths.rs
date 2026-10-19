//! Every path under the data root (spec §3.3), plus the fixed system paths.

use crate::PhpVersion;
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

pub const DAEMON_LABEL: &str = "dev.more.yard.daemon";
pub const HELPER_LABEL: &str = "dev.more.yard.helper";
pub const HELPER_SOCKET: &str = "/var/run/dev.more.yard.helper.sock";
pub const HELPER_BINARY: &str = "/Library/PrivilegedHelperTools/dev.more.yard.helper";
pub const RESOLVER_MARKER: &str = "# Managed by Yard";
pub const RESOLVER_DIR: &str = "/etc/resolver";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `YARD_ROOT`, else `$HOME/Library/Application Support/Yard`.
    pub fn from_env() -> Result<Self, YardError> {
        Self::from_vars(
            std::env::var_os("YARD_ROOT").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    pub fn from_vars(yard_root: Option<PathBuf>, home: Option<PathBuf>) -> Result<Self, YardError> {
        match (yard_root, home) {
            (Some(root), _) => Ok(Self::new(root)),
            (None, Some(home)) => Ok(Self::new(home.join("Library/Application Support/Yard"))),
            (None, None) => Err(YardError::new(ErrorCode::Internal, "neither YARD_ROOT nor HOME is set")),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn yard_toml(&self) -> PathBuf {
        self.root.join("yard.toml")
    }

    pub fn sites_toml(&self) -> PathBuf {
        self.root.join("sites.toml")
    }

    pub fn ports_toml(&self) -> PathBuf {
        self.root.join("ports.toml")
    }

    pub fn hashes(&self) -> PathBuf {
        self.root.join("state/hashes.json")
    }

    pub fn nginx_dir(&self) -> PathBuf {
        self.root.join("nginx")
    }

    pub fn nginx_conf(&self) -> PathBuf {
        self.root.join("nginx/nginx.conf")
    }

    pub fn nginx_sites_dir(&self) -> PathBuf {
        self.root.join("nginx/sites")
    }

    pub fn nginx_site(&self, name: &str) -> PathBuf {
        self.nginx_sites_dir().join(format!("{name}.conf"))
    }

    pub fn nginx_override(&self, name: &str) -> PathBuf {
        self.root.join(format!("nginx/overrides/{name}.conf"))
    }

    pub fn nginx_default_dir(&self) -> PathBuf {
        self.root.join("nginx/default")
    }

    pub fn nginx_default_index(&self) -> PathBuf {
        self.nginx_default_dir().join("index.html")
    }

    pub fn php_dir(&self, v: PhpVersion) -> PathBuf {
        self.root.join(format!("php/{v}"))
    }

    pub fn php_ini(&self, v: PhpVersion) -> PathBuf {
        self.php_dir(v).join("php.ini")
    }

    pub fn fpm_conf(&self, v: PhpVersion) -> PathBuf {
        self.php_dir(v).join("fpm.conf")
    }

    pub fn php_conf_d(&self, v: PhpVersion) -> PathBuf {
        self.php_dir(v).join("conf.d")
    }

    pub fn php_overrides(&self, v: PhpVersion) -> PathBuf {
        self.php_conf_d(v).join("99-overrides.ini")
    }

    pub fn fpm_socket(&self, v: PhpVersion) -> PathBuf {
        self.run_dir().join(format!("php-{v}.sock"))
    }

    pub fn dnsmasq_conf(&self) -> PathBuf {
        self.root.join("dnsmasq/dnsmasq.conf")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn log(&self, unit: &str) -> PathBuf {
        self.logs_dir().join(format!("{unit}.log"))
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn nginx_pid(&self) -> PathBuf {
        self.run_dir().join("nginx.pid")
    }

    pub fn daemon_socket(&self) -> PathBuf {
        self.root.join("yardd.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhpVersion;

    fn v(s: &str) -> PhpVersion {
        s.parse().unwrap()
    }

    #[test]
    fn default_paths_follow_spec_layout() {
        let p = Paths::new("/r");
        assert_eq!(p.yard_toml(), Path::new("/r/yard.toml"));
        assert_eq!(p.sites_toml(), Path::new("/r/sites.toml"));
        assert_eq!(p.ports_toml(), Path::new("/r/ports.toml"));
        assert_eq!(p.hashes(), Path::new("/r/state/hashes.json"));
        assert_eq!(p.nginx_conf(), Path::new("/r/nginx/nginx.conf"));
        assert_eq!(p.nginx_site("a"), Path::new("/r/nginx/sites/a.conf"));
        assert_eq!(p.nginx_override("a"), Path::new("/r/nginx/overrides/a.conf"));
        assert_eq!(p.nginx_default_index(), Path::new("/r/nginx/default/index.html"));
        assert_eq!(p.php_ini(v("8.4")), Path::new("/r/php/8.4/php.ini"));
        assert_eq!(p.fpm_conf(v("8.4")), Path::new("/r/php/8.4/fpm.conf"));
        assert_eq!(p.php_conf_d(v("8.4")), Path::new("/r/php/8.4/conf.d"));
        assert_eq!(p.php_overrides(v("8.4")), Path::new("/r/php/8.4/conf.d/99-overrides.ini"));
        assert_eq!(p.fpm_socket(v("8.4")), Path::new("/r/run/php-8.4.sock"));
        assert_eq!(p.dnsmasq_conf(), Path::new("/r/dnsmasq/dnsmasq.conf"));
        assert_eq!(p.log("nginx"), Path::new("/r/logs/nginx.log"));
        assert_eq!(p.daemon_socket(), Path::new("/r/yardd.sock"));
        assert_eq!(p.nginx_pid(), Path::new("/r/run/nginx.pid"));
    }

    #[test]
    fn from_root_env_prefers_yard_root() {
        assert_eq!(Paths::from_vars(Some("/x".into()), Some("/h".into())).unwrap().root(), Path::new("/x"));
        assert_eq!(
            Paths::from_vars(None, Some("/h".into())).unwrap().root(),
            Path::new("/h/Library/Application Support/Yard")
        );
        assert!(Paths::from_vars(None, None).is_err());
    }
}
