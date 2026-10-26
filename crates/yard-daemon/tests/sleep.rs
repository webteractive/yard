//! On-demand services (M4e): an idle instance falls asleep, and wakes on use. The fake
//! clock (`SleepConfig.second` = 1 ms) makes `"60s"` last 60 ms.

mod support;

use serde_json::Value;
use std::path::Path;
use std::time::Duration;
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
