//! Daemon method names and their parameter/result shapes (spec §6.2).

use crate::{Profile, Topic};
use serde::{Deserialize, Serialize};

pub const HELLO: &str = "hello";
pub const STATUS: &str = "status";
pub const DOCTOR: &str = "doctor";
pub const SITES_LIST: &str = "sites.list";
pub const SITES_LINK: &str = "sites.link";
pub const SITES_UNLINK: &str = "sites.unlink";
pub const SITES_PROXY: &str = "sites.proxy";
pub const SITES_UNPROXY: &str = "sites.unproxy";
pub const SITES_DOCROOT: &str = "sites.docroot";
pub const SITES_ISOLATE: &str = "sites.isolate";
pub const SITES_INIT: &str = "sites.init";
pub const SITES_SECURE: &str = "sites.secure";
pub const SITES_UNSECURE: &str = "sites.unsecure";
pub const CA_INFO: &str = "ca.info";
pub const PORTS_LIST: &str = "ports.list";
pub const PORTS_CHECK: &str = "ports.check";
pub const PORTS_SET: &str = "ports.set";
pub const PORTS_PIN: &str = "ports.pin";
pub const PORTS_UNPIN: &str = "ports.unpin";
pub const PORTS_RANGE: &str = "ports.range";
pub const PORTS_PROFILE: &str = "ports.profile";
pub const UNITS_RESTART: &str = "units.restart";
pub const PHP_LIST: &str = "php.list";
pub const PHP_INSTALL: &str = "php.install";
pub const PHP_UNINSTALL: &str = "php.uninstall";
pub const PHP_DEFAULT: &str = "php.default";
pub const PHP_CONFIG_GET: &str = "php.config.get";
pub const PHP_CONFIG_SET: &str = "php.config.set";
pub const PHP_CONFIG_UNSET: &str = "php.config.unset";
pub const EXT_ADD: &str = "ext.add";
pub const EXT_LIST: &str = "ext.list";
pub const EXT_ENABLE: &str = "ext.enable";
pub const EXT_DISABLE: &str = "ext.disable";
pub const EXT_REMOVE: &str = "ext.remove";
pub const XDEBUG_SET: &str = "xdebug.set";
pub const JOBS_CANCEL: &str = "jobs.cancel";
pub const SUBSCRIBE: &str = "subscribe";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: String,
    pub client: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: String,
    pub daemon_version: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkParams {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameParams {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyParams {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocrootParams {
    pub site: String,
    /// `None` clears the override and returns to detection.
    #[serde(default)]
    pub path: Option<String>,
}

/// `sites.init`: write `.yard` in `path` (unless it is there and `force` is unset), move
/// the site's settings into it and link it. `name` defaults to the existing link, else
/// the directory's name. A name another directory serves is `SITE_NAME_TAKEN` unless
/// `replace` is set, which moves that site here and keeps how it serves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitParams {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub replace: bool,
}

/// `sites.secure` and `sites.unsecure`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteParams {
    pub site: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolateParams {
    pub site: String,
    /// `None` unisolates: the site runs the default version again.
    #[serde(default)]
    pub php: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortSetParams {
    pub unit: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitParams {
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeParams {
    pub range: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileParams {
    pub profile: Profile,
}

/// `php.install`, `php.uninstall` and `php.default`: a version such as `"7.1"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhpParams {
    pub php: String,
}

/// `php.config.get|set|unset` on `99-overrides.ini`. `php` defaults to the default
/// version; `value` is for `set` only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhpConfigParams {
    #[serde(default)]
    pub php: Option<String>,
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribeParams {
    pub topics: Vec<Topic>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn method_names_match_spec() {
        assert_eq!(SITES_LINK, "sites.link");
        assert_eq!(UNITS_RESTART, "units.restart");
        assert_eq!(PORTS_PROFILE, "ports.profile");
        assert_eq!((SITES_SECURE, SITES_UNSECURE, CA_INFO), ("sites.secure", "sites.unsecure", "ca.info"));
        assert_eq!(SITES_INIT, "sites.init");
    }

    #[test]
    fn init_params_default_force_false() {
        let p: InitParams = serde_json::from_value(json!({"path": "/code/blog"})).unwrap();
        assert_eq!((p.name, p.force, p.replace), (None, false, false));
    }

    #[test]
    fn docroot_params_allow_missing_path() {
        let p: DocrootParams = serde_json::from_value(json!({"site": "blog"})).unwrap();
        assert_eq!(p.path, None);
    }
}

/// `ext.add`: a name (brew formula or pecl package) or the path of a `.so`; the version
/// defaults to the global default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtAddParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
}

/// `ext.list`; the version defaults to the global default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
}

/// `ext.enable`, `ext.disable` and `ext.remove`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
}

/// `xdebug.set`: `ext.enable` or `ext.disable` of xdebug.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XdebugParams {
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
}

/// `jobs.cancel`: the id a job method answered with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobParams {
    pub job_id: String,
}
