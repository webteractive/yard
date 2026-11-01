//! M4d: asking emailulator about its LaunchAgent. The binary is the fake, linked as
//! `emailulator`: it reads `agent.json` (exit code on the first line) beside itself, and
//! hangs while `hang` is there. The real one and `~/Library/LaunchAgents` are never touched.

mod support;

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::FAKE;
use tempfile::TempDir;
use yard_daemon::emailulator::{Probe, probe};

fn fake_emailulator(dir: &Path) -> PathBuf {
    let bin = dir.join("emailulator");
    symlink(FAKE, &bin).unwrap();
    bin
}

/// `service status --json` as emailulator 0.1.3 prints it, on `smtp` and `http`.
fn agent_json(exit: i32, smtp: u16, http: u16) -> String {
    format!(
        r#"{exit}
{{"label":"com.webteractive.emailulator","installed":true,"answering":true,"loaded":true,"running":true,
"pid":4242,"smtp_addr":"127.0.0.1:{smtp}","http_addr":"127.0.0.1:{http}","tls":false,
"log":"/tmp/emailulator-server.log"}}"#
    )
}

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn probe_reads_agent() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    std::fs::write(dir.path().join("agent.json"), agent_json(0, 41025, 48025)).unwrap();
    let p = probe(&bin, TIMEOUT, None).await;
    assert_eq!(p.bin, bin);
    assert_eq!(p.version, Some((0, 1, 3)));
    assert_eq!(p.status_error, None);
    let agent = p.agent.expect("an agent");
    assert_eq!(agent.smtp.port(), 41025);
    assert_eq!(agent.http.port(), 48025);
    assert_eq!(agent.pid, Some(4242));
}

#[tokio::test]
async fn probe_without_agent() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    let p = probe(&bin, TIMEOUT, None).await;
    assert_eq!((p.agent, p.status_error), (None, None));
    assert_eq!(p.version, Some((0, 1, 3)));
}

#[tokio::test]
async fn old_emailulator_has_no_agent() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    std::fs::write(dir.path().join("version"), "emailulator 0.1.2 (fake, 2026-10-01)\n").unwrap();
    std::fs::write(dir.path().join("agent.json"), agent_json(0, 41025, 48025)).unwrap();
    let p = probe(&bin, TIMEOUT, None).await;
    assert_eq!(p.version, Some((0, 1, 2)));
    assert_eq!((p.agent, p.status_error), (None, None));
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert!(!calls.contains("service"), "service status ran: {calls}");
}

#[tokio::test]
async fn probe_asks_the_version_then_the_agent() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    probe(&bin, TIMEOUT, None).await;
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert!(calls.lines().any(|l| l == "--version"), "{calls}");
    assert!(calls.lines().any(|l| l == "service status --json"), "{calls}");
}

#[tokio::test]
async fn garbage_status_keeps_last_state() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    std::fs::write(dir.path().join("agent.json"), agent_json(0, 41025, 48025)).unwrap();
    let earlier = probe(&bin, TIMEOUT, None).await;
    std::fs::write(dir.path().join("agent.json"), "0\nnot json").unwrap();
    let p = probe(&bin, TIMEOUT, Some(&earlier)).await;
    assert_eq!(p.agent, earlier.agent);
    assert!(p.status_error.as_deref().is_some_and(|e| e.contains("not JSON")), "{:?}", p.status_error);
}

#[tokio::test]
async fn status_timeout_keeps_last_state() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    std::fs::write(dir.path().join("agent.json"), agent_json(0, 41025, 48025)).unwrap();
    let earlier: Probe = probe(&bin, TIMEOUT, None).await;
    assert!(earlier.agent.is_some());
    std::fs::write(dir.path().join("hang"), "").unwrap();
    let started = Instant::now();
    let p = probe(&bin, Duration::from_secs(1), Some(&earlier)).await;
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
    assert_eq!(p.agent, earlier.agent);
    assert!(p.status_error.as_deref().is_some_and(|e| e.contains("did not answer")), "{:?}", p.status_error);
}

// Reconcile points the registry and mail.<tld> at the agent.

use support::daemon::{Daemon, prepared};
use support::{free_port, wait_until};
use yard_core::ports::Registry;
use yard_proto::PortMode;

/// A daemon whose home has the fake emailulator, with an agent on two free ports when
/// `agent` is set. Returns the daemon, the fake's directory and the agent's ports.
async fn with_emailulator(agent: bool) -> (Daemon, PathBuf, u16, u16) {
    let d = prepared("").await;
    let dir = d.fx.add_emailulator();
    let (smtp, http) = (free_port(), free_port());
    if agent {
        std::fs::write(dir.join("agent.json"), agent_json(0, smtp, http)).unwrap();
    }
    (d, dir, smtp, http)
}

fn entry(d: &Daemon, unit: &str) -> Option<(u16, PortMode)> {
    let registry = Registry::load(&d.fx.paths).unwrap();
    registry.entries.get(unit).map(|e| (e.port, e.mode))
}

fn mail_conf(d: &Daemon) -> Option<String> {
    std::fs::read_to_string(d.fx.paths.nginx_service("emailulator")).ok()
}

#[tokio::test]
async fn agent_ports_become_external_entries() {
    let (d, _dir, smtp, http) = with_emailulator(true).await;
    let d = d.start().await;
    assert_eq!(entry(&d, "emailulator-smtp"), Some((smtp, PortMode::External)));
    assert_eq!(entry(&d, "emailulator-http"), Some((http, PortMode::External)));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn mail_host_follows_the_agent() {
    let (d, _dir, _smtp, http) = with_emailulator(true).await;
    let d = d.start().await;
    let conf = mail_conf(&d).expect("nginx/services/emailulator.conf");
    assert!(conf.contains(&format!("proxy_set_header Host 127.0.0.1:{http};")), "{conf}");
    assert!(conf.contains("server_name mail.yard *.mail.yard;"), "{conf}");
    // The file its block includes exists, or nginx would refuse the config.
    assert!(d.fx.paths.nginx_override("mail").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn no_emailulator_no_mail_host() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    assert_eq!(mail_conf(&d), None);
    assert_eq!(entry(&d, "emailulator-smtp"), None);
    assert_eq!(entry(&d, "emailulator-http"), None);
    assert!(d.fx.paths.nginx_site("blog").exists(), "other sites are served");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn no_agent_no_mail_host() {
    let (d, _dir, _, _) = with_emailulator(false).await;
    let d = d.start().await;
    assert_eq!(mail_conf(&d), None);
    assert_eq!(entry(&d, "emailulator-smtp"), None);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn site_named_mail_wins() {
    let (d, _dir, _, _) = with_emailulator(true).await;
    d.project("mail");
    let d = d.start().await;
    assert!(d.fx.paths.nginx_site("mail").exists());
    assert_eq!(mail_conf(&d), None);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn agent_port_change_is_followed() {
    let (d, dir, _, _) = with_emailulator(true).await;
    let d = d.start().await;
    let (smtp, http) = (free_port(), free_port());
    std::fs::write(dir.join("agent.json"), agent_json(0, smtp, http)).unwrap();
    let moved = || {
        entry(&d, "emailulator-smtp") == Some((smtp, PortMode::External))
            && mail_conf(&d).is_some_and(|c| c.contains(&format!("Host 127.0.0.1:{http};")))
    };
    assert!(
        wait_until(moved, Duration::from_secs(10)).await,
        "{:?}\n{:?}",
        entry(&d, "emailulator-smtp"),
        mail_conf(&d)
    );
    // And an agent that goes away takes the entries and the host with it.
    std::fs::remove_file(dir.join("agent.json")).unwrap();
    let gone = || entry(&d, "emailulator-smtp").is_none() && mail_conf(&d).is_none();
    assert!(wait_until(gone, Duration::from_secs(10)).await);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn secure_by_default_issues_mail_cert() {
    let (d, _dir, _, _) = with_emailulator(true).await;
    std::fs::write(d.fx.paths.yard_toml(), "secure_by_default = true\n").unwrap();
    let d = d.start().await;
    let cert = d.fx.paths.services_certs_dir().join("emailulator.pem");
    assert!(cert.exists(), "{}", cert.display());
    let conf = mail_conf(&d).unwrap();
    assert!(conf.contains("services/emailulator.pem"), "{conf}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn recorded_path_is_used_first() {
    let d = prepared("").await;
    let tools = TempDir::new().unwrap();
    let bin = fake_emailulator(tools.path());
    let (smtp, http) = (free_port(), free_port());
    std::fs::write(tools.path().join("agent.json"), agent_json(0, smtp, http)).unwrap();
    std::fs::write(
        d.fx.paths.services_toml(),
        format!("[emailulator]\npath = {:?}\n", bin.display().to_string()),
    )
    .unwrap();
    let d = d.start().await;
    assert_eq!(entry(&d, "emailulator-smtp"), Some((smtp, PortMode::External)));
    d.shutdown().await.unwrap();
}

// A .yard that asks for emailulator.

use serde_json::{Value, json};

async fn needs_of(d: &Daemon, name: &str) -> Value {
    let mut c = d.client().await;
    let list = c.call("sites.list", Value::Null).await.unwrap();
    let site = list.as_array().unwrap().iter().find(|s| s["name"] == name).cloned();
    site.unwrap_or_else(|| panic!("{list}"))["needs"].clone()
}

fn asks_for_emailulator(d: &Daemon) {
    let dir = d.project("shop");
    std::fs::write(dir.join(".yard"), "php = \"8.4\"\n\n[services]\nemailulator = true\n").unwrap();
}

#[tokio::test]
async fn emailulator_need_when_missing() {
    let d = prepared("").await;
    asks_for_emailulator(&d);
    let d = d.start().await;
    let installer = yard_core::emailulator::INSTALLER;
    assert_eq!(
        needs_of(&d, "shop").await,
        json!([{"kind": "manual", "what": "emailulator", "command": installer}])
    );
    let page = std::fs::read_to_string(d.fx.paths.nginx_setup_page("shop")).unwrap();
    assert!(page.contains("install.sh"), "{page}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn found_without_agent_needs_the_service() {
    let (d, _dir, _, _) = with_emailulator(false).await;
    asks_for_emailulator(&d);
    let d = d.start().await;
    assert_eq!(needs_of(&d, "shop").await, json!([{"kind": "service", "engine": "emailulator"}]));
    let page = std::fs::read_to_string(d.fx.paths.nginx_setup_page("shop")).unwrap();
    assert!(page.contains("yard init"), "{page}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn agent_meets_emailulator() {
    let (d, _dir, _, _) = with_emailulator(true).await;
    asks_for_emailulator(&d);
    let d = d.start().await;
    assert_eq!(needs_of(&d, "shop").await, Value::Null);
    assert!(!d.fx.paths.nginx_setup_page("shop").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn stopped_owner_still_meets() {
    let (d, dir, smtp, http) = with_emailulator(false).await;
    // Loaded, so launchd restarts it, but neither running nor answering: exit 3.
    let quiet = agent_json(3, smtp, http)
        .replace("\"answering\":true", "\"answering\":false")
        .replace("\"running\":true", "\"running\":false");
    std::fs::write(dir.join("agent.json"), quiet).unwrap();
    asks_for_emailulator(&d);
    let d = d.start().await;
    assert_eq!(needs_of(&d, "shop").await, Value::Null);
    d.shutdown().await.unwrap();
}
