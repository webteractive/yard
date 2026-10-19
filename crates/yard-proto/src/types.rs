//! Domain values that cross the wire, shared by core, daemon, CLI and app.

use crate::YardError;
use serde::{Deserialize, Serialize};

/// A named set of pinned values (spec §7). Trial runs beside Herd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Trial,
    Default,
}

impl Profile {
    pub fn tld(self) -> &'static str {
        match self {
            Profile::Trial => "yard",
            Profile::Default => "test",
        }
    }

    pub fn http_port(self) -> u16 {
        match self {
            Profile::Trial => 8080,
            Profile::Default => 80,
        }
    }

    pub fn https_port(self) -> u16 {
        match self {
            Profile::Trial => 8443,
            Profile::Default => 443,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteSource {
    Parked,
    Linked,
    Proxy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SiteKind {
    Php {
        path: String,
        docroot: String,
        php: String,
    },
    Proxy {
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteInfo {
    pub name: String,
    pub host: String,
    pub url: String,
    pub source: SiteSource,
    #[serde(flatten)]
    pub kind: SiteKind,
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
    pub parked: Vec<String>,
    pub units: Vec<UnitInfo>,
    pub sites: usize,
    pub config_error: Option<YardError>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_values() {
        assert_eq!(
            (
                Profile::Trial.tld(),
                Profile::Trial.http_port(),
                Profile::Trial.https_port()
            ),
            ("yard", 8080, 8443)
        );
        assert_eq!(
            (
                Profile::Default.tld(),
                Profile::Default.http_port(),
                Profile::Default.https_port()
            ),
            ("test", 80, 443)
        );
        assert_eq!(
            serde_json::to_value(Profile::Trial).unwrap(),
            json!("trial")
        );
    }

    #[test]
    fn unit_state_is_tagged() {
        let s = UnitState::Backoff { retry_in_ms: 2000 };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            json!({"state":"backoff","retry_in_ms":2000})
        );
        assert_eq!(
            serde_json::to_value(UnitState::Running).unwrap(),
            json!({"state":"running"})
        );
    }

    #[test]
    fn port_enums_are_snake_case() {
        assert_eq!(
            serde_json::to_value(PortMode::Preferred).unwrap(),
            json!("preferred")
        );
        assert_eq!(
            serde_json::to_value(PortStatus::ExternalNotListening).unwrap(),
            json!("external_not_listening")
        );
        assert_eq!(
            serde_json::to_value(HolderKind::BrewServices).unwrap(),
            json!("brew_services")
        );
        assert_eq!(
            serde_json::to_value(CheckStatus::Warn).unwrap(),
            json!("warn")
        );
    }

    #[test]
    fn site_kind_is_tagged() {
        let k = SiteKind::Proxy {
            target: "http://127.0.0.1:3000".into(),
        };
        assert_eq!(
            serde_json::to_value(&k).unwrap(),
            json!({"kind":"proxy","target":"http://127.0.0.1:3000"})
        );
    }
}
