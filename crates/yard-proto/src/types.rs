//! Domain values that cross the wire, shared by core, daemon, CLI and app.

use crate::{ErrorCode, YardError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
}

impl Profile {
    /// The name `ports.toml`, the CLI and the API use.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Standard => "standard",
            Profile::Privileged => "privileged",
            Profile::Other => "unknown",
        }
    }

    pub fn http_port(self) -> u16 {
        match self {
            Profile::Standard => 8080,
            Profile::Privileged => 80,
            // Never in a registry: ports.toml and ports.profile read it strictly.
            Profile::Other => 8080,
        }
    }

    pub fn https_port(self) -> u16 {
        match self {
            Profile::Standard => 8443,
            Profile::Privileged => 443,
            Profile::Other => 8443,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteSource {
    Linked,
    Proxy,
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
    /// The `.yard` the site is served from, when it has one: the project's own, or its
    /// managed one in Yard's data root (`project_file_kind` says which).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_file: Option<String>,
    /// Whether `project_file` is the project's own `.yard` or the managed one in Yard's
    /// data root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_file_kind: Option<ProjectFileKind>,
    /// A managed `.yard` for the site that is not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored_managed_file: Option<IgnoredManagedFile>,
    /// What the `.yard` asks for that is not installed; the site serves a setup page
    /// until it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<Need>,
    /// Why the `.yard` cannot be read (`PROJECT_FILE_INVALID`); the site serves a setup
    /// page until it is fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_error: Option<YardError>,
    /// The Node the site root resolves to, for a site that uses Node; filled per request,
    /// so a `site_changed` event does not carry it. A missing version never puts the site
    /// on its setup page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<SiteNode>,
}

/// A site's Node: the spec its root resolves to and the installed version that meets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteNode {
    /// `None` only with `error`.
    pub spec: Option<String>,
    pub source: SettingSource,
    /// The `.yard`, `.nvmrc` or `.node-version` the spec comes from, or with `error` the
    /// one that cannot be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// `None` while nothing installed matches.
    pub version: Option<String>,
    /// Why the spec cannot be resolved (an unusable `.nvmrc`, a bad `.yard`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<YardError>,
}

/// One row of `node.list`: an installed version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub version: String,
    /// What the default spec runs.
    pub default: bool,
    /// Linked sites that run this version.
    pub sites: Vec<String>,
}

/// The newest release of a Node line on nodejs.org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRelease {
    pub version: String,
    /// The LTS codename, lowercased.
    pub lts: Option<String>,
}

/// `node.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeList {
    /// The default spec.
    pub default: String,
    /// Installed versions, newest first.
    pub versions: Vec<NodeInfo>,
    /// With `available`: the newest release of each line, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available: Vec<NodeRelease>,
}

impl SiteInfo {
    /// The site serves a setup page instead of the project.
    pub fn needs_setup(&self) -> bool {
        self.project_error.is_some() || !self.needs.is_empty()
    }
}

/// Where a site's `.yard` lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFileKind {
    /// `<project>/.yard`.
    Project,
    /// `<data root>/sites/<name>/.yard`.
    Managed,
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
}

/// A managed `.yard` that exists for a site but is not used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredManagedFile {
    pub file: String,
    pub because: IgnoredBecause,
    /// The directory the file names as its project, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IgnoredBecause {
    /// The project has its own `.yard`, which always wins.
    Committed,
    /// The file names another directory as its project, or none.
    OtherProject,
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
    /// A `[services]` entry with no instance yet. `engine` is `mysql`, `postgres`, `redis`
    /// or `minio`; `version` is the line (`8.4`), and `None` (`true` in the file) asks for
    /// the engine's default line.
    Service {
        engine: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// Something Yard cannot install itself (emailulator's binary): `command` is what the
    /// user runs (protocol 2.9).
    Manual {
        what: String,
        command: String,
    },
    /// A need a newer yardd reports that this client does not know, with or without data;
    /// never sent.
    #[serde(other)]
    Other,
}

/// How a service engine is named to people: `mysql` is `MySQL`. An engine Yard does not
/// know is shown as written.
pub fn engine_display(engine: &str) -> &str {
    match engine {
        "mysql" => "MySQL",
        "postgres" => "PostgreSQL",
        "redis" => "Redis",
        "minio" => "MinIO",
        other => other,
    }
}

impl Need {
    /// `PHP 7.3`, `the redis extension for PHP 7.3`.
    pub fn describe(&self) -> String {
        match self {
            Need::Php { php } => format!("PHP {php}"),
            Need::Extension { name, php } => format!("the {name} extension for PHP {php}"),
            Need::Service { engine, version: Some(v) } => format!("{} {v}", engine_display(engine)),
            Need::Service { engine, version: None } => engine_display(engine).to_string(),
            Need::Manual { what, .. } => what.clone(),
            Need::Other => "something a newer yardd asks for".to_string(),
        }
    }

    /// `PHP 7.3, the redis extension for PHP 7.3 and …`
    pub fn describe_all(needs: &[Need]) -> String {
        let items: Vec<String> = needs.iter().map(Need::describe).collect();
        match items.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => String::new(),
        }
    }

    /// The command that installs it.
    pub fn command(&self) -> String {
        match self {
            Need::Php { php } => format!("yard php install {php}"),
            Need::Extension { name, php } => format!("yard ext add {name} --php {php}"),
            Need::Service { engine, version: Some(v) } => format!("yard service add {engine} --version {v}"),
            Need::Service { engine, version: None } => format!("yard service add {engine}"),
            Need::Manual { command, .. } => command.clone(),
            Need::Other => "yard doctor".to_string(),
        }
    }

    /// Whether Yard can install it after a yes; a `Manual` need is the user's to run, and an
    /// `Other` one this client cannot name.
    pub fn installable(&self) -> bool {
        !matches!(self, Need::Manual { .. } | Need::Other)
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
    /// What the site served before `init` moved it here, when the name was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    /// The managed `.yard` that `--local` moved into the project as `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_managed: Option<String>,
    /// Where `replace` kept the managed `.yard` written for another directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UnitState {
    Stopped,
    Starting,
    Running,
    Backoff {
        retry_in_ms: u64,
    },
    Failed {
        last_lines: Vec<String>,
    },
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
            UnitState::Other => "unknown",
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
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HolderKind {
    Herd,
    BrewServices,
    Docker,
    /// DBngin.app's servers, which run from `/Users/Shared/DBngin/`.
    Dbngin,
    /// Postgres.app's servers, which run from `/Applications/Postgres.app/`.
    PostgresApp,
    Yard,
    Unknown,
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
    /// A value a newer yardd sends that this build does not know; never sent.
    #[serde(other)]
    Other,
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
    /// Service instances that sleep (protocol 2.8); they have no unit while they do.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sleeping: Vec<SleepingInstance>,
}

/// A service instance asleep, in `status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepingInstance {
    pub name: String,
    /// RFC 3339.
    pub since: String,
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

/// The state of a service instance (`services.list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", remote = "Self")]
pub enum ServiceState {
    Running,
    Starting,
    Stopped,
    Failed {
        last_lines: Vec<String>,
        /// Why it failed, when yardd knows: `PORT_IN_USE` naming the holder of a port it needs
        /// (hint `yard service port <i> <port>`), `FORMULA_MISSING` for a keg that is gone (hint
        /// `brew install|reinstall <formula>`), or `JOB_FAILED` with the supervisor's reason
        /// (`not healthy within 120 s of starting`). Added in protocol 2.5; absent from older
        /// daemons.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<YardError>,
    },
    /// A state a newer yardd reports that this client does not know, with or without data;
    /// never sent.
    #[serde(other)]
    Other,
}

impl Serialize for ServiceState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ServiceState::serialize(self, serializer)
    }
}

/// `#[serde(other)]` alone catches only an unknown unit variant (`"hibernating"`); an unknown
/// one with data (`{"hibernating": {…}}`) would fail the whole answer. A known state that does
/// not parse is still an error.
impl<'de> Deserialize<'de> for ServiceState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        ServiceState::deserialize(&value).or_else(|e| match &value {
            serde_json::Value::Object(map) if map.len() == 1 => {
                let tag = map.keys().next().cloned().unwrap_or_default();
                match ServiceState::deserialize(serde_json::Value::String(tag)) {
                    Ok(ServiceState::Other) => Ok(ServiceState::Other),
                    _ => Err(serde::de::Error::custom(e)),
                }
            }
            _ => Err(serde::de::Error::custom(e)),
        })
    }
}

impl ServiceState {
    /// The error a command that waited for `name` to start reports when it is `Failed`:
    /// the reason first, then the last line of its log. Its code and hint are the reason's
    /// (`PORT_IN_USE`, hint `yard service port …`), else `JOB_FAILED` with `yard service logs`.
    /// `None` for any other state.
    pub fn failure(&self, name: &str) -> Option<YardError> {
        let ServiceState::Failed { last_lines, reason } = self else { return None };
        let last = last_lines.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty());
        let message = match (reason, last) {
            (Some(r), _) if r.code == ErrorCode::PortInUse || r.code == ErrorCode::FormulaMissing => {
                r.message.clone()
            }
            (Some(r), Some(last)) => format!("{}; its log ends: {last}", r.message),
            (Some(r), None) => r.message.clone(),
            (None, Some(last)) => last.to_string(),
            (None, None) => "no output".to_string(),
        };
        let mut error = YardError::new(
            reason.as_ref().map_or(ErrorCode::JobFailed, |r| r.code),
            format!("{name} failed to start: {message}"),
        )
        .with_hint(
            reason
                .as_ref()
                .and_then(|r| r.hint.clone())
                .unwrap_or_else(|| format!("yard service logs {name}")),
        );
        let mut data = reason.as_ref().and_then(|r| r.data.clone()).unwrap_or_else(|| serde_json::json!({}));
        if let Some(map) = data.as_object_mut() {
            map.insert("last_lines".into(), serde_json::json!(last_lines));
        }
        error = error.with_data(data);
        Some(error)
    }
}

/// A login a service accepts. `user: None` means the service has no authentication;
/// `password: Some("")` means the account has no password. Yard stores no other password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user: Option<String>,
    pub password: Option<String>,
}

/// A registered port: the `ports.toml` entry's name and its number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortRef {
    pub name: String,
    pub port: u16,
}

/// One row of `services.list`: an instance in `services.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// `mysql-8.4`, `postgres-17`, `redis`, `minio`.
    pub name: String,
    pub engine: String,
    /// The line (`8.4`); `None` for engines that have one (`redis`, `minio`).
    pub version: Option<String>,
    pub formula: String,
    pub ports: Vec<PortRef>,
    pub state: ServiceState,
    /// Stopped by `yard service stop`, so yardd leaves it stopped.
    pub stopped_by_user: bool,
    pub account: Account,
    /// The web console's URL, when the engine has one (MinIO, and emailulator's
    /// `mail.<tld>`).
    pub console: Option<String>,
    /// Sites whose `.yard` lists this instance.
    pub used_by: Vec<String>,
    /// When it fell asleep (RFC 3339; protocol 2.8): set with `stopped` while it sleeps and
    /// with `starting` while it wakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleeping_since: Option<String>,
    /// How long it may go unused before it sleeps; `None` when it never sleeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_after_secs: Option<u64>,
    /// Where that setting comes from: `services_toml`, `yard_toml` or `default` (none).
    #[serde(default)]
    pub sleep_after_source: SettingSource,
    /// Who runs it (protocol 2.9): Yard, or emailulator's own LaunchAgent.
    #[serde(default)]
    pub managed_by: ServiceManager,
}

/// Who runs a service instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServiceManager {
    /// A yardd unit.
    #[default]
    Yard,
    /// A launchd agent Yard does not start or stop, such as emailulator's.
    LaunchAgent { label: String, pid: Option<u32> },
    /// A manager a newer yardd reports that this client does not know; never sent.
    #[serde(other)]
    Other,
}

/// One block of `env.print`: the `.env` lines for an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvBlock {
    pub instance: String,
    pub lines: Vec<String>,
    /// Something to tell the reader, such as a missing instance or a clash with another block.
    pub note: Option<String>,
}

/// One formula in a source tap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFormula {
    pub name: String,
    pub trusted: bool,
    pub installed: bool,
}

/// One row of `sources.list`: a Homebrew tap Yard installs from, and what brew trusts in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub tap: String,
    pub tapped: bool,
    pub trusted: bool,
    pub formulae: Vec<SourceFormula>,
}

/// Where a setting of a site (`sites.info`) or a service instance comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    ProjectFile,
    /// The site's managed `.yard`, in Yard's data root.
    ManagedFile,
    SitesToml,
    #[default]
    Default,
    /// `yard.toml` (`service_sleep_after`, protocol 2.8).
    YardToml,
    /// The instance's entry in `services.toml` (`sleep_after`, protocol 2.8).
    ServicesToml,
    /// The nearest `.nvmrc` (a site's Node).
    Nvmrc,
    /// The nearest `.node-version` (a site's Node).
    NodeVersionFile,
    /// A source a newer yardd reports that this client does not know; never sent.
    #[serde(other)]
    Other,
}

/// A `[services]` entry of a site's `.yard`, with the instance that meets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteService {
    pub engine: String,
    /// The line the `.yard` asks for; `None` for `true`.
    pub want: Option<String>,
    /// The instance that meets it, `None` while it is missing.
    pub service: Option<ServiceInfo>,
}

/// `sites.info`: a site, where each of its settings comes from, and its services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteDetail {
    pub site: SiteInfo,
    /// The full PHP version, `8.4.3`, when the site runs PHP and it is installed.
    pub php_full: Option<String>,
    /// `php`, `docroot` and `secure`.
    pub sources: BTreeMap<String, SettingSource>,
    pub services: Vec<SiteService>,
}

/// The answer to a method that runs as a job: its progress and result arrive as
/// `job_progress` and `job_finished` events on the `jobs` topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStarted {
    pub job_id: String,
}

/// The service a tunnel runs through (tunnels spec §7.1). Only cloudflared is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelProvider {
    #[default]
    Cloudflared,
    /// A provider a newer yardd reports that this client does not know; never sent.
    #[serde(other)]
    Other,
}

/// A quick tunnel has a random URL and lives in yardd's memory; a named one has a hostname
/// on the user's domain and lives in `sites.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    Quick,
    Named,
    /// A kind a newer yardd reports that this client does not know; never sent.
    #[serde(other)]
    Other,
}

/// The tunnel's unit state, refined by cloudflared's `/ready`: `connecting` is running but
/// not yet connected to Cloudflare's edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Connecting,
    Ready,
    Backoff,
    Failed,
    Stopped,
    /// A state a newer yardd reports that this client does not know; never sent.
    #[serde(other)]
    Other,
}

/// The Cloudflare tunnel behind a named tunnel. `created_by_yard` decides whether
/// `yard untunnel` deletes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelRef {
    pub name: String,
    pub id: String,
    pub created_by_yard: bool,
}

/// One tunnel (`tunnels.list`, the `tunnels.add` job's result, `tunnel_changed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub site: String,
    pub provider: TunnelProvider,
    pub kind: TunnelKind,
    /// `None` while a quick tunnel has no hostname yet.
    pub url: Option<String>,
    /// Named tunnels only.
    pub host: Option<String>,
    /// Named tunnels only.
    pub tunnel: Option<TunnelRef>,
    pub state: TunnelState,
    /// `tunnel-<site>`.
    pub unit: String,
    /// nginx's loopback listener for the tunnel.
    pub origin_port: u16,
    /// cloudflared's metrics server, which answers `/ready`.
    pub metrics_port: u16,
    pub last_error: Option<String>,
}

/// A DNS record Yard made and cannot delete: the user deletes it in Cloudflare's dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    pub host: String,
    /// `<tunnel id>.cfargotunnel.com`.
    pub target: String,
}

/// The `tunnels.remove` job's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelRemoved {
    pub site: String,
    pub deleted_tunnel: bool,
    pub dns_left: Vec<DnsRecord>,
}

/// A domain set up with `yard tunnels domain add`; `logged_in` is whether its certificate
/// exists (Yard never reads it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelDomain {
    pub provider: TunnelProvider,
    pub domain: String,
    pub logged_in: bool,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manual_need_is_tagged() {
        let need = Need::Manual { what: "emailulator".into(), command: "curl … | sh".into() };
        assert_eq!(
            serde_json::to_value(&need).unwrap(),
            json!({"kind":"manual","what":"emailulator","command":"curl … | sh"})
        );
        assert_eq!(need.describe(), "emailulator");
        assert_eq!(need.command(), "curl … | sh");
    }

    #[test]
    fn an_unknown_need_is_other_and_not_installable() {
        let later = json!({"kind":"keychain","item":"login"});
        let need = serde_json::from_value::<Need>(later).unwrap();
        assert_eq!(need, Need::Other);
        assert!(!need.installable());
        assert_eq!(need.describe(), "something a newer yardd asks for");
        assert_eq!(need.command(), "yard doctor");
    }

    #[test]
    fn an_unknown_service_manager_is_other() {
        let later = json!({"kind":"system_daemon","label":"com.example","pid":9});
        assert_eq!(serde_json::from_value::<ServiceManager>(later).unwrap(), ServiceManager::Other);
        assert_eq!(
            serde_json::from_value::<ServiceManager>(json!({"kind":"yard"})).unwrap(),
            ServiceManager::Yard
        );
    }

    #[test]
    fn manual_need_is_not_installable() {
        assert!(!Need::Manual { what: "emailulator".into(), command: "sh".into() }.installable());
        assert!(Need::Php { php: "7.3".into() }.installable());
        assert!(Need::Service { engine: "emailulator".into(), version: None }.installable());
    }

    fn mysql_info() -> serde_json::Value {
        json!({"name":"mysql-8.4","engine":"mysql","version":"8.4","formula":"mysql@8.4",
            "ports":[{"name":"mysql-8.4","port":3307}],"state":"running","stopped_by_user":false,
            "account":{"user":"root","password":""},"console":null,"used_by":[]})
    }

    #[test]
    fn service_info_without_manager_is_yard() {
        // What a 2.8 daemon sent: no `managed_by`.
        let info: ServiceInfo = serde_json::from_value(mysql_info()).unwrap();
        assert_eq!(info.managed_by, ServiceManager::Yard);
        assert_eq!(info.console, None);
    }

    #[test]
    fn launch_agent_manager_round_trips() {
        let manager =
            ServiceManager::LaunchAgent { label: "com.webteractive.emailulator".into(), pid: Some(3261) };
        let v = serde_json::to_value(&manager).unwrap();
        assert_eq!(v, json!({"kind":"launch_agent","label":"com.webteractive.emailulator","pid":3261}));
        assert_eq!(serde_json::from_value::<ServiceManager>(v).unwrap(), manager);
        assert_eq!(serde_json::to_value(ServiceManager::Yard).unwrap(), json!({"kind":"yard"}));
        let mut info = mysql_info();
        info["managed_by"] = json!({"kind":"launch_agent","label":"l","pid":null});
        let info: ServiceInfo = serde_json::from_value(info).unwrap();
        assert_eq!(info.managed_by, ServiceManager::LaunchAgent { label: "l".into(), pid: None });
    }

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
    fn setting_source_names() {
        assert_eq!(serde_json::to_value(SettingSource::YardToml).unwrap(), json!("yard_toml"));
        assert_eq!(serde_json::to_value(SettingSource::ServicesToml).unwrap(), json!("services_toml"));
        assert_eq!(
            serde_json::from_value::<SettingSource>(json!("yard_toml")).unwrap(),
            SettingSource::YardToml
        );
        assert_eq!(
            serde_json::from_value::<SettingSource>(json!("services_toml")).unwrap(),
            SettingSource::ServicesToml
        );
        assert_eq!(SettingSource::default(), SettingSource::Default);
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
    fn site_node_round_trips_and_an_old_answer_has_none() {
        // What a 2.6 daemon sent: no node.
        let v = json!({"name":"blog","host":"blog.yard","url":"http://blog.yard:8080/","source":"linked",
            "kind":"proxy","target":"http://127.0.0.1:3000","secure":false});
        let mut site = serde_json::from_value::<SiteInfo>(v).unwrap();
        assert_eq!(site.node, None);
        site.node = Some(SiteNode {
            spec: Some("16.14.0".into()),
            source: SettingSource::Nvmrc,
            file: Some("/code/maker/.nvmrc".into()),
            version: None,
            error: None,
        });
        let v = serde_json::to_value(&site).unwrap();
        assert_eq!(
            v["node"],
            json!({"spec":"16.14.0","source":"nvmrc","file":"/code/maker/.nvmrc","version":null})
        );
        assert_eq!(serde_json::from_value::<SiteInfo>(v).unwrap(), site);
        assert_eq!(serde_json::to_value(SettingSource::NodeVersionFile).unwrap(), json!("node_version_file"));
        let list = NodeList {
            default: "22".into(),
            versions: vec![NodeInfo { version: "22.18.0".into(), default: true, sites: vec![] }],
            available: vec![],
        };
        let v = serde_json::to_value(&list).unwrap();
        assert!(v.get("available").is_none(), "{v}");
        assert_eq!(serde_json::from_value::<NodeList>(v).unwrap(), list);
        let r = NodeRelease { version: "24.5.0".into(), lts: Some("krypton".into()) };
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"version":"24.5.0","lts":"krypton"}));
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

    #[test]
    fn need_service_is_tagged() {
        let n = Need::Service { engine: "mysql".into(), version: Some("8.4".into()) };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({"kind":"service","engine":"mysql","version":"8.4"})
        );
        let redis = Need::Service { engine: "redis".into(), version: None };
        let v = serde_json::to_value(&redis).unwrap();
        // `true` in the `.yard` means the engine's default line: no version on the wire.
        assert_eq!(v, json!({"kind":"service","engine":"redis"}));
        assert_eq!(serde_json::from_value::<Need>(v).unwrap(), redis);
    }

    #[test]
    fn need_service_describe_and_command() {
        let mysql = Need::Service { engine: "mysql".into(), version: Some("8.4".into()) };
        assert_eq!(mysql.describe(), "MySQL 8.4");
        assert_eq!(mysql.command(), "yard service add mysql --version 8.4");
        let redis = Need::Service { engine: "redis".into(), version: None };
        assert_eq!((redis.describe(), redis.command()), ("Redis".into(), "yard service add redis".into()));
        let names: Vec<String> = ["mysql", "postgres", "redis", "minio"]
            .iter()
            .map(|e| Need::Service { engine: (*e).into(), version: None }.describe())
            .collect();
        assert_eq!(names, ["MySQL", "PostgreSQL", "Redis", "MinIO"]);
    }

    #[test]
    fn service_state_failed_carries_lines() {
        let s = ServiceState::Failed { last_lines: vec!["boom".into()], reason: None };
        assert_eq!(serde_json::to_value(&s).unwrap(), json!({"failed":{"last_lines":["boom"]}}));
        assert_eq!(serde_json::to_value(ServiceState::Running).unwrap(), json!("running"));
        assert_eq!(serde_json::to_value(ServiceState::Starting).unwrap(), json!("starting"));
        assert_eq!(serde_json::to_value(ServiceState::Stopped).unwrap(), json!("stopped"));
        assert_eq!(
            serde_json::from_value::<ServiceState>(json!({"failed":{"last_lines":[]}})).unwrap(),
            ServiceState::Failed { last_lines: vec![], reason: None },
            "an older daemon sends no reason"
        );
    }

    #[test]
    fn service_state_failed_carries_its_reason() {
        let reason = YardError::new(ErrorCode::PortInUse, "port 3307 is held by mysqld (pid 77034, DBngin)")
            .with_hint("yard service port mysql-8.4 <port>")
            .with_data(json!({"port": 3307}));
        let s = ServiceState::Failed { last_lines: vec![], reason: Some(reason.clone()) };
        let wire = serde_json::to_value(&s).unwrap();
        assert_eq!(wire["failed"]["reason"]["code"], "PORT_IN_USE");
        assert_eq!(serde_json::from_value::<ServiceState>(wire).unwrap(), s);

        let e = s.failure("mysql-8.4").unwrap();
        assert_eq!(e.code, ErrorCode::PortInUse);
        assert_eq!(e.message, "mysql-8.4 failed to start: port 3307 is held by mysqld (pid 77034, DBngin)");
        assert_eq!(e.hint.as_deref(), Some("yard service port mysql-8.4 <port>"));
        assert_eq!(e.data, Some(json!({"port": 3307, "last_lines": []})));
    }

    #[test]
    fn a_failure_names_the_reason_then_the_log() {
        let lines = vec!["InnoDB: upgrading".to_string(), "  ".to_string()];
        let timeout = YardError::new(ErrorCode::JobFailed, "not healthy within 120 s of starting")
            .with_hint("yard service logs mysql-8.4");
        let with = ServiceState::Failed { last_lines: lines.clone(), reason: Some(timeout) };
        let e = with.failure("mysql-8.4").unwrap();
        assert_eq!(e.code, ErrorCode::JobFailed);
        assert_eq!(
            e.message,
            "mysql-8.4 failed to start: not healthy within 120 s of starting; its log ends: InnoDB: upgrading"
        );
        // An older daemon gives no reason: the last line is the message, as before.
        let without = ServiceState::Failed { last_lines: lines, reason: None };
        let e = without.failure("mysql-8.4").unwrap();
        assert_eq!(e.message, "mysql-8.4 failed to start: InnoDB: upgrading");
        assert_eq!(e.hint.as_deref(), Some("yard service logs mysql-8.4"));
        let empty = ServiceState::Failed { last_lines: vec![], reason: None };
        assert_eq!(empty.failure("redis").unwrap().message, "redis failed to start: no output");
        assert_eq!(ServiceState::Running.failure("redis"), None);
    }

    #[test]
    fn service_info_round_trips() {
        let info = ServiceInfo {
            name: "mysql-8.4".into(),
            engine: "mysql".into(),
            version: Some("8.4".into()),
            formula: "mysql@8.4".into(),
            ports: vec![PortRef { name: "mysql-8.4".into(), port: 3306 }],
            state: ServiceState::Running,
            stopped_by_user: false,
            account: Account { user: Some("root".into()), password: Some(String::new()) },
            console: None,
            used_by: vec!["blog".into()],
            sleeping_since: None,
            sleep_after_secs: None,
            sleep_after_source: SettingSource::Default,
            managed_by: ServiceManager::Yard,
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["state"], "running");
        assert_eq!(v["account"], json!({"user":"root","password":""}));
        assert_eq!(v["ports"], json!([{"name":"mysql-8.4","port":3306}]));
        assert_eq!(serde_json::from_value::<ServiceInfo>(v).unwrap(), info);
    }

    #[test]
    fn a_2_7_service_info_still_decodes() {
        let v = json!({"name":"redis","engine":"redis","version":null,"formula":"redis",
            "ports":[{"name":"redis","port":6380}],"state":"running","stopped_by_user":false,
            "account":{"user":null,"password":null},"console":null,"used_by":[]});
        let info: ServiceInfo = serde_json::from_value(v).unwrap();
        assert_eq!(info.sleeping_since, None);
        assert_eq!(info.sleep_after_secs, None);
        assert_eq!(info.sleep_after_source, SettingSource::Default);
    }

    #[test]
    fn a_sleeping_instance_is_stopped_with_since() {
        let v = json!({"name":"redis","engine":"redis","version":null,"formula":"redis",
            "ports":[],"state":"stopped","stopped_by_user":false,
            "account":{"user":null,"password":null},"console":null,"used_by":[]});
        let mut info: ServiceInfo = serde_json::from_value(v).unwrap();
        info.sleeping_since = Some("2026-10-26T14:02:00Z".into());
        info.sleep_after_secs = Some(900);
        info.sleep_after_source = SettingSource::YardToml;
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["state"], "stopped");
        assert_eq!(v["sleeping_since"], "2026-10-26T14:02:00Z");
        assert_eq!(v["sleep_after_secs"], 900);
        assert_eq!(v["sleep_after_source"], "yard_toml");
        assert_eq!(serde_json::from_value::<ServiceInfo>(v).unwrap(), info);
    }

    #[test]
    fn an_unknown_unit_state_is_other() {
        assert_eq!(
            serde_json::from_value::<ServiceState>(json!("hibernating")).unwrap(),
            ServiceState::Other
        );
    }

    #[test]
    fn an_unknown_state_with_data_is_other() {
        let later = json!({"hibernating": {"until": "2026-10-27T09:00:00Z"}});
        assert_eq!(serde_json::from_value::<ServiceState>(later).unwrap(), ServiceState::Other);
    }

    #[test]
    fn a_malformed_known_state_is_still_an_error() {
        assert!(serde_json::from_value::<ServiceState>(json!({"failed": {"last_lines": 3}})).is_err());
    }

    #[test]
    fn status_lists_sleeping_instances_only_when_there_are_some() {
        let s = SleepingInstance { name: "redis".into(), since: "2026-10-26T14:02:00Z".into() };
        assert_eq!(serde_json::to_value(&s).unwrap(), json!({"name":"redis","since":"2026-10-26T14:02:00Z"}));
    }

    #[test]
    fn holder_kinds_for_database_apps() {
        assert_eq!(serde_json::to_value(HolderKind::Dbngin).unwrap(), json!("dbngin"));
        assert_eq!(serde_json::to_value(HolderKind::PostgresApp).unwrap(), json!("postgres_app"));
    }

    #[test]
    fn site_detail_and_sources_round_trip() {
        let site: SiteInfo = serde_json::from_value(json!({"name":"a","host":"a.yard",
            "url":"http://a.yard:8080/","source":"linked","kind":"proxy","target":"http://127.0.0.1:3000"}))
        .unwrap();
        let detail = SiteDetail {
            site,
            php_full: Some("8.4.3".into()),
            sources: BTreeMap::from([("php".to_string(), SettingSource::ProjectFile)]),
            services: vec![SiteService { engine: "redis".into(), want: None, service: None }],
        };
        let v = serde_json::to_value(&detail).unwrap();
        assert_eq!(v["sources"], json!({"php":"project_file"}));
        assert_eq!(serde_json::to_value(SettingSource::SitesToml).unwrap(), json!("sites_toml"));
        assert_eq!(serde_json::to_value(SettingSource::Default).unwrap(), json!("default"));
        assert_eq!(serde_json::to_value(SettingSource::ManagedFile).unwrap(), json!("managed_file"));
        assert_eq!(serde_json::from_value::<SiteDetail>(v).unwrap(), detail);
        // A source a newer yardd adds is read, not refused.
        assert_eq!(serde_json::from_value::<SettingSource>(json!("volta")).unwrap(), SettingSource::Other);

        let src = SourceInfo {
            tap: "minio/stable".into(),
            tapped: true,
            trusted: false,
            formulae: vec![SourceFormula { name: "minio".into(), trusted: false, installed: true }],
        };
        assert_eq!(serde_json::from_value::<SourceInfo>(serde_json::to_value(&src).unwrap()).unwrap(), src);
        let env =
            EnvBlock { instance: "redis".into(), lines: vec!["REDIS_HOST=127.0.0.1".into()], note: None };
        assert_eq!(serde_json::from_value::<EnvBlock>(serde_json::to_value(&env).unwrap()).unwrap(), env);
    }

    pub(crate) fn sample_tunnel() -> TunnelInfo {
        TunnelInfo {
            site: "blog".into(),
            provider: TunnelProvider::Cloudflared,
            kind: TunnelKind::Quick,
            url: None,
            host: None,
            tunnel: None,
            state: TunnelState::Connecting,
            unit: "tunnel-blog".into(),
            origin_port: 33011,
            metrics_port: 33012,
            last_error: None,
        }
    }

    #[test]
    fn tunnel_wire_names() {
        use crate::ErrorCode;
        assert_eq!(serde_json::to_value(TunnelProvider::Cloudflared).unwrap(), json!("cloudflared"));
        assert_eq!(serde_json::to_value(TunnelState::Connecting).unwrap(), json!("connecting"));
        assert_eq!(serde_json::from_value::<TunnelState>(json!("paused")).unwrap(), TunnelState::Other);
        assert_eq!(serde_json::from_value::<TunnelKind>(json!("funnel")).unwrap(), TunnelKind::Other);
        assert_eq!(serde_json::from_value::<TunnelProvider>(json!("ngrok")).unwrap(), TunnelProvider::Other);
        assert_eq!(serde_json::to_value(ErrorCode::SiteTunnelled).unwrap(), json!("SITE_TUNNELLED"));
        assert_eq!(serde_json::to_value(ErrorCode::TunnelDnsExists).unwrap(), json!("TUNNEL_DNS_EXISTS"));
        let t = sample_tunnel();
        assert_eq!(serde_json::from_value::<TunnelInfo>(serde_json::to_value(&t).unwrap()).unwrap(), t);
    }

    /// A newer yardd may send a value this build does not know; the answer still decodes, with
    /// the value read as its fallback (M6 design §6.3).
    #[test]
    fn unknown_values_read_as_their_fallback() {
        use crate::Event;
        let e: YardError =
            serde_json::from_value(json!({"code":"SOMETHING_NEW","message":"m","hint":"h"})).unwrap();
        assert_eq!((e.code, e.message.as_str(), e.hint.as_deref()), (ErrorCode::Other, "m", Some("h")));
        let ev: Event = serde_json::from_value(json!({"type":"service_moved","x":1})).unwrap();
        assert_eq!(ev, Event::Unknown);
        let u: UnitInfo = serde_json::from_value(
            json!({"name":"nginx","state":"paused","pid":null,"restarts":0,"last_error":null}),
        )
        .unwrap();
        assert_eq!(u.state, UnitState::Other);
        assert_eq!(serde_json::from_value::<PortStatus>(json!("sleeping")).unwrap(), PortStatus::Other);
        assert_eq!(serde_json::from_value::<HolderKind>(json!("orbstack")).unwrap(), HolderKind::Other);
        assert_eq!(serde_json::from_value::<PortMode>(json!("leased")).unwrap(), PortMode::Other);
        assert_eq!(serde_json::from_value::<CheckStatus>(json!("skipped")).unwrap(), CheckStatus::Other);
        assert_eq!(serde_json::from_value::<SiteSource>(json!("static")).unwrap(), SiteSource::Other);
        assert_eq!(
            serde_json::from_value::<ProjectFileKind>(json!("global")).unwrap(),
            ProjectFileKind::Other
        );
        assert_eq!(serde_json::from_value::<IgnoredBecause>(json!("stale")).unwrap(), IgnoredBecause::Other);
        assert_eq!(serde_json::from_value::<Profile>(json!("turbo")).unwrap(), Profile::Other);
        let site: SiteInfo = serde_json::from_value(json!({
            "name":"blog","host":"blog.yard","url":"http://blog.yard:8080/","source":"linked",
            "kind":"static","path":"/p"
        }))
        .unwrap();
        assert_eq!(site.kind, SiteKind::Other);
    }

    #[test]
    fn a_known_variant_that_does_not_parse_still_fails() {
        let backoff = json!({"name":"x","state":"backoff","pid":null,"restarts":0,"last_error":null});
        assert!(serde_json::from_value::<UnitInfo>(backoff).is_err());
    }

    /// A fallback is for reading a newer daemon's answers, never for the daemon's own inputs.
    #[test]
    fn strict_profile_and_mode_refuse_an_unknown_name() {
        #[derive(Debug, Deserialize)]
        struct T {
            #[serde(deserialize_with = "crate::strict::profile")]
            p: Profile,
            #[serde(deserialize_with = "crate::strict::port_mode")]
            m: PortMode,
        }
        let ok: T = serde_json::from_value(json!({"p":"trial","m":"pinned"})).unwrap();
        assert_eq!((ok.p, ok.m), (Profile::Standard, PortMode::Pinned));
        let e = serde_json::from_value::<T>(json!({"p":"turbo","m":"pinned"})).unwrap_err().to_string();
        assert!(e.contains("unknown variant `turbo`"), "{e}");
        let e = serde_json::from_value::<T>(json!({"p":"standard","m":"other"})).unwrap_err().to_string();
        assert!(e.contains("unknown variant `other`"), "{e}");
    }
}
