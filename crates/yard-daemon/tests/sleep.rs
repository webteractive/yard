//! On-demand services (M4e): an idle instance falls asleep, and wakes on use. The fake
//! clock (`SleepConfig.second` = 1 ms) makes `"60s"` last 60 ms.

mod support;

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::daemon::{Client, Daemon, prepared};
use support::{free_port, wait_until};

/// A `ports.toml` line pinning `unit` to `port`.
fn pin(unit: &str, port: u16) -> String {
    format!("\"{unit}\" = {{ port = {port}, mode = \"pinned\" }}\n")
}

/// A daemon with one fake `redis` instance on `port`, and `yard_toml` as `yard.toml`.
async fn with_redis(port: u16, yard_toml: &str, instance_extra: &str) -> Daemon {
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    if !instance_extra.is_empty() {
        let file = d.fx.paths.services_toml();
        let text = std::fs::read_to_string(&file)
            .unwrap()
            .replace("formula = \"redis\"\n", &format!("formula = \"redis\"\n{instance_extra}"));
        std::fs::write(&file, text).unwrap();
    }
    std::fs::write(d.fx.paths.yard_toml(), yard_toml).unwrap();
    d
}

async fn redis(c: &mut Client) -> Value {
    let list = c.call("services.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == "redis").cloned().expect("redis is listed")
}

/// Polls `services.list` until `done` holds for redis, for up to `within`.
async fn redis_until(c: &mut Client, within: Duration, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let info = redis(c).await;
        if done(&info) {
            return info;
        }
        assert!(tokio::time::Instant::now() < deadline, "redis never got there; last {info}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn running(info: &Value) -> bool {
    info["state"] == "running"
}

fn asleep(info: &Value) -> bool {
    info["state"] == "stopped" && info["sleeping_since"].is_string()
}

fn write_yard(dir: &Path, text: &str) {
    std::fs::write(dir.join(".yard"), text).unwrap();
}

#[tokio::test]
async fn nothing_sleeps_without_a_setting() {
    let d = with_redis(free_port(), "", "").await.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let info = redis(&mut c).await;
    assert!(running(&info), "{info}");
    assert!(info.get("sleeping_since").is_none(), "{info}");
    assert_eq!(info["sleep_after_source"], "default");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_idle_instance_falls_asleep() {
    let d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "").await;
    let ports_before = std::fs::read(d.fx.paths.ports_toml()).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    let ports_running = std::fs::read(d.fx.paths.ports_toml()).unwrap();
    let info = redis_until(&mut c, Duration::from_secs(2), asleep).await;
    assert_eq!(info["stopped_by_user"], false);
    assert_eq!(info["sleep_after_secs"], 60);
    assert_eq!(info["sleep_after_source"], "yard_toml");
    let pid_file = d.fx.paths.service_pid("redis");
    assert!(wait_until(|| !pid_file.exists(), Duration::from_secs(2)).await, "the process is gone");
    assert_eq!(std::fs::read(d.fx.paths.ports_toml()).unwrap(), ports_running, "ports.toml is untouched");
    assert_eq!(ports_running, ports_before, "the pinned port was there from the start");
    let services = std::fs::read_to_string(d.fx.paths.services_toml()).unwrap();
    assert!(!services.contains("stopped"), "sleep is never written: {services}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_instance_key_off_keeps_it_awake() {
    let d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "sleep_after = \"off\"\n").await;
    let d = d.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let info = redis(&mut c).await;
    assert!(running(&info), "{info}");
    assert_eq!(info["sleep_after_source"], "services_toml");
    assert!(info.get("sleep_after_secs").is_none(), "{info}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_held_connection_keeps_it_awake() {
    let port = free_port();
    let d = with_redis(port, "service_sleep_after = \"60s\"\n", "").await.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    let held = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let info = redis(&mut c).await;
    assert!(running(&info), "a client keeps it awake: {info}");
    drop(held);
    redis_until(&mut c, Duration::from_secs(2), asleep).await;
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn status_and_doctor_report_sleep() {
    let d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "").await.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), asleep).await;
    let status = c.call("status", Value::Null).await.unwrap();
    let sleeping = status["sleeping"].as_array().expect("status lists sleeping instances");
    assert_eq!(sleeping.len(), 1, "{status}");
    assert_eq!(sleeping[0]["name"], "redis");
    assert!(sleeping[0]["since"].as_str().is_some_and(|s| s.contains('T')), "{status}");
    let units = status["units"].as_array().unwrap();
    assert!(units.iter().all(|u| u["name"] != "redis"), "no unit while it sleeps: {status}");
    let report = c.call("doctor", Value::Null).await.unwrap();
    let check = report["checks"].as_array().unwrap().iter().find(|c| c["id"] == "service:redis").cloned();
    let check = check.expect("a service:redis check");
    assert_eq!(check["status"], "ok", "{check}");
    assert!(check["message"].as_str().unwrap().contains("sleeping"), "{check}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn run_sleeping_lists_the_sites_that_need_it() {
    let d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "").await;
    write_yard(&d.project("blog"), "[services]\nredis = true\n");
    d.project("plain");
    let d = d.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), asleep).await;
    let file = d.fx.paths.sleeping_file();
    let listed = || std::fs::read_to_string(&file).is_ok_and(|t| t == "blog\n");
    assert!(wait_until(listed, Duration::from_secs(2)).await, "{:?}", std::fs::read_to_string(&file));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_empties_run_sleeping() {
    let d = with_redis(free_port(), "", "").await;
    std::fs::create_dir_all(d.fx.paths.run_dir()).unwrap();
    std::fs::write(d.fx.paths.sleeping_file(), "stale\n").unwrap();
    let d = d.start().await;
    assert_eq!(std::fs::read_to_string(d.fx.paths.sleeping_file()).unwrap(), "");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn instances_start_awake_after_a_restart() {
    let mut d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "").await.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), asleep).await;
    drop(c);
    d.stop().await.unwrap();
    // Long enough that it stays awake while this looks.
    std::fs::write(d.fx.paths.yard_toml(), "service_sleep_after = \"1h\"\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let info = redis_until(&mut c, Duration::from_secs(10), running).await;
    assert!(info.get("sleeping_since").is_none(), "{info}");
    d.shutdown().await.unwrap();
}

// ---- Waking (Task 6) ----

/// Sends `request` over `wake.sock` and returns the answer's status line and how long it took.
async fn raw(socket: PathBuf, request: &'static [u8]) -> (String, Duration) {
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
        stream.write_all(request).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        (answer.lines().next().unwrap_or_default().to_string(), started.elapsed())
    })
    .await
    .unwrap()
}

/// `GET /wake/<site>?wait=<wait>` as nginx and the shims send it.
async fn ask(d: &Daemon, site: &str, wait: u64) -> (String, Duration) {
    let request = format!("GET /wake/{site}?wait={wait} HTTP/1.1\r\nHost: yard\r\nConnection: close\r\n\r\n");
    raw(d.fx.paths.wake_socket(), Box::leak(request.into_bytes().into_boxed_slice())).await
}

const NO_CONTENT: &str = "HTTP/1.1 204 No Content";

/// A daemon whose `redis`, which `blog` needs, is asleep.
async fn asleep_for_blog(port: u16) -> (Daemon, Client) {
    let d = with_redis(port, "service_sleep_after = \"60s\"\n", "").await;
    write_yard(&d.project("blog"), "[services]\nredis = true\n");
    let d = d.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), asleep).await;
    (d, c)
}

/// Makes `sleep_after` a day (86 s on the test clock), so a woken instance stays awake while
/// the test looks; an asleep one stays asleep.
async fn sleep_long(d: &Daemon, c: &mut Client) {
    std::fs::write(d.fx.paths.yard_toml(), "service_sleep_after = \"24h\"\n").unwrap();
    redis_until(c, Duration::from_secs(5), |i| i["sleep_after_secs"] == 86400).await;
}

fn sleeping_file(d: &Daemon) -> String {
    std::fs::read_to_string(d.fx.paths.sleeping_file()).unwrap_or_default()
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn redis_unit(c: &mut Client) -> Option<Value> {
    let status = c.call("status", Value::Null).await.unwrap();
    status["units"].as_array().unwrap().iter().find(|u| u["name"] == "redis").cloned()
}

#[tokio::test]
async fn an_ask_wakes_the_sites_instance() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    sleep_long(&d, &mut c).await;
    assert_eq!(sleeping_file(&d), "blog\n");
    let (status, _) = ask(&d, "blog", 5).await;
    assert_eq!(status, NO_CONTENT);
    let info = redis(&mut c).await;
    assert!(running(&info), "the answer waits until it runs: {info}");
    assert!(info.get("sleeping_since").is_none(), "{info}");
    assert_eq!(sleeping_file(&d), "");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_asks_share_one_start() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    sleep_long(&d, &mut c).await;
    let (a, b) = tokio::join!(ask(&d, "blog", 5), ask(&d, "blog", 5));
    assert_eq!((a.0.as_str(), b.0.as_str()), (NO_CONTENT, NO_CONTENT));
    let unit = redis_unit(&mut c).await.expect("a redis unit");
    assert_eq!(unit["state"], "running", "both answers wait for it: {unit}");
    assert_eq!(unit["restarts"], 0, "one start: {unit}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_ask_counts_as_use() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    ask(&d, "blog", 5).await;
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        ask(&d, "blog", 5).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let info = redis(&mut c).await;
    assert!(running(&info), "asks keep it awake: {info}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unknown_site_and_garbage_get_204() {
    let (d, _c) = asleep_for_blog(free_port()).await;
    let socket = d.fx.paths.wake_socket();
    assert_eq!(ask(&d, "nope", 1).await.0, NO_CONTENT);
    assert_eq!(raw(socket.clone(), b"GET / HTTP/1.1\r\n\r\n").await.0, NO_CONTENT);
    assert_eq!(raw(socket.clone(), b"hello\r\n\r\n").await.0, NO_CONTENT);
    assert_eq!(raw(socket, b"GET /wake/../x?wait=1 HTTP/1.1\r\n\r\n").await.0, NO_CONTENT);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_silent_client_is_answered_and_closed() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    sleep_long(&d, &mut c).await;
    let silent = raw(d.fx.paths.wake_socket(), b"");
    let other = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        ask(&d, "nope", 1).await
    };
    let ((silent, waited), (other, other_took)) = tokio::join!(silent, other);
    assert_eq!(silent, NO_CONTENT);
    assert!(waited < Duration::from_secs(3), "answered after {waited:?}");
    assert_eq!(other, NO_CONTENT);
    assert!(other_took < Duration::from_secs(1), "another ask is not held up: {other_took:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_stopped_instance_is_never_woken() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    c.call("services.stop", json!({"name": "redis"})).await.unwrap();
    assert_eq!(ask(&d, "blog", 2).await.0, NO_CONTENT);
    let info = redis(&mut c).await;
    assert_eq!(info["stopped_by_user"], true, "{info}");
    assert!(redis_unit(&mut c).await.is_none(), "nothing started it");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_held_port_answers_at_once_and_fails() {
    let port = free_port();
    let (d, mut c) = asleep_for_blog(port).await;
    sleep_long(&d, &mut c).await;
    let _holder = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let (status, took) = ask(&d, "blog", 10).await;
    assert_eq!(status, NO_CONTENT);
    assert!(took < Duration::from_secs(2), "a held port ends the wait: {took:?}");
    let info = redis_until(&mut c, Duration::from_secs(2), |i| i["state"].get("failed").is_some()).await;
    assert_eq!(info["state"]["failed"]["reason"]["code"], "PORT_IN_USE", "{info}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_wake_during_the_stop_waits_for_it() {
    let d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "").await;
    write_yard(&d.project("blog"), "[services]\nredis = true\n");
    std::fs::write(d.fx.brew.path().join("fake-stop-delay"), "500").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    let old_pid: i32 =
        std::fs::read_to_string(d.fx.paths.service_pid("redis")).unwrap().trim().parse().unwrap();
    // The fake logs this when its stop signal arrives, then takes 500 ms to exit.
    let log = d.fx.paths.log("redis");
    let stopping = || std::fs::read_to_string(&log).is_ok_and(|t| t.contains("fake stopping"));
    assert!(wait_until(stopping, Duration::from_secs(5)).await, "redis began to stop");
    std::fs::write(d.fx.paths.yard_toml(), "service_sleep_after = \"24h\"\n").unwrap();
    let (status, _) = ask(&d, "blog", 5).await;
    assert_eq!(status, NO_CONTENT);
    assert!(!alive(old_pid), "the old process was gone before the new one started");
    let unit = redis_unit(&mut c).await.expect("a redis unit");
    assert_eq!(unit["state"], "running", "{unit}");
    assert_ne!(unit["pid"], old_pid, "{unit}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn service_start_and_restart_wake() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    sleep_long(&d, &mut c).await;
    c.call("services.start", json!({"name": "redis"})).await.unwrap();
    redis_until(&mut c, Duration::from_secs(2), running).await;
    std::fs::write(d.fx.paths.yard_toml(), "service_sleep_after = \"60s\"\n").unwrap();
    redis_until(&mut c, Duration::from_secs(5), asleep).await;
    sleep_long(&d, &mut c).await;
    c.call("services.restart", json!({"name": "redis"})).await.unwrap();
    let info = redis_until(&mut c, Duration::from_secs(2), running).await;
    assert_eq!(info["stopped_by_user"], false, "{info}");
    let services = std::fs::read_to_string(d.fx.paths.services_toml()).unwrap();
    assert!(!services.contains("stopped"), "{services}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn stop_and_remove_clear_sleep() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    c.call("services.stop", json!({"name": "redis"})).await.unwrap();
    let info = redis(&mut c).await;
    assert!(info.get("sleeping_since").is_none(), "{info}");
    assert_eq!(sleeping_file(&d), "");
    let status = c.call("status", Value::Null).await.unwrap();
    assert!(status.get("sleeping").is_none(), "{status}");

    c.call("services.start", json!({"name": "redis"})).await.unwrap();
    redis_until(&mut c, Duration::from_secs(5), asleep).await;
    std::fs::remove_file(d.projects.path().join("blog").join(".yard")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match c.call("services.remove", json!({"name": "redis", "keep_data": false})).await {
            Ok(_) => break,
            Err(e) if e.code == yard_proto::ErrorCode::ServiceInUse && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("{e:?}"),
        }
    }
    let status = c.call("status", Value::Null).await.unwrap();
    assert!(status.get("sleeping").is_none(), "a removed instance does not sleep: {status}");
    assert_eq!(sleeping_file(&d), "");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn sleep_set_writes_and_clears_the_key() {
    let d = with_redis(free_port(), "", "").await.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    let info = c.call("services.sleep.set", json!({"name": "redis", "after": "1h"})).await.unwrap();
    assert_eq!(info["sleep_after_secs"], 3600, "{info}");
    assert_eq!(info["sleep_after_source"], "services_toml", "{info}");
    let services = std::fs::read_to_string(d.fx.paths.services_toml()).unwrap();
    assert!(services.contains("sleep_after = \"1h\""), "{services}");
    let info = c.call("services.sleep.set", json!({"name": "redis", "after": "default"})).await.unwrap();
    assert!(info.get("sleep_after_secs").is_none(), "{info}");
    let services = std::fs::read_to_string(d.fx.paths.services_toml()).unwrap();
    assert!(!services.contains("sleep_after"), "{services}");
    let e = c.call("services.sleep.set", json!({"name": "redis", "after": "5s"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument);
    assert_eq!(e.hint.as_deref(), Some("yard service sleep-after redis 15m"));
    let e = c.call("services.sleep.set", json!({"name": "nope", "after": "1h"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::ServiceNotFound);
    d.shutdown().await.unwrap();
}

// ---- Final review fixes ----

/// A daemon whose `redis`, which `blog` needs, takes `delay_ms` to stop, and has just begun
/// to stop on falling asleep.
async fn stopping_slowly(delay_ms: u64) -> (Daemon, Client) {
    let d = with_redis(free_port(), "service_sleep_after = \"60s\"\n", "").await;
    write_yard(&d.project("blog"), "[services]\nredis = true\n");
    std::fs::write(d.fx.brew.path().join("fake-stop-delay"), delay_ms.to_string()).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    let log = d.fx.paths.log("redis");
    let stopping = || std::fs::read_to_string(&log).is_ok_and(|t| t.contains("fake stopping"));
    assert!(wait_until(stopping, Duration::from_secs(5)).await, "redis began to stop");
    (d, c)
}

#[tokio::test]
async fn run_sleeping_lists_the_site_while_its_instance_stops() {
    let (d, _c) = stopping_slowly(3000).await;
    // A shim run now asks for a wake, which waits for the stop, instead of running against a
    // stopping engine.
    assert_eq!(sleeping_file(&d), "blog\n");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_sleep_keeps_it_awake() {
    let port = free_port();
    let d = with_redis(port, "service_sleep_after = \"60s\"\n", "").await.start().await;
    let mut c = d.client().await;
    redis_until(&mut c, Duration::from_secs(10), running).await;
    let held = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    // A sites.toml that does not parse fails every reconcile, the sleep's included.
    std::fs::write(d.fx.paths.sites_toml(), "[sites\n").unwrap();
    let broken = async {
        loop {
            let status = c.call("status", Value::Null).await.unwrap();
            if status["config_error"].is_object() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), broken).await.expect("the reconcile fails");
    drop(held);
    // Several samples try to put it to sleep and fail; it is reported awake, as it is.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let info =
        redis_until(&mut c, Duration::from_secs(2), |i| running(i) && i.get("sleeping_since").is_none())
            .await;
    assert!(running(&info), "{info}");
    assert_eq!(sleeping_file(&d), "");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_ask_does_not_wait_on_another_stop() {
    let (d, _c) = stopping_slowly(3000).await;
    // `shop` needs nothing asleep: its answer must not wait for redis's 3 s stop.
    write_yard(&d.project("shop"), "");
    let (status, took) = ask(&d, "shop", 5).await;
    assert_eq!(status, NO_CONTENT);
    assert!(took < Duration::from_millis(1000), "took {took:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn wait_bounds_an_ask_during_a_stop() {
    let (d, _c) = stopping_slowly(3000).await;
    let (status, took) = ask(&d, "blog", 1).await;
    assert_eq!(status, NO_CONTENT);
    assert!(took < Duration::from_millis(1800), "wait=1 answered after {took:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn turning_sleep_off_wakes_it() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    std::fs::write(d.fx.paths.yard_toml(), "").unwrap();
    let info = redis_until(&mut c, Duration::from_secs(5), running).await;
    assert!(info.get("sleeping_since").is_none(), "{info}");
    assert_eq!(sleeping_file(&d), "");

    std::fs::write(d.fx.paths.yard_toml(), "service_sleep_after = \"60s\"\n").unwrap();
    redis_until(&mut c, Duration::from_secs(5), asleep).await;
    c.call("services.sleep.set", json!({"name": "redis", "after": "off"})).await.unwrap();
    redis_until(&mut c, Duration::from_secs(5), running).await;
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn run_sleeping_follows_links_and_hand_edits() {
    let (d, mut c) = asleep_for_blog(free_port()).await;
    sleep_long(&d, &mut c).await;
    assert_eq!(sleeping_file(&d), "blog\n");
    write_yard(&d.project("shop"), "[services]\nredis = true\n");
    let both = || sleeping_file(&d) == "blog\nshop\n";
    assert!(
        wait_until(both, Duration::from_secs(5)).await,
        "a newly linked site is listed: {:?}",
        sleeping_file(&d)
    );

    let file = d.fx.paths.services_toml();
    let text = std::fs::read_to_string(&file)
        .unwrap()
        .replace("formula = \"redis\"\n", "formula = \"redis\"\nstopped = true\n");
    std::fs::write(&file, text).unwrap();
    let empty = || sleeping_file(&d).is_empty();
    assert!(
        wait_until(empty, Duration::from_secs(5)).await,
        "a stopped instance lists nothing: {:?}",
        sleeping_file(&d)
    );
    let info = redis(&mut c).await;
    assert!(info.get("sleeping_since").is_none(), "{info}");
    d.shutdown().await.unwrap();
}
