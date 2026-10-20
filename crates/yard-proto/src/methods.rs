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
    }

    #[test]
    fn docroot_params_allow_missing_path() {
        let p: DocrootParams = serde_json::from_value(json!({"site": "blog"})).unwrap();
        assert_eq!(p.path, None);
    }
}
