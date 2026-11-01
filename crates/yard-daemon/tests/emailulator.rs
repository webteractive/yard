//! M4d: asking emailulator about its LaunchAgent. The binary is the fake, linked as
//! `emailulator`: it reads `agent.json` (exit code on the first line) beside itself, and
//! hangs while `hang` is there. The real one and `~/Library/LaunchAgents` are never touched.

mod support;

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::{FAKE, agent_json};
use tempfile::TempDir;
use yard_daemon::emailulator::{Probe, probe};

fn fake_emailulator(dir: &Path) -> PathBuf {
    let bin = dir.join("emailulator");
    symlink(FAKE, &bin).unwrap();
    bin
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

// yard service add|start|stop|… emailulator: the agent is the only server (option B).

use support::daemon::run_job;

fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls")).unwrap_or_default()
}

async fn list_row(d: &Daemon) -> Option<Value> {
    let mut c = d.client().await;
    let list = c.call("services.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == "emailulator").cloned()
}

#[tokio::test]
async fn add_installs_the_agent() {
    let (d, dir, _, _) = with_emailulator(false).await;
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "services.add", json!({"engine": "emailulator"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(calls(&dir).lines().any(|l| l == "service install"), "{}", calls(&dir));
    let row = list_row(&d).await.expect("an emailulator row");
    assert_eq!(row["managed_by"]["kind"], "launch_agent", "{row}");
    assert_eq!(row["managed_by"]["label"], "com.webteractive.emailulator", "{row}");
    assert_eq!(row["state"], "running", "{row}");
    assert_eq!(row["console"], format!("http://mail.yard:{}", row_http(&d)), "{row}");
    let services = std::fs::read_to_string(d.fx.paths.services_toml()).unwrap();
    let bin = dir.join("emailulator");
    assert!(
        services.contains(&format!("[emailulator]\npath = {:?}", bin.display().to_string())),
        "{services}"
    );
    d.shutdown().await.unwrap();
}

/// nginx's http port in this test daemon.
fn row_http(d: &Daemon) -> String {
    Registry::load(&d.fx.paths).unwrap().entries["nginx-http"].port.to_string()
}

#[tokio::test]
async fn add_with_port_passes_addresses() {
    let (d, dir, _, _) = with_emailulator(false).await;
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) =
        run_job(&mut c, "services.add", json!({"engine": "emailulator", "port": 2525})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(
        calls(&dir).lines().any(|l| l == "service install -- --smtp-addr 127.0.0.1:2525"),
        "{}",
        calls(&dir)
    );
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_with_agent_runs_nothing() {
    let (d, dir, _, _) = with_emailulator(true).await;
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "services.add", json!({"engine": "emailulator"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("com.webteractive.emailulator") && l.contains("already")),
        "{lines:?}"
    );
    assert!(!calls(&dir).contains("install"), "{}", calls(&dir));
    assert!(std::fs::read_to_string(d.fx.paths.services_toml()).unwrap().contains("[emailulator]"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_without_binary_is_not_found() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let e = c.call("services.add", json!({"engine": "emailulator"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::EmailulatorNotFound);
    assert_eq!(e.hint.as_deref(), Some(yard_core::emailulator::INSTALLER));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn version_is_refused() {
    let (d, _dir, _, _) = with_emailulator(true).await;
    let d = d.start().await;
    let mut c = d.client().await;
    let e = c.call("services.add", json!({"engine": "emailulator", "version": "0.1"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument);
    assert_eq!(e.message, "emailulator has no versions; it updates itself");
    assert_eq!(e.hint.as_deref(), Some("emailulator self-update"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn every_lifecycle_command_is_service_external() {
    let (d, dir, _, _) = with_emailulator(true).await;
    let d = d.start().await;
    let mut c = d.client().await;
    for (method, params, hint) in [
        ("services.start", json!({"name": "emailulator"}), "emailulator service status"),
        ("services.stop", json!({"name": "emailulator"}), "emailulator service status"),
        ("services.restart", json!({"name": "emailulator"}), "emailulator service status"),
        ("services.port", json!({"name": "emailulator", "port": 2525}), "emailulator service status"),
        ("services.sleep.set", json!({"name": "emailulator", "after": "15m"}), "emailulator service status"),
        ("services.remove", json!({"name": "emailulator"}), "emailulator service uninstall"),
    ] {
        let e = c.call(method, params).await.unwrap_err();
        assert_eq!(e.code, yard_proto::ErrorCode::ServiceExternal, "{method}: {e:?}");
        assert_eq!(e.hint.as_deref(), Some(hint), "{method}");
        assert_eq!(
            e.message,
            "emailulator is run by its LaunchAgent com.webteractive.emailulator; Yard does not start or stop it"
        );
        let data = e.data.unwrap();
        assert_eq!(data["label"], "com.webteractive.emailulator", "{method}");
        assert_eq!(data["pid"], 4242, "{method}");
    }
    assert!(!calls(&dir).contains("install"), "{}", calls(&dir));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn logs_read_the_agent_log() {
    let (d, dir, smtp, http) = with_emailulator(false).await;
    let log = dir.join("server.log");
    std::fs::write(&log, "one\ntwo\nthree\n").unwrap();
    let json = agent_json(0, smtp, http).replace("/tmp/emailulator-server.log", &log.display().to_string());
    std::fs::write(dir.join("agent.json"), json).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let lines = c.call("services.logs", json!({"name": "emailulator", "lines": 2})).await.unwrap();
    assert_eq!(lines, json!(["two", "three"]));
    d.shutdown().await.unwrap();
}

// yard env prints emailulator's MAIL_* lines.

async fn env_blocks(d: &Daemon, params: Value) -> Vec<Value> {
    let mut c = d.client().await;
    c.call("env.print", params).await.unwrap().as_array().unwrap().clone()
}

fn mail_block(blocks: &[Value]) -> Option<Value> {
    blocks.iter().find(|b| b["instance"] == "emailulator").cloned()
}

fn mail_lines(port: u16) -> Value {
    json!([
        "MAIL_MAILER=smtp",
        "MAIL_HOST=127.0.0.1",
        format!("MAIL_PORT={port}"),
        "MAIL_USERNAME=null",
        "MAIL_PASSWORD=null",
        "MAIL_ENCRYPTION=null",
        "MAIL_SCHEME=null",
    ])
}

#[tokio::test]
async fn env_prints_mail_lines_for_the_agent() {
    let (d, _dir, smtp, _) = with_emailulator(true).await;
    asks_for_emailulator(&d);
    let d = d.start().await;
    for params in [json!({}), json!({"site": "shop"}), json!({"service": "emailulator"})] {
        let block = mail_block(&env_blocks(&d, params.clone()).await).unwrap_or_else(|| panic!("{params}"));
        assert_eq!(block["lines"], mail_lines(smtp), "{params}");
        assert_eq!(block["note"], Value::Null, "{params}");
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_warns_when_not_answering() {
    let (d, dir, smtp, http) = with_emailulator(false).await;
    let json = agent_json(3, smtp, http).replace("\"answering\":true", "\"answering\":false");
    std::fs::write(dir.join("agent.json"), json).unwrap();
    let d = d.start().await;
    let block = mail_block(&env_blocks(&d, json!({})).await).expect("an emailulator block");
    assert_eq!(block["lines"], mail_lines(smtp));
    assert_eq!(block["note"], "emailulator is not answering; see yard doctor");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_has_no_mail_lines_without_emailulator() {
    let d = prepared("").await.start().await;
    let blocks = env_blocks(&d, json!({})).await;
    assert_eq!(mail_block(&blocks), None, "{blocks:?}");
    let mut c = d.client().await;
    let e = c.call("env.print", json!({"service": "emailulator"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::ServiceNotFound);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_names_what_a_site_still_needs_for_mail() {
    let (d, _dir, _, _) = with_emailulator(false).await;
    asks_for_emailulator(&d);
    let d = d.start().await;
    let block = mail_block(&env_blocks(&d, json!({"site": "shop"})).await).expect("an emailulator block");
    assert_eq!(block["lines"], json!([]));
    assert_eq!(block["note"], "emailulator is not catching mail: yard service add emailulator");
    d.shutdown().await.unwrap();
}

// Final review fixes: an emailulator Yard cannot read is not one that went away, and asking
// it never holds the reconcile lock.

#[tokio::test]
async fn unreadable_version_keeps_last_state() {
    let dir = TempDir::new().unwrap();
    let bin = fake_emailulator(dir.path());
    std::fs::write(dir.path().join("agent.json"), agent_json(0, 41025, 48025)).unwrap();
    let earlier = probe(&bin, TIMEOUT, None).await;
    std::fs::write(dir.path().join("hang-version"), "").unwrap();
    let p = probe(&bin, Duration::from_secs(1), Some(&earlier)).await;
    assert_eq!(p.agent, earlier.agent);
    assert_eq!(p.version, earlier.version);
    assert!(p.status_error.as_deref().is_some_and(|e| e.contains("did not answer")), "{:?}", p.status_error);
}

#[tokio::test]
async fn unreadable_emailulator_is_no_need() {
    let (d, dir, _, _) = with_emailulator(false).await;
    std::fs::write(dir.join("agent.json"), "0\nnot json").unwrap();
    asks_for_emailulator(&d);
    let d = d.start().await;
    let needs = needs_of(&d, "shop").await;
    assert!(needs.is_null() || needs == json!([]), "{needs}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_hung_emailulator_does_not_hold_the_lock() {
    let (d, dir, _, _) = with_emailulator(true).await;
    let d = d.start().await;
    std::fs::write(dir.join("hang"), "").unwrap();
    let site = d.projects.path().join("other");
    std::fs::create_dir_all(&site).unwrap();
    let mut linker = d.client().await;
    let link = tokio::spawn(async move {
        linker.call("sites.link", json!({"name": "other", "path": site.display().to_string()})).await
    });
    // The link's reconcile is now asking emailulator, which takes its 5 s timeout.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut c = d.client().await;
    let started = Instant::now();
    c.call("status", Value::Null).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2), "status waited {:?}", started.elapsed());
    link.await.unwrap().unwrap();
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_crashed_agent_is_seen_within_a_scan() {
    let (d, dir, smtp, http) = with_emailulator(true).await;
    let d = d.start().await;
    let json = agent_json(3, smtp, http).replace("\"answering\":true", "\"answering\":false");
    std::fs::write(dir.join("agent.json"), json).unwrap();
    let mut c = d.client().await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let list = c.call("services.list", Value::Null).await.unwrap();
        let row = list.as_array().unwrap().iter().find(|s| s["name"] == "emailulator").cloned().unwrap();
        if row["state"].get("failed").is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "still {row}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    d.shutdown().await.unwrap();
}
