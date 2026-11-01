//! Daemon method names and their parameter/result shapes (spec §6.2).

use crate::{Profile, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
pub const SITES_INFO: &str = "sites.info";
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
pub const SERVICES_LIST: &str = "services.list";
pub const SERVICES_ADD: &str = "services.add";
pub const SERVICES_REMOVE: &str = "services.remove";
pub const SERVICES_START: &str = "services.start";
pub const SERVICES_STOP: &str = "services.stop";
pub const SERVICES_RESTART: &str = "services.restart";
pub const SERVICES_LOGS: &str = "services.logs";
pub const SERVICES_PORT: &str = "services.port";
pub const SERVICES_SLEEP_SET: &str = "services.sleep.set";
pub const ENV_PRINT: &str = "env.print";
pub const SOURCES_LIST: &str = "sources.list";
pub const SOURCES_TRUST: &str = "sources.trust";
pub const SOURCES_UNTRUST: &str = "sources.untrust";
pub const NODE_LIST: &str = "node.list";
pub const NODE_INSTALL: &str = "node.install";
pub const NODE_UNINSTALL: &str = "node.uninstall";
pub const NODE_DEFAULT: &str = "node.default";
pub const NODE_USE: &str = "node.use";
pub const NODE_UNUSE: &str = "node.unuse";
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

/// `doctor`. `client_env` is what the CLI's shell sets that brew reads (`XDG_CONFIG_HOME`
/// and every `HOMEBREW_FORBIDDEN_*`), so the daemon can say where its own environment
/// differs. Older clients send no params at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorParams {
    #[serde(default)]
    pub client_env: BTreeMap<String, String>,
}

/// `sites.init`: write the site's `.yard`, the managed one in Yard's data root or with
/// `local` the one in `path` (unless one is there and `force` is unset; a `.yard` in `path`
/// always wins and is applied), move the site's settings into it and link it. `name`
/// defaults to the existing link, else the directory's name. A name another directory
/// serves, or whose managed `.yard` was written for another directory, is
/// `SITE_NAME_TAKEN` unless `replace` is set, which moves that site here and keeps how it
/// serves (and keeps the other managed file as `.yard-backup`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitParams {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub replace: bool,
    /// Write `.yard` into the project instead of the managed one in Yard's data root.
    #[serde(default)]
    pub local: bool,
}

/// `sites.secure`, `sites.unsecure` and `node.unuse`.
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

/// `node.list`: `available` also asks nodejs.org (through fnm) for the newest release of
/// each line, or of `major` only. `null` params are the defaults.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NodeListParams {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub major: Option<u32>,
}

/// `node.install`: a `spec` (`22`, `16.14.0`, `lts/iron`), or the absolute `path` of a
/// directory whose pin is installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInstallParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `node.uninstall`: an installed version such as `"16.14.0"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeVersionParams {
    pub version: String,
}

/// `node.default`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSpecParams {
    pub spec: String,
}

/// `node.use`; `node.unuse` takes [`SiteParams`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeUseParams {
    pub site: String,
    pub spec: String,
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
        assert_eq!(
            [SERVICES_LIST, SERVICES_ADD, SERVICES_REMOVE, SERVICES_START],
            ["services.list", "services.add", "services.remove", "services.start"]
        );
        assert_eq!(
            [SERVICES_STOP, SERVICES_RESTART, SERVICES_LOGS, SERVICES_PORT],
            ["services.stop", "services.restart", "services.logs", "services.port"]
        );
        assert_eq!((ENV_PRINT, SITES_INFO), ("env.print", "sites.info"));
        assert_eq!(
            [SOURCES_LIST, SOURCES_TRUST, SOURCES_UNTRUST],
            ["sources.list", "sources.trust", "sources.untrust"]
        );
    }

    #[test]
    fn sleep_method_name() {
        assert_eq!(SERVICES_SLEEP_SET, "services.sleep.set");
        let p: ServiceSleepParams = serde_json::from_value(json!({"name":"redis","after":"15m"})).unwrap();
        assert_eq!((p.name.as_str(), p.after.as_str()), ("redis", "15m"));
    }

    #[test]
    fn node_method_names() {
        assert_eq!(
            [NODE_LIST, NODE_INSTALL, NODE_UNINSTALL, NODE_DEFAULT, NODE_USE, NODE_UNUSE],
            ["node.list", "node.install", "node.uninstall", "node.default", "node.use", "node.unuse"]
        );
    }

    #[test]
    fn node_list_params_accept_null() {
        let p: NodeListParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!((p.available, p.major), (false, None));
        let p: NodeListParams = serde_json::from_value(json!({"available":true,"major":16})).unwrap();
        assert_eq!((p.available, p.major), (true, Some(16)));
        assert_eq!(serde_json::from_value::<Option<NodeListParams>>(json!(null)).unwrap(), None);
        let p: NodeInstallParams = serde_json::from_value(json!({"path":"/code/maker"})).unwrap();
        assert_eq!((p.spec, p.path.as_deref()), (None, Some("/code/maker")));
    }

    #[test]
    fn doctor_params_without_client_env_parse() {
        let p: DoctorParams = serde_json::from_value(json!({})).unwrap();
        assert!(p.client_env.is_empty());
        let p: DoctorParams = serde_json::from_value(json!({"client_env":{"XDG_CONFIG_HOME":"/x"}})).unwrap();
        assert_eq!(p.client_env["XDG_CONFIG_HOME"], "/x");
    }

    #[test]
    fn service_params_default_their_options() {
        let p: ServiceAddParams = serde_json::from_value(json!({"engine":"redis"})).unwrap();
        assert_eq!((p.engine.as_str(), p.version, p.port), ("redis", None, None));
        let p: ServiceRemoveParams = serde_json::from_value(json!({"name":"redis"})).unwrap();
        assert!(!p.keep_data);
        let p: ServiceLogsParams = serde_json::from_value(json!({"name":"redis"})).unwrap();
        assert_eq!(p.lines, None);
        let p: EnvParams = serde_json::from_value(json!({"path":"/code/blog"})).unwrap();
        assert_eq!((p.path.as_deref(), p.site, p.service), (Some("/code/blog"), None, None));
        let p: EnvParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!((p.path, p.site, p.service), (None, None, None));
        let p: SiteDetailParams = serde_json::from_value(json!({"site":"blog"})).unwrap();
        assert_eq!((p.path, p.site.as_deref()), (None, Some("blog")));
        let p: SourceParams = serde_json::from_value(json!({"target":"minio"})).unwrap();
        assert!(!p.tap);
        let p: ServicePortParams = serde_json::from_value(json!({"name":"redis","port":6380})).unwrap();
        assert_eq!(p.port, 6380);
        let p: ServiceParams = serde_json::from_value(json!({"name":"redis"})).unwrap();
        assert_eq!(p.name, "redis");
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

/// `services.add`: `version` is a line such as `8.4` (the engine's default when unset) and
/// `port` pins the instance's port (the usual one, or the next free one, when unset). For
/// emailulator, `port` is its SMTP port and `path` the binary the CLI found on the user's PATH
/// (protocol 2.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceAddParams {
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `services.start|stop|restart`: an instance name such as `mysql-8.4`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceParams {
    pub name: String,
}

/// `services.remove`: `keep_data` leaves the instance's data directory in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceRemoveParams {
    pub name: String,
    #[serde(default)]
    pub keep_data: bool,
}

/// `services.port`: move an instance to another port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServicePortParams {
    pub name: String,
    pub port: u16,
}

/// `services.sleep.set`: how long an instance may go unused before it sleeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceSleepParams {
    pub name: String,
    /// A duration (`15m`), `off`, or `default` to follow `yard.toml`.
    pub after: String,
}

/// `services.logs`: the last `lines` lines (100 when unset, and never more than 10000).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceLogsParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
}

/// `env.print`: `service` names one instance; else `site`, else the site whose path
/// contains `path`; else every instance.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

/// `sites.info`: a site by name, or by a `path` inside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteDetailParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

/// `sources.trust|untrust`: a formula (`minio`) or, with `tap`, a whole tap (`minio/stable`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceParams {
    pub target: String,
    #[serde(default)]
    pub tap: bool,
}
