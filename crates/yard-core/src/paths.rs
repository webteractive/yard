//! Every path under the data root (spec §3.3), plus the fixed system paths.

use crate::PhpVersion;
use crate::node::NodeVersion;
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

    /// `YARD_ROOT`, else `$HOME/.yard`.
    pub fn from_env() -> Result<Self, YardError> {
        Self::from_vars(
            std::env::var_os("YARD_ROOT").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    pub fn from_vars(yard_root: Option<PathBuf>, home: Option<PathBuf>) -> Result<Self, YardError> {
        match (yard_root, home) {
            (Some(root), _) => Ok(Self::new(root)),
            (None, Some(home)) => Ok(Self::new(home.join(".yard"))),
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

    pub fn services_toml(&self) -> PathBuf {
        self.root.join("services.toml")
    }

    /// Where the managed `.yard` files live, one directory per site name.
    pub fn managed_sites_dir(&self) -> PathBuf {
        self.root.join("sites")
    }

    /// A site's managed `.yard`, used when its project has none of its own.
    pub fn managed_project_file(&self, name: &str) -> PathBuf {
        self.managed_sites_dir().join(name).join(crate::project::FILE)
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

    /// Generated nginx blocks for services that have a web console (`minio.<tld>`).
    pub fn nginx_services_dir(&self) -> PathBuf {
        self.root.join("nginx/services")
    }

    pub fn nginx_service(&self, name: &str) -> PathBuf {
        self.nginx_services_dir().join(format!("{name}.conf"))
    }

    pub fn nginx_default_dir(&self) -> PathBuf {
        self.root.join("nginx/default")
    }

    pub fn nginx_default_index(&self) -> PathBuf {
        self.nginx_default_dir().join("index.html")
    }

    /// Setup pages for sites whose `.yard` is not met.
    pub fn nginx_setup_dir(&self) -> PathBuf {
        self.root.join("nginx/setup")
    }

    pub fn nginx_setup_page(&self, name: &str) -> PathBuf {
        self.nginx_setup_dir().join(format!("{name}.html"))
    }

    /// The modules each PHP keg has compiled in, by the keg's canonical path.
    pub fn php_modules_cache(&self) -> PathBuf {
        self.root.join("state/php-modules.json")
    }

    /// The shims: `php`, `composer`, … linked to `yard-shim`.
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// One directory per service instance: `data/`, the generated config, `overrides.conf`.
    pub fn services_dir(&self) -> PathBuf {
        self.root.join("services")
    }

    pub fn service_dir(&self, instance: &str) -> PathBuf {
        self.services_dir().join(instance)
    }

    pub fn service_data(&self, instance: &str) -> PathBuf {
        self.service_dir(instance).join("data")
    }

    /// A generated file in the instance's directory (`my.cnf`, `redis.conf`).
    pub fn service_conf(&self, instance: &str, file: &str) -> PathBuf {
        self.service_dir(instance).join(file)
    }

    /// User-owned, created empty once and never written.
    pub fn service_overrides(&self, instance: &str) -> PathBuf {
        self.service_dir(instance).join("overrides.conf")
    }

    pub fn service_pid(&self, instance: &str) -> PathBuf {
        self.run_dir().join(format!("{instance}.pid"))
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

    /// yardd's wake socket, which nginx and the shims ask before a site's sleeping
    /// services are needed (M4e spec §5.1, §5.2).
    pub fn wake_socket(&self) -> PathBuf {
        self.run_dir().join("wake.sock")
    }

    /// The sites with a sleeping instance, one per line, for the shims (M4e spec §5.2).
    pub fn sleeping_file(&self) -> PathBuf {
        self.run_dir().join("sleeping")
    }

    pub fn nginx_pid(&self) -> PathBuf {
        self.run_dir().join("nginx.pid")
    }

    pub fn daemon_socket(&self) -> PathBuf {
        self.root.join("yardd.sock")
    }

    pub fn ca_dir(&self) -> PathBuf {
        self.root.join("ca")
    }

    pub fn ca_cert(&self) -> PathBuf {
        self.ca_dir().join("yard-ca.pem")
    }

    pub fn ca_key(&self) -> PathBuf {
        self.ca_dir().join("yard-ca.key")
    }

    pub fn certs_dir(&self) -> PathBuf {
        self.root.join("certs")
    }

    /// Certificates for service hostnames, apart from the sites'.
    pub fn services_certs_dir(&self) -> PathBuf {
        self.root.join("certs/services")
    }

    pub fn cert(&self, site: &str) -> PathBuf {
        self.certs_dir().join(format!("{site}.pem"))
    }

    pub fn cert_key(&self, site: &str) -> PathBuf {
        self.certs_dir().join(format!("{site}.key"))
    }

    /// fnm's `FNM_DIR`: Node versions Yard installs, and `lts.toml`.
    pub fn node_dir(&self) -> PathBuf {
        self.root.join("node")
    }

    pub fn node_versions_dir(&self) -> PathBuf {
        self.node_dir().join("node-versions")
    }

    /// `node`, `npm` or `npx` of an installed version.
    pub fn node_bin(&self, v: NodeVersion, name: &str) -> PathBuf {
        self.node_versions_dir().join(format!("v{v}")).join("installation/bin").join(name)
    }

    /// LTS codenames seen in nodejs.org's release list (generated).
    pub fn lts_toml(&self) -> PathBuf {
        self.node_dir().join("lts.toml")
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
        assert_eq!(p.ca_cert(), Path::new("/r/ca/yard-ca.pem"));
        assert_eq!(p.ca_key(), Path::new("/r/ca/yard-ca.key"));
        assert_eq!(p.cert("blog"), Path::new("/r/certs/blog.pem"));
        assert_eq!(p.cert_key("blog"), Path::new("/r/certs/blog.key"));
    }

    #[test]
    fn sleep_paths_are_in_run() {
        let p = Paths::new("/r");
        assert_eq!(p.wake_socket(), Path::new("/r/run/wake.sock"));
        assert_eq!(p.sleeping_file(), Path::new("/r/run/sleeping"));
    }

    #[test]
    fn service_paths_follow_the_m4b_layout() {
        let p = Paths::new("/r");
        assert_eq!(p.services_toml(), Path::new("/r/services.toml"));
        assert_eq!(p.services_dir(), Path::new("/r/services"));
        assert_eq!(p.service_dir("mysql-8.4"), Path::new("/r/services/mysql-8.4"));
        assert_eq!(p.service_data("mysql-8.4"), Path::new("/r/services/mysql-8.4/data"));
        assert_eq!(p.service_conf("mysql-8.4", "my.cnf"), Path::new("/r/services/mysql-8.4/my.cnf"));
        assert_eq!(p.service_overrides("redis"), Path::new("/r/services/redis/overrides.conf"));
        assert_eq!(p.service_pid("redis"), Path::new("/r/run/redis.pid"));
        assert_eq!(p.nginx_services_dir(), Path::new("/r/nginx/services"));
        assert_eq!(p.nginx_service("minio"), Path::new("/r/nginx/services/minio.conf"));
        assert_eq!(p.services_certs_dir(), Path::new("/r/certs/services"));
    }

    #[test]
    fn managed_project_files_live_under_sites() {
        let p = Paths::new("/r");
        assert_eq!(p.managed_sites_dir(), Path::new("/r/sites"));
        assert_eq!(p.managed_project_file("control"), Path::new("/r/sites/control/.yard"));
    }

    #[test]
    fn node_paths_are_fnm_s_layout_under_node() {
        let p = Paths::new("/r");
        assert_eq!(p.node_dir(), Path::new("/r/node"));
        assert_eq!(p.node_versions_dir(), Path::new("/r/node/node-versions"));
        assert_eq!(
            p.node_bin("16.14.0".parse().unwrap(), "npm"),
            Path::new("/r/node/node-versions/v16.14.0/installation/bin/npm")
        );
        assert_eq!(p.lts_toml(), Path::new("/r/node/lts.toml"));
    }

    #[test]
    fn from_root_env_prefers_yard_root() {
        assert_eq!(Paths::from_vars(Some("/x".into()), Some("/h".into())).unwrap().root(), Path::new("/x"));
        assert_eq!(Paths::from_vars(None, Some("/h".into())).unwrap().root(), Path::new("/h/.yard"));
        assert!(Paths::from_vars(None, None).is_err());
    }
}
