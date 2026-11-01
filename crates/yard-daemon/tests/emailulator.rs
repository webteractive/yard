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
