//! emailulator, the mail catcher Yard points at but never installs or runs (emailulator
//! spec, option B): where its binary is, what `--version` and `service status --json`
//! print, who serves mail, and the `.env` lines for it. Running the binary is yardd's.

use serde::Deserialize;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

/// emailulator's own installer, the fix whenever its binary is missing.
pub const INSTALLER: &str =
    "curl -fsSL https://raw.githubusercontent.com/webteractive/emailulator/main/install.sh | sh";
/// The first release with `service status --json`.
pub const MIN_VERSION: (u32, u32, u32) = (0, 1, 3);
/// The registry entries for its two ports.
pub const SMTP_UNIT: &str = "emailulator-smtp";
pub const HTTP_UNIT: &str = "emailulator-http";
/// emailulator's own defaults.
pub const SMTP_USUAL: u16 = 1025;
pub const HTTP_USUAL: u16 = 8025;
/// `mail.<tld>` serves its web UI.
pub const MAIL_HOST_LABEL: &str = "mail";
/// The `[services]` key and the instance name.
pub const NAME: &str = "emailulator";

/// The binary: the path the CLI resolved on the user's PATH and yardd recorded, else where
/// the installer puts it. The first that `is_exec` accepts wins.
pub fn locate(
    recorded: Option<&Path>,
    home: &Path,
    is_exec: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, YardError> {
    let candidates: Vec<PathBuf> =
        recorded.map(Path::to_path_buf).into_iter().chain([home.join(".local/bin/emailulator")]).collect();
    if let Some(found) = candidates.iter().find(|p| is_exec(p)) {
        return Ok(found.clone());
    }
    let searched: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    Err(YardError::new(ErrorCode::EmailulatorNotFound, "emailulator is not installed")
        .with_hint(INSTALLER)
        .with_data(json!({ "searched": searched })))
}

/// `emailulator 0.1.3 (823e513, 2026-10-20)` is `(0, 1, 3)`.
pub fn parse_version(stdout: &str) -> Option<(u32, u32, u32)> {
    let mut words = stdout.split_whitespace();
    if words.next()? != "emailulator" {
        return None;
    }
    let mut parts = words.next()?.split('.').map(|p| p.parse::<u32>().ok());
    match (parts.next()?, parts.next()?, parts.next()?, parts.next()) {
        (Some(major), Some(minor), Some(patch), None) => Some((major, minor, patch)),
        _ => None,
    }
}

/// emailulator's LaunchAgent, as `service status --json` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub label: String,
    pub installed: bool,
    pub loaded: bool,
    pub running: bool,
    pub answering: bool,
    pub pid: Option<u32>,
    /// Where to connect: a wildcard listener is reached on 127.0.0.1.
    pub smtp: SocketAddr,
    pub http: SocketAddr,
    pub tls: bool,
    pub log: Option<PathBuf>,
}

/// The fields Yard reads, by name; the rest are ignored.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStatus {
    label: String,
    installed: bool,
    loaded: bool,
    running: bool,
    answering: bool,
    pid: Option<u32>,
    smtp_addr: Option<String>,
    http_addr: Option<String>,
    tls: bool,
    log: Option<PathBuf>,
}

/// `service status --json` exits 0 (answering), 3 (installed, not answering) or 4 (not
/// installed). Anything else, or JSON without both addresses, is `Err` with the reason.
pub fn parse_status(exit: i32, stdout: &str) -> Result<Option<AgentStatus>, String> {
    match exit {
        4 => return Ok(None),
        0 | 3 => {}
        other => return Err(format!("it exited {other}")),
    }
    let raw: RawStatus = serde_json::from_str(stdout).map_err(|e| format!("its answer is not JSON: {e}"))?;
    let addr = |field: &str, value: Option<String>| -> Result<SocketAddr, String> {
        let value = value.ok_or_else(|| format!("its answer has no {field}"))?;
        let mut addr: SocketAddr =
            value.parse().map_err(|_| format!("{field} {value:?} is not an address"))?;
        if addr.ip().is_unspecified() {
            addr.set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
        }
        Ok(addr)
    };
    Ok(Some(AgentStatus {
        smtp: addr("smtp_addr", raw.smtp_addr)?,
        http: addr("http_addr", raw.http_addr)?,
        label: raw.label,
        installed: raw.installed,
        loaded: raw.loaded,
        running: raw.running,
        answering: raw.answering,
        pid: raw.pid,
        tls: raw.tls,
        log: raw.log,
    }))
}

/// Who serves mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Agent(AgentStatus),
    /// Yard's own unit (option A only, not built).
    Yard,
    Nobody,
}

/// An agent that is installed and loaded owns mail even while launchd restarts it, so
/// nothing else takes its ports in the gap. `yard_instance` is option A's unit, so always
/// `false` today.
pub fn owner(agent: Option<&AgentStatus>, yard_instance: bool) -> Owner {
    match agent {
        Some(a) if a.installed && a.loaded => Owner::Agent(a.clone()),
        _ if yard_instance => Owner::Yard,
        _ => Owner::Nobody,
    }
}

/// The `.env` lines for Laravel. The `null`s override credentials and TLS copied from
/// another environment, which emailulator on loopback neither needs nor offers.
pub fn env_lines(host: &str, smtp: u16) -> Vec<(String, String)> {
    [
        ("MAIL_MAILER", "smtp".to_string()),
        ("MAIL_HOST", host.to_string()),
        ("MAIL_PORT", smtp.to_string()),
        ("MAIL_USERNAME", "null".to_string()),
        ("MAIL_PASSWORD", "null".to_string()),
        ("MAIL_ENCRYPTION", "null".to_string()),
        ("MAIL_SCHEME", "null".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What this machine's agent printed on 2026-11-01 (the same as on 2026-10-23).
    const STATUS: &str = r#"{
  "label": "com.webteractive.emailulator",
  "installed": true,
  "answering": true,
  "plist": "/Users/me/Library/LaunchAgents/com.webteractive.emailulator.plist",
  "log": "/Users/me/Library/Logs/emailulator/server.log",
  "program": ["/Users/me/.local/bin/emailulator", "run", "--smtp-addr", "127.0.0.1:1025", "--http-addr", "127.0.0.1:8025"],
  "smtp_addr": "127.0.0.1:1025",
  "http_addr": "127.0.0.1:8025",
  "tls": false,
  "loaded": true,
  "running": true,
  "pid": 3261,
  "last_exit_code": "(never exited)"
}"#;

    fn agent(loaded: bool, running: bool) -> AgentStatus {
        let mut a = parse_status(0, STATUS).unwrap().unwrap();
        a.loaded = loaded;
        a.running = running;
        a
    }

    #[test]
    fn locate_prefers_recorded_then_local_bin() {
        let home = Path::new("/Users/me");
        let recorded = Path::new("/opt/tools/emailulator");
        let local = home.join(".local/bin/emailulator");
        let all = |_: &Path| true;
        assert_eq!(locate(Some(recorded), home, &all).unwrap(), recorded);
        let only_local = |p: &Path| p == local;
        assert_eq!(locate(Some(recorded), home, &only_local).unwrap(), local);
        assert_eq!(locate(None, home, &only_local).unwrap(), local);
    }

    #[test]
    fn locate_not_found_names_installer() {
        let e = locate(Some(Path::new("/opt/tools/emailulator")), Path::new("/Users/me"), &|_| false)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::EmailulatorNotFound);
        assert_eq!(e.message, "emailulator is not installed");
        assert_eq!(e.hint.as_deref(), Some(INSTALLER));
        let searched = &e.data.unwrap()["searched"];
        assert_eq!(
            searched,
            &serde_json::json!(["/opt/tools/emailulator", "/Users/me/.local/bin/emailulator"])
        );
    }

    #[test]
    fn parse_version_forms() {
        assert_eq!(parse_version("emailulator 0.1.3 (823e513, 2026-10-20)\n"), Some((0, 1, 3)));
        assert_eq!(parse_version("emailulator 1.10.0"), Some((1, 10, 0)));
        assert_eq!(parse_version("emailulator"), None);
        assert_eq!(parse_version("something 0.1"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn parse_status_installed() {
        let a = parse_status(0, STATUS).unwrap().unwrap();
        assert_eq!(a.label, "com.webteractive.emailulator");
        assert!(a.installed && a.loaded && a.running && a.answering && !a.tls);
        assert_eq!(a.pid, Some(3261));
        assert_eq!(a.smtp, "127.0.0.1:1025".parse().unwrap());
        assert_eq!(a.http, "127.0.0.1:8025".parse().unwrap());
        assert_eq!(a.log, Some(PathBuf::from("/Users/me/Library/Logs/emailulator/server.log")));
        // Installed but not answering exits 3 with the same JSON.
        let quiet = STATUS.replace("\"answering\": true", "\"answering\": false");
        assert!(!parse_status(3, &quiet).unwrap().unwrap().answering);
    }

    #[test]
    fn parse_status_not_installed() {
        assert_eq!(parse_status(4, "").unwrap(), None);
        assert_eq!(parse_status(4, "{\"installed\": false}").unwrap(), None);
    }

    #[test]
    fn parse_status_wildcard_is_loopback() {
        let any = STATUS.replace("\"127.0.0.1:1025\"", "\"0.0.0.0:2525\"");
        assert_eq!(parse_status(0, &any).unwrap().unwrap().smtp, "127.0.0.1:2525".parse().unwrap());
    }

    #[test]
    fn parse_status_missing_flags_read_as_false() {
        let text = r#"{"label":"l","installed":true,"smtp_addr":"127.0.0.1:1","http_addr":"127.0.0.1:2"}"#;
        let a = parse_status(3, text).unwrap().unwrap();
        assert!(!a.loaded && !a.running && !a.answering);
        assert_eq!(a.pid, None);
    }

    #[test]
    fn parse_status_garbage_is_err() {
        assert!(parse_status(0, "not json").is_err());
        assert!(parse_status(1, STATUS).is_err());
        assert!(parse_status(0, r#"{"label":"l","installed":true}"#).is_err(), "no addresses");
    }

    #[test]
    fn loaded_agent_owns_mail_even_when_not_running() {
        let a = agent(true, false);
        assert_eq!(owner(Some(&a), false), Owner::Agent(a.clone()));
        assert_eq!(owner(Some(&a), true), Owner::Agent(a));
    }

    #[test]
    fn unloaded_agent_does_not_own_mail() {
        let a = agent(false, false);
        assert_eq!(owner(Some(&a), false), Owner::Nobody);
        assert_eq!(owner(Some(&a), true), Owner::Yard);
        assert_eq!(owner(None, false), Owner::Nobody);
    }

    #[test]
    fn env_lines_exact() {
        let lines: Vec<String> =
            env_lines("127.0.0.1", 1025).into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(
            lines,
            [
                "MAIL_MAILER=smtp",
                "MAIL_HOST=127.0.0.1",
                "MAIL_PORT=1025",
                "MAIL_USERNAME=null",
                "MAIL_PASSWORD=null",
                "MAIL_ENCRYPTION=null",
                "MAIL_SCHEME=null",
            ]
        );
    }
}
