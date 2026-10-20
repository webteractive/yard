//! Unit lifecycle against `yard-fake-unit`, with intervals scaled down to milliseconds.

mod support;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{FAKE, fast, free_port};
use tokio::sync::broadcast::Receiver;
use yard_daemon::events::EventBus;
use yard_daemon::helper_client::HelperClient;
use yard_daemon::logsink::LogSink;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::probe::Proto;
use yard_daemon::supervisor::{Health, Reload, Supervisor, SupervisorConfig, UnitSpec};
use yard_proto::{Event, UnitInfo, UnitState};

struct Harness {
    sup: Supervisor,
    bus: EventBus,
    logs: tempfile::TempDir,
}

fn harness(cfg: SupervisorConfig) -> Harness {
    let logs = tempfile::tempdir().unwrap();
    let bus = EventBus::new();
    let sockets = Arc::new(PrivilegedSockets::new(HelperClient::new("/tmp/yard-test-no-helper/helper.sock")));
    Harness { sup: Supervisor::new(cfg, logs.path().to_path_buf(), bus.clone(), sockets), bus, logs }
}

fn process(name: &str, program: &str, args: &[&str], health: Health) -> UnitSpec {
    UnitSpec {
        name: name.into(),
        program: PathBuf::from(program),
        args: args.iter().map(OsString::from).collect(),
        env: vec![],
        health,
        port: None,
        reload: Reload::Restart,
        inherit: vec![],
    }
}

fn listener(name: &str, port: u16) -> UnitSpec {
    process(name, FAKE, &["listen-tcp", &port.to_string()], Health::Tcp(port))
}

fn crasher(name: &str) -> UnitSpec {
    process(name, "/bin/sh", &["-c", "seq 1 60; exit 1"], Health::Tcp(free_port()))
}

fn info(sup: &Supervisor, name: &str) -> Option<UnitInfo> {
    sup.list().into_iter().find(|u| u.name == name)
}

async fn wait_for(sup: &Supervisor, name: &str, done: impl Fn(&UnitInfo) -> bool) -> UnitInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(u) = info(sup, name).filter(|u| done(u)) {
            return u;
        }
        assert!(Instant::now() < deadline, "timed out; last seen {:?}", info(sup, name));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn running(u: &UnitInfo) -> bool {
    u.state == UnitState::Running
}

fn failed(u: &UnitInfo) -> bool {
    matches!(u.state, UnitState::Failed { .. })
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Unit state changes for `unit` seen on `rx` until `until` matches or 10 s pass.
async fn states_until(
    rx: &mut Receiver<Event>,
    unit: &str,
    until: impl Fn(&UnitState) -> bool,
) -> Vec<UnitState> {
    let mut seen = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        if let Event::UnitStateChanged { unit: u, state } = event
            && u == unit
        {
            let stop = until(&state);
            seen.push(state);
            if stop {
                break;
            }
        }
    }
    seen
}

#[tokio::test]
async fn healthy_unit_reaches_running() {
    let h = harness(fast());
    let p = free_port();
    let mut spec = listener("web", p);
    spec.port = Some((p, Proto::Tcp));
    h.sup.apply(vec![spec]).await;

    let u = wait_for(&h.sup, "web", running).await;
    assert!(u.pid.is_some_and(alive));
    assert_eq!(u.restarts, 0);
    h.sup.shutdown().await;
}

#[tokio::test]
async fn crashing_unit_backs_off_then_fails_with_last_lines() {
    let h = harness(fast());
    h.sup.apply(vec![crasher("boom")]).await;

    let u = wait_for(&h.sup, "boom", failed).await;
    let UnitState::Failed { last_lines } = u.state else { unreachable!() };
    assert_eq!(last_lines.len(), 50);
    assert_eq!(last_lines.last().map(String::as_str), Some("60"));
    assert_eq!(u.restarts, 4, "five failures: four restarts, then failed");
    assert!(u.last_error.is_some_and(|e| e.contains("exited")));
    assert!(h.logs.path().join("boom.log").exists());
    h.sup.shutdown().await;
}

#[tokio::test]
async fn restart_clears_failed() {
    let h = harness(fast());
    h.sup.apply(vec![crasher("boom")]).await;
    wait_for(&h.sup, "boom", failed).await;

    let mut rx = h.bus.subscribe();
    h.sup.restart("boom").await.unwrap();
    let states = states_until(&mut rx, "boom", |s| *s == UnitState::Starting).await;
    assert_eq!(states.last(), Some(&UnitState::Starting), "{states:?}");
    assert!(h.sup.restart("nope").await.is_err());
    h.sup.shutdown().await;
}

#[tokio::test]
async fn hung_unit_failing_health_is_killed_and_counted() {
    let h = harness(SupervisorConfig { start_grace: Duration::from_millis(100), ..fast() });
    h.sup.apply(vec![process("stuck", FAKE, &["hang"], Health::Tcp(free_port()))]).await;

    let mut pids = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let u = loop {
        let u = info(&h.sup, "stuck").unwrap();
        pids.extend(u.pid);
        if failed(&u) {
            break u;
        }
        assert!(Instant::now() < deadline, "never failed: {u:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(u.restarts, 4);
    assert!(!pids.is_empty());
    assert!(pids.iter().all(|p| !alive(*p)), "every hung process was killed: {pids:?}");
    h.sup.shutdown().await;
}

#[tokio::test]
async fn apply_restarts_only_changed_units() {
    let h = harness(fast());
    let (pa, pb) = (free_port(), free_port());
    h.sup.apply(vec![listener("a", pa), listener("b", pb)]).await;
    let a = wait_for(&h.sup, "a", running).await.pid;
    let b = wait_for(&h.sup, "b", running).await.pid;

    let pb2 = free_port();
    h.sup.apply(vec![listener("a", pa), listener("b", pb2)]).await;
    let b2 = wait_for(&h.sup, "b", |u| running(u) && u.pid != b).await;
    assert_eq!(info(&h.sup, "a").unwrap().pid, a, "unchanged unit keeps its process");
    assert!(b.is_some_and(|pid| !alive(pid)));
    assert!(b2.pid.is_some_and(alive));

    h.sup.apply(vec![listener("a", pa)]).await;
    assert!(info(&h.sup, "b").is_none(), "removed unit leaves the list");
    assert!(b2.pid.is_some_and(|pid| !alive(pid)));
    h.sup.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_children() {
    let h = harness(fast());
    h.sup.apply(vec![listener("a", free_port()), listener("b", free_port())]).await;
    let mut pids = vec![];
    for name in ["a", "b"] {
        pids.extend(wait_for(&h.sup, name, running).await.pid);
    }

    h.sup.shutdown().await;
    assert!(pids.iter().all(|p| !alive(*p)), "{pids:?}");
    assert!(h.sup.list().iter().all(|u| u.state == UnitState::Stopped && u.pid.is_none()));
}

#[tokio::test]
async fn foreign_listener_blocks_start_without_backoff() {
    let h = harness(fast());
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = held.local_addr().unwrap().port();
    let mut rx = h.bus.subscribe();
    let mut spec = listener("web", p);
    spec.port = Some((p, Proto::Tcp));
    h.sup.apply(vec![spec]).await;

    let u = wait_for(&h.sup, "web", failed).await;
    let error = u.last_error.unwrap_or_default();
    assert!(error.contains("PORT_IN_USE"), "{error}");
    assert!(error.contains(&std::process::id().to_string()), "names our pid: {error}");
    assert_eq!(u.restarts, 0);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let (mut conflicts, mut starts) = (0, 0);
    while let Ok(event) = rx.try_recv() {
        match event {
            Event::PortConflict { port, holder, .. } => {
                assert_eq!(port, p);
                assert_eq!(holder.map(|h| h.pid), Some(std::process::id()));
                conflicts += 1;
            }
            Event::UnitStateChanged { state: UnitState::Starting | UnitState::Backoff { .. }, .. } => {
                starts += 1
            }
            _ => {}
        }
    }
    assert_eq!((conflicts, starts), (1, 0), "one check, no spawn, no backoff");
    h.sup.shutdown().await;
}

#[test]
fn log_rotates_and_keeps_n() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unit.log");
    let mut sink = LogSink::open(&path, 100, 2, "unit".into(), EventBus::new()).unwrap();
    for i in 0..60 {
        sink.write_line(&format!("line number {i:03}"));
    }
    assert!(path.exists());
    assert!(dir.path().join("unit.log.1").exists());
    assert!(dir.path().join("unit.log.2").exists());
    assert!(!dir.path().join("unit.log.3").exists());
    assert!(std::fs::metadata(&path).unwrap().len() <= 100);
    let tail = sink.tail();
    assert_eq!(tail.len(), 50);
    assert_eq!(tail.last().map(String::as_str), Some("line number 059"));
}

#[tokio::test]
async fn state_changes_are_emitted() {
    let h = harness(fast());
    let mut rx = h.bus.subscribe();
    h.sup.apply(vec![listener("web", free_port())]).await;

    let states = states_until(&mut rx, "web", |s| *s == UnitState::Running).await;
    assert_eq!(states, vec![UnitState::Starting, UnitState::Running]);

    h.sup.shutdown().await;
    let states = states_until(&mut rx, "web", |s| *s == UnitState::Stopped).await;
    assert_eq!(states, vec![UnitState::Stopped]);
}

#[tokio::test]
async fn units_do_not_inherit_open_sockets() {
    let h = harness(fast());
    let (leaked, p) = support::inheritable_listener();
    h.sup.apply(vec![process("stuck", FAKE, &["hang"], Health::Tcp(free_port()))]).await;
    wait_for(&h.sup, "stuck", |u| u.pid.is_some()).await;

    drop(leaked);
    // Other tests spawn too, and their children hold the socket until exec; only a
    // unit that kept it holds it for good.
    let freed =
        support::wait_until(|| yard_daemon::probe::is_free(p, Proto::Tcp), Duration::from_secs(2)).await;
    assert!(freed, "the unit holds a socket it inherited");
    h.sup.shutdown().await;
}

#[tokio::test]
async fn reload_sends_the_units_signal() {
    let h = harness(fast());
    let port = free_port();
    // The fake ignores SIGHUP, as nginx does when it reloads.
    let hup = UnitSpec { reload: Reload::Signal(libc::SIGHUP), ..listener("hup", port) };
    h.sup.apply(vec![hup]).await;
    let before = wait_for(&h.sup, "hup", running).await.pid;
    h.sup.reload("hup").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(wait_for(&h.sup, "hup", running).await.pid, before, "a signal, not a restart");

    let other = free_port();
    h.sup.apply(vec![listener("plain", other)]).await;
    let first = wait_for(&h.sup, "plain", running).await.pid;
    h.sup.reload("plain").await.unwrap();
    let again = wait_for(&h.sup, "plain", |u| running(u) && u.pid != first).await;
    assert_ne!(again.pid, first, "Reload::Restart restarts");
    h.sup.shutdown().await;
}
