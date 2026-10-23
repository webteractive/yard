//! Unit lifecycle against `yard-fake-unit`, with intervals scaled down to milliseconds.

mod support;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{FAKE, fast, free_port, held_listener};
use tokio::sync::broadcast::Receiver;
use yard_daemon::events::EventBus;
use yard_daemon::helper_client::HelperClient;
use yard_daemon::logsink::LogSink;
use yard_daemon::orphans;
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
        ports: vec![],
        reload: Reload::Restart,
        stop: libc::SIGTERM,
        inherit: vec![],
        stop_timeout: None,
        pid_file: None,
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
    spec.ports = vec![(p, Proto::Tcp)];
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
    let now_a = info(&h.sup, "a").unwrap();
    assert_eq!(now_a.pid, a, "unchanged unit keeps its process: {now_a:?}");
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

#[test]
fn a_held_listener_never_takes_a_port_given_out() {
    // More binds than the ephemeral range holds, so the OS sequence wraps past `given`.
    let given = free_port();
    for _ in 0..20_000 {
        let port = held_listener().local_addr().unwrap().port();
        assert_ne!(port, given, "a held listener took a port free_port gave out");
    }
}

#[tokio::test]
async fn foreign_listener_blocks_start_without_backoff() {
    let h = harness(fast());
    let held = held_listener();
    let p = held.local_addr().unwrap().port();
    let mut rx = h.bus.subscribe();
    let mut spec = listener("web", p);
    spec.ports = vec![(p, Proto::Tcp)];
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

#[tokio::test]
async fn apply_reports_the_units_it_started() {
    let h = harness(fast());
    let port = free_port();
    assert_eq!(h.sup.apply(vec![listener("a", port)]).await, BTreeSet::from(["a".to_string()]));
    wait_for(&h.sup, "a", running).await;
    assert!(h.sup.apply(vec![listener("a", port)]).await.is_empty(), "an unchanged unit keeps running");
    let other = free_port();
    assert_eq!(
        h.sup.apply(vec![listener("a", other)]).await,
        BTreeSet::from(["a".to_string()]),
        "a changed spec restarts"
    );
    h.sup.shutdown().await;
}

#[tokio::test]
async fn stop_sends_the_units_stop_signal() {
    let h = harness(fast());
    let port = free_port();
    let quit = UnitSpec {
        stop: libc::SIGQUIT,
        ..process("quit", FAKE, &["signals", &port.to_string()], Health::Tcp(port))
    };
    h.sup.apply(vec![quit]).await;
    wait_for(&h.sup, "quit", running).await;
    h.sup.shutdown().await;
    let log = std::fs::read_to_string(h.logs.path().join("quit.log")).unwrap_or_default();
    assert!(log.contains("got SIGQUIT"), "nginx stops gracefully on SIGQUIT: {log}");
}

#[tokio::test]
async fn fake_nginx_exits_cleanly_on_sigquit() {
    // Real nginx shuts down gracefully on SIGQUIT. A fake that died of the signal instead
    // would leave a crash report behind and stall every stop while ReportCrash writes it.
    let dir = tempfile::tempdir().unwrap();
    let nginx = dir.path().join("nginx");
    std::os::unix::fs::symlink(FAKE, &nginx).unwrap();
    let port = free_port();
    let conf = dir.path().join("nginx.conf");
    std::fs::write(&conf, format!("listen 127.0.0.1:{port};\n")).unwrap();
    let mut child =
        tokio::process::Command::new(&nginx).arg("-p").arg(dir.path()).arg("-c").arg(&conf).spawn().unwrap();
    let up = || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok();
    assert!(support::wait_until(up, Duration::from_secs(5)).await, "the fake nginx listens");
    // SAFETY: kill(2) on a child this test owns.
    unsafe { libc::kill(child.id().unwrap() as libc::pid_t, libc::SIGQUIT) };
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await.unwrap().unwrap();
    assert!(status.success(), "the fake nginx exits 0 on SIGQUIT: {status:?}");
}

#[tokio::test]
async fn a_unit_stopped_before_it_is_healthy_gets_sigterm() {
    // Nothing is in flight before the first health check passes, and a process stopped
    // that early may not have installed its SIGQUIT handler yet.
    let h = harness(fast());
    let port = free_port();
    let never = UnitSpec {
        stop: libc::SIGQUIT,
        ..process("early", FAKE, &["signals", &port.to_string()], Health::Tcp(free_port()))
    };
    h.sup.apply(vec![never]).await;
    // `signals` installs its handlers before it listens.
    assert!(support::wait_until(|| support::served(port), Duration::from_secs(5)).await);
    assert_eq!(info(&h.sup, "early").unwrap().state, UnitState::Starting);
    h.sup.shutdown().await;
    let log = std::fs::read_to_string(h.logs.path().join("early.log")).unwrap_or_default();
    assert!(log.contains("got SIGTERM"), "{log}");
}

#[tokio::test]
async fn fake_nginx_serves_every_inherited_socket() {
    // nginx serves all of `NGINX="fd;fd;"`: in the privileged profile that is http and
    // https, each on 127.0.0.1 and ::1, in whatever order the helper opened them.
    let (a, a_port) = support::inheritable_listener();
    let (b, b_port) = support::inheritable_listener();
    let dir = tempfile::tempdir().unwrap();
    let nginx = dir.path().join("nginx");
    std::os::unix::fs::symlink(FAKE, &nginx).unwrap();
    let conf = dir.path().join("nginx.conf");
    std::fs::write(&conf, "").unwrap();
    use std::os::fd::AsRawFd;
    let fds = format!("{};{};", a.as_raw_fd(), b.as_raw_fd());
    let mut child = tokio::process::Command::new(&nginx)
        .arg("-p")
        .arg(dir.path())
        .arg("-c")
        .arg(&conf)
        .env("NGINX", fds)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    for port in [a_port, b_port] {
        let served = || support::served(port);
        assert!(support::wait_until(served, Duration::from_secs(5)).await, "nothing serves {port}");
    }
    child.kill().await.unwrap();
}

#[tokio::test]
async fn every_port_is_probed_before_start() {
    let h = harness(fast());
    let (free, held) = (free_port(), held_listener());
    let held_port = held.local_addr().unwrap().port();
    let mut rx = h.bus.subscribe();
    let mut spec = listener("two", free);
    spec.ports = vec![(free, Proto::Tcp), (held_port, Proto::Tcp)];
    h.sup.apply(vec![spec]).await;

    let u = wait_for(&h.sup, "two", failed).await;
    let error = u.last_error.unwrap_or_default();
    assert!(error.contains("PORT_IN_USE"), "{error}");
    assert!(error.contains(&held_port.to_string()), "names the second port: {error}");
    assert!(error.contains(&std::process::id().to_string()), "names the holder: {error}");
    assert_eq!(u.restarts, 0);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let (mut conflicts, mut starts) = (0, 0);
    while let Ok(event) = rx.try_recv() {
        match event {
            Event::PortConflict { port, .. } => {
                assert_eq!(port, held_port);
                conflicts += 1;
            }
            Event::UnitStateChanged { state: UnitState::Starting | UnitState::Backoff { .. }, .. } => {
                starts += 1
            }
            _ => {}
        }
    }
    assert_eq!((conflicts, starts), (1, 0), "one check, no spawn, no backoff");
    assert!(!support::served(free), "the free port was never bound: nothing started");
    h.sup.shutdown().await;
}

#[tokio::test]
async fn unit_stop_timeout_overrides_default() {
    // The config would wait 10 s; the unit's own 300 ms is what applies.
    let h = harness(SupervisorConfig { stop_timeout: Duration::from_secs(10), ..fast() });
    let port = free_port();
    let stubborn = UnitSpec {
        stop_timeout: Some(Duration::from_millis(300)),
        ..process("stubborn", FAKE, &["stubborn", &port.to_string()], Health::Tcp(port))
    };
    h.sup.apply(vec![stubborn]).await;
    let pid = wait_for(&h.sup, "stubborn", running).await.pid.unwrap();

    let started = Instant::now();
    h.sup.shutdown().await;
    let took = started.elapsed();
    assert!(!alive(pid), "SIGKILL ended the process that ignores SIGTERM");
    assert!(took >= Duration::from_millis(300), "SIGTERM got its full timeout first: {took:?}");
    assert!(took < Duration::from_secs(5), "the unit's timeout applied, not the config's: {took:?}");
}

#[tokio::test]
async fn pid_file_follows_the_process() {
    let h = harness(fast());
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("run/svc.pid");
    let port = free_port();
    let svc = UnitSpec { pid_file: Some(pid_file.clone()), ..listener("svc", port) };
    h.sup.apply(vec![svc]).await;

    let pid = wait_for(&h.sup, "svc", running).await.pid.unwrap();
    let written = std::fs::read_to_string(&pid_file).unwrap();
    assert_eq!(written.trim(), pid.to_string());

    h.sup.shutdown().await;
    assert!(!pid_file.exists(), "the file goes with the process");
}

#[tokio::test]
async fn pid_file_is_removed_when_the_unit_crashes() {
    let h = harness(fast());
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("boom.pid");
    let boom = UnitSpec { pid_file: Some(pid_file.clone()), ..crasher("boom") };
    h.sup.apply(vec![boom]).await;
    wait_for(&h.sup, "boom", failed).await;
    assert!(!pid_file.exists(), "a failed unit has no process, so no pid file");
    h.sup.shutdown().await;
}

/// Starts `listen-tcp` detached, so launchd is its parent, and returns its pid.
async fn orphaned_listener(port: u16) -> u32 {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "\"$0\" listen-tcp \"$1\" >/dev/null 2>&1 & echo $!", FAKE, &port.to_string()])
        .output()
        .unwrap();
    let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert!(
        support::wait_until(|| support::ppid(pid) == 1, Duration::from_secs(5)).await,
        "parent is launchd"
    );
    assert!(support::wait_until(|| support::served(port), Duration::from_secs(5)).await, "it listens");
    pid
}

fn kill(pid: u32) {
    // SAFETY: kill(2) on a process this test started.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}

#[tokio::test]
async fn orphan_service_is_stopped_on_start() {
    let port = free_port();
    let pid = orphaned_listener(port).await;
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("svc.pid");
    std::fs::write(&pid_file, format!("{pid}\n")).unwrap();

    let stopped = orphans::stop(&pid_file, std::path::Path::new(FAKE), Duration::from_secs(2));
    assert_eq!(stopped, Some(pid));
    assert!(support::wait_until(|| !alive(pid), Duration::from_secs(5)).await, "the orphan exited");
    assert!(
        support::wait_until(|| yard_daemon::probe::is_free(port, Proto::Tcp), Duration::from_secs(2)).await,
        "its port is free again"
    );
    assert!(!pid_file.exists());
    kill(pid);
}

#[tokio::test]
async fn orphan_that_ignores_sigterm_is_killed_after_the_grace() {
    let port = free_port();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "\"$0\" stubborn \"$1\" >/dev/null 2>&1 & echo $!", FAKE, &port.to_string()])
        .output()
        .unwrap();
    let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert!(support::wait_until(|| support::ppid(pid) == 1, Duration::from_secs(5)).await);
    assert!(support::wait_until(|| support::served(port), Duration::from_secs(5)).await);
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("svc.pid");
    std::fs::write(&pid_file, pid.to_string()).unwrap();

    let started = Instant::now();
    let stopped = orphans::stop(&pid_file, std::path::Path::new(FAKE), Duration::from_millis(300));
    assert_eq!(stopped, Some(pid));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(support::wait_until(|| !alive(pid), Duration::from_secs(5)).await, "SIGKILL after the grace");
    kill(pid);
}

#[tokio::test]
async fn orphan_ignores_other_programs() {
    // A live process that is not the unit's program, even one whose parent is launchd.
    let port = free_port();
    let pid = orphaned_listener(port).await;
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("svc.pid");
    std::fs::write(&pid_file, pid.to_string()).unwrap();

    let stopped = orphans::stop(&pid_file, std::path::Path::new("/bin/sh"), Duration::from_millis(200));
    assert_eq!(stopped, None);
    assert!(alive(pid), "an unrelated process is left running");
    assert!(support::served(port));
    assert!(!pid_file.exists(), "a file that names someone else's process is stale");
    kill(pid);
}

#[tokio::test]
async fn orphan_that_still_has_a_parent_is_left_alone() {
    // Same program, but a live parent that is not launchd: a unit some yardd still supervises.
    let h = harness(fast());
    let port = free_port();
    h.sup.apply(vec![listener("svc", port)]).await;
    let pid = wait_for(&h.sup, "svc", running).await.pid.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("svc.pid");
    std::fs::write(&pid_file, pid.to_string()).unwrap();

    assert_eq!(orphans::stop(&pid_file, std::path::Path::new(FAKE), Duration::from_millis(200)), None);
    assert!(alive(pid));
    h.sup.shutdown().await;
}

#[test]
fn stale_pid_files_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let program = std::path::Path::new(FAKE);
    let grace = Duration::from_millis(100);

    let missing = dir.path().join("missing.pid");
    assert_eq!(orphans::stop(&missing, program, grace), None, "no file, nothing to do");

    let dead = dir.path().join("dead.pid");
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let gone = child.id();
    child.wait().unwrap();
    std::fs::write(&dead, gone.to_string()).unwrap();
    assert_eq!(orphans::stop(&dead, program, grace), None);
    assert!(!dead.exists(), "a dead pid is stale");

    let junk = dir.path().join("junk.pid");
    std::fs::write(&junk, "not a pid").unwrap();
    assert_eq!(orphans::stop(&junk, program, grace), None);
    assert!(!junk.exists());
}
