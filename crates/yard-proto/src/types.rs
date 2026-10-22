//! Domain values that cross the wire, shared by core, daemon, CLI and app.

use crate::YardError;
use serde::{Deserialize, Serialize};

/// A named set of pinned nginx ports (spec §7). `standard` needs no root for nginx;
/// `privileged` takes 80/443 from the helper. Neither sets the TLD. M1 called them
/// `trial` and `default`, and those names still parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    #[serde(alias = "trial")]
    Standard,
    #[serde(alias = "default")]
    Privileged,
}

impl Profile {
    /// The name `ports.toml`, the CLI and the API use.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Standard => "standard",
            Profile::Privileged => "privileged",
        }
    }

    pub fn http_port(self) -> u16 {
        match self {
            Profile::Standard => 8080,
            Profile::Privileged => 80,
        }
    }

    pub fn https_port(self) -> u16 {
        match self {
            Profile::Standard => 8443,
            Profile::Privileged => 443,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteSource {
    Linked,
    Proxy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SiteKind {
    Php { path: String, docroot: String, php: String },
    Proxy { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteInfo {
    pub name: String,
    pub host: String,
    pub url: String,
    pub source: SiteSource,
    #[serde(flatten)]
    pub kind: SiteKind,
    /// Served over HTTPS with a certificate from Yard's CA (`yard secure`); its `url` is
    /// then `https://`.
    #[serde(default)]
    pub secure: bool,
    /// The project's `.yard`, when it has one; the site is then served as that file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_file: Option<String>,
    /// What the `.yard` asks for that is not installed; the site serves a setup page
    /// until it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<Need>,
    /// Why the `.yard` cannot be read (`PROJECT_FILE_INVALID`); the site serves a setup
    /// page until it is fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_error: Option<YardError>,
}

impl SiteInfo {
    /// The site serves a setup page instead of the project.
    pub fn needs_setup(&self) -> bool {
        self.project_error.is_some() || !self.needs.is_empty()
    }
}

/// Something a project's `.yard` asks for that is not installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Need {
    Php {
        php: String,
    },
    /// `name` as the `.yard` writes it, e.g. `xdebug-2.9.8`.
    Extension {
        name: String,
        php: String,
    },
}

impl Need {
    /// `PHP 7.3`, `the redis extension for PHP 7.3`.
    pub fn describe(&self) -> String {
        match self {
            Need::Php { php } => format!("PHP {php}"),
            Need::Extension { name, php } => format!("the {name} extension for PHP {php}"),
        }
    }

    /// The command that installs it.
    pub fn command(&self) -> String {
        match self {
            Need::Php { php } => format!("yard php install {php}"),
            Need::Extension { name, php } => format!("yard ext add {name} --php {php}"),
        }
    }
}

/// `sites.init`: the `.yard` (written now, or already there) and the site it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitResult {
    pub file: String,
    pub text: String,
    /// `false` when the file was already there and left as it was.
    pub written: bool,
    pub site: SiteInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UnitState {
    Stopped,
    Starting,
    Running,
    Backoff { retry_in_ms: u64 },
    Failed { last_lines: Vec<String> },
}

impl UnitState {
    /// The `state` tag on its own: `running`, `backoff`, …
    pub fn name(&self) -> &'static str {
        match self {
            UnitState::Stopped => "stopped",
            UnitState::Starting => "starting",
            UnitState::Running => "running",
            UnitState::Backoff { .. } => "backoff",
            UnitState::Failed { .. } => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitInfo {
    pub name: String,
    #[serde(flatten)]
    pub state: UnitState,
    pub pid: Option<u32>,
    pub restarts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortMode {
    Pinned,
    Preferred,
    Auto,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortStatus {
    Free,
    /// Held by the Yard unit that owns the entry.
    Yard,
    InUse,
    ExternalListening,
    ExternalNotListening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HolderKind {
    Herd,
    BrewServices,
    Docker,
    Yard,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
    pub ppid: Option<u32>,
    pub kind: HolderKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    pub unit: String,
    pub port: u16,
    pub mode: PortMode,
    pub preferred: Option<u16>,
    pub status: PortStatus,
    pub holder: Option<Holder>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub cli: Option<String>,
    pub daemon: Option<String>,
    pub helper: Option<String>,
    pub protocol: String,
    pub commit: String,
    pub build_date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub version: VersionInfo,
    pub profile: Profile,
    pub tld: String,
    pub units: Vec<UnitInfo>,
    pub sites: usize,
    /// Why the last reconcile failed, until one succeeds; the last good state keeps serving.
    pub config_error: Option<YardError>,
}

/// One row of `php.list`: an installed version, or the default when it is not installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhpInfo {
    pub version: String,
    pub default: bool,
    pub installed: bool,
    /// Sites that run on this version.
    pub sites: Vec<String>,
    /// The state of its php-fpm pool (`running`, `stopped`, …); `None` when it has none.
    pub pool: Option<String>,
}

/// One row of `ext.list`: an extension `yard.toml` manages, or a module brew's `conf.d`
/// loads (`bundled`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtInfo {
    pub name: String,
    /// `brew`, `pecl`, `path` or `bundled`.
    pub source: String,
    pub enabled: bool,
}

/// `ca.info`: Yard's CA, made on first use, and whether the login keychain trusts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaInfo {
    /// The CA certificate, `<root>/ca/yard-ca.pem`.
    pub cert: String,
    /// Upper-case hex SHA-256 of the certificate's DER, as `security -Z` takes it.
    pub sha256: String,
    pub trusted: bool,
    /// The day it expires, `YYYY-MM-DD`.
    pub expires: String,
}

/// The answer to a method that runs as a job: its progress and result arrive as
/// `job_progress` and `job_finished` events on the `jobs` topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStarted {
    pub job_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_values() {
        assert_eq!((Profile::Standard.http_port(), Profile::Standard.https_port()), (8080, 8443));
        assert_eq!((Profile::Privileged.http_port(), Profile::Privileged.https_port()), (80, 443));
        assert_eq!(Profile::default(), Profile::Standard);
    }

    #[test]
    fn profile_names_and_aliases() {
        assert_eq!(serde_json::to_value(Profile::Standard).unwrap(), json!("standard"));
        assert_eq!(serde_json::to_value(Profile::Privileged).unwrap(), json!("privileged"));
        // M1 wrote these names into ports.toml.
        assert_eq!(serde_json::from_value::<Profile>(json!("trial")).unwrap(), Profile::Standard);
        assert_eq!(serde_json::from_value::<Profile>(json!("default")).unwrap(), Profile::Privileged);
    }

    #[test]
    fn unit_state_is_tagged() {
        let s = UnitState::Backoff { retry_in_ms: 2000 };
        assert_eq!(serde_json::to_value(&s).unwrap(), json!({"state":"backoff","retry_in_ms":2000}));
        assert_eq!(serde_json::to_value(UnitState::Running).unwrap(), json!({"state":"running"}));
    }

    #[test]
    fn port_enums_are_snake_case() {
        assert_eq!(serde_json::to_value(PortMode::Preferred).unwrap(), json!("preferred"));
        assert_eq!(
            serde_json::to_value(PortStatus::ExternalNotListening).unwrap(),
            json!("external_not_listening")
        );
        assert_eq!(serde_json::to_value(HolderKind::BrewServices).unwrap(), json!("brew_services"));
        assert_eq!(serde_json::to_value(CheckStatus::Warn).unwrap(), json!("warn"));
    }

    #[test]
    fn site_info_secure_defaults_to_false() {
        // What a 2.1 daemon sent: no `secure`.
        let v = json!({"name":"blog","host":"blog.yard","url":"http://blog.yard:8080/","source":"linked",
            "kind":"proxy","target":"http://127.0.0.1:3000"});
        assert!(!serde_json::from_value::<SiteInfo>(v).unwrap().secure);
    }

    #[test]
    fn ca_info_round_trips() {
        let ca = CaInfo {
            cert: "/r/ca/yard-ca.pem".into(),
            sha256: "AB12".into(),
            trusted: true,
            expires: "2036-10-18".into(),
        };
        let v = serde_json::to_value(&ca).unwrap();
        assert_eq!(
            v,
            json!({"cert":"/r/ca/yard-ca.pem","sha256":"AB12","trusted":true,"expires":"2036-10-18"})
        );
        assert_eq!(serde_json::from_value::<CaInfo>(v).unwrap(), ca);
    }

    #[test]
    fn site_info_without_project_fields_still_parses() {
        // What a 2.3 daemon sent: no project_file, needs or project_error.
        let v = json!({"name":"blog","host":"blog.yard","url":"http://blog.yard:8080/","source":"linked",
            "kind":"proxy","target":"http://127.0.0.1:3000","secure":false});
        let site = serde_json::from_value::<SiteInfo>(v).unwrap();
        assert_eq!((site.project_file, site.needs, site.project_error), (None, vec![], None));
    }

    #[test]
    fn site_info_omits_empty_project_fields() {
        let v = json!({"name":"blog","host":"blog.yard","url":"http://blog.yard:8080/","source":"linked",
            "kind":"proxy","target":"http://127.0.0.1:3000","secure":false});
        let site = serde_json::from_value::<SiteInfo>(v.clone()).unwrap();
        assert!(!site.needs_setup());
        assert_eq!(serde_json::to_value(&site).unwrap(), v);
    }

    #[test]
    fn needs_setup_when_a_need_or_an_error() {
        let v = json!({"name":"a","host":"a.yard","url":"http://a.yard:8080/","source":"linked",
            "kind":"proxy","target":"http://127.0.0.1:3000"});
        let mut site = serde_json::from_value::<SiteInfo>(v).unwrap();
        site.needs.push(Need::Php { php: "7.3".into() });
        assert!(site.needs_setup());
        site.needs.clear();
        site.project_error = Some(YardError::new(crate::ErrorCode::ProjectFileInvalid, "bad"));
        assert!(site.needs_setup());
    }

    #[test]
    fn need_is_tagged() {
        let n = Need::Extension { name: "redis".into(), php: "7.3".into() };
        assert_eq!(serde_json::to_value(&n).unwrap(), json!({"kind":"extension","name":"redis","php":"7.3"}));
        assert_eq!(
            serde_json::to_value(Need::Php { php: "7.3".into() }).unwrap(),
            json!({"kind":"php","php":"7.3"})
        );
    }

    #[test]
    fn need_describe_and_command() {
        let php = Need::Php { php: "7.3".into() };
        assert_eq!((php.describe(), php.command()), ("PHP 7.3".into(), "yard php install 7.3".into()));
        let ext = Need::Extension { name: "xdebug-2.9.8".into(), php: "7.3".into() };
        assert_eq!(ext.describe(), "the xdebug-2.9.8 extension for PHP 7.3");
        assert_eq!(ext.command(), "yard ext add xdebug-2.9.8 --php 7.3");
    }

    #[test]
    fn site_kind_is_tagged() {
        let k = SiteKind::Proxy { target: "http://127.0.0.1:3000".into() };
        assert_eq!(
            serde_json::to_value(&k).unwrap(),
            json!({"kind":"proxy","target":"http://127.0.0.1:3000"})
        );
    }
}
