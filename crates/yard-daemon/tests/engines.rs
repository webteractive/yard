//! The user's recipes in `engines/` (recipes spec §7): added, edited, broken and removed while
//! yardd runs, with `yard-fake-unit` as `fake-service` standing in for the engine's program.

mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::daemon::{Client, Daemon, prepared};
use support::{free_port, served, wait_until};

/// A brew recipe for `key` whose program is the fake, listening on its main port; `extra` is
/// one more argument, so an edit can change the command.
fn recipe(key: &str, extra: &str) -> String {
    recipe_on(key, extra, 27000)
}

/// [`recipe`] whose main port is offered `usual` first; tests keep it in 20000-29999.
fn recipe_on(key: &str, extra: &str, usual: u16) -> String {
    format!(
        "schema = 1\nkey = \"{key}\"\nname = \"Echo {key}\"\n\n[source]\nkind = \"brew\"\nformula = \"fake-{key}\"\n\n\
         [ports]\nmain = {usual}\n\n[run]\ncommand = [\"{{home}}/bin/fake-service\", \"127.0.0.1:{{port.main}}\", \"{extra}\"]\n\
         dirs = [\"data\"]\n\n[health]\ntcp = \"main\"\n\n[env]\nlines = [\"ECHO_PORT={{port.main}}\", \"ECHO_EXTRA={extra}\"]\n"
    )
}

/// A daemon whose `echo` engine's keg is in place and whose instance `echo` is pinned to a port.
async fn with_echo(text: Option<&str>) -> (Daemon, u16) {
    let port = free_port();
    let d = prepared(&format!("\"echo\" = {{ port = {port}, mode = \"pinned\" }}\n")).await;
    d.fx.add_fake_formula("fake-echo");
    if let Some(text) = text {
        d.fx.add_recipe("echo", text);
    }
    (d.start().await, port)
}

async fn status(c: &mut Client) -> Value {
    c.call("status", Value::Null).await.unwrap()
}

async fn pid(c: &mut Client, name: &str) -> Option<u64> {
    let s = status(c).await;
    s["units"].as_array()?.iter().find(|u| u["name"] == name && u["state"] == "running")?["pid"].as_u64()
}

/// Polls until `check` holds of `status`, for up to 10 s.
async fn status_until(c: &mut Client, check: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..200 {
        last = status(c).await;
        if check(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("status never matched: {last}");
}

async fn env_lines(c: &mut Client, service: &str) -> Value {
    let blocks = c.call("env.print", json!({"service": service})).await.unwrap();
    blocks[0]["lines"].clone()
}

fn engine_errors(s: &Value) -> Vec<String> {
    s["engine_errors"]
        .as_array()
        .map(|a| a.iter().map(|e| e["message"].as_str().unwrap_or_default().to_string()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn user_recipe_is_added_without_restart() {
    let (d, _) = with_echo(None).await;
    let mut c = d.client().await;
    // No instance holds a pin yet, so the add allocates from the recipe's usual port.
    let port = free_port();
    d.fx.add_recipe("echo", &recipe_on("echo", "one", port));
    // The watcher reconciles within its debounce; until then `services.add` does not know echo.
    c.call("subscribe", json!({"topics": ["jobs"]})).await.unwrap();
    let mut started = None;
    for _ in 0..100 {
        match c.call("services.add", json!({"engine": "echo"})).await {
            Ok(job) => {
                started = Some(job);
                break;
            }
            Err(e) if e.code == yard_proto::ErrorCode::InvalidArgument => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("{e:?}"),
        }
    }
    let id = started.expect("yardd learned the recipe")["job_id"].as_str().unwrap().to_string();
    let done = c
        .event_where(Duration::from_secs(10), |e| e["type"] == "job_finished" && e["job_id"] == id.as_str())
        .await
        .expect("the add finished");
    assert_eq!(done["error"], Value::Null, "{done}");
    assert!(wait_until(|| served(port), Duration::from_secs(5)).await, "echo listens on its usual port");
    assert!(d.fx.paths.service_data("echo").is_dir(), "the dirs the recipe lists are made");
    assert_eq!(env_lines(&mut c, "echo").await, json!([format!("ECHO_PORT={port}"), "ECHO_EXTRA=one"]));
    let rows = c.call("services.list", Value::Null).await.unwrap();
    let row = rows.as_array().unwrap().iter().find(|r| r["name"] == "echo").unwrap().clone();
    assert_eq!(row["engine_name"], "Echo echo");
    assert_eq!(row["formula"], "fake-echo");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn user_recipe_replaces_carried() {
    let redis = free_port();
    let d = prepared(&format!("\"redis\" = {{ port = {redis}, mode = \"pinned\" }}\n")).await;
    d.fx.add_fake_formula("fake-redis");
    d.fx.add_recipe("redis", &recipe("redis", "mine"));
    d.fx.add_service("redis", "fake-redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    assert!(wait_until(|| served(redis), Duration::from_secs(5)).await, "the user's redis runs");
    assert_eq!(env_lines(&mut c, "redis").await, json!([format!("ECHO_PORT={redis}"), "ECHO_EXTRA=mine"]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn user_recipe_cannot_replace_builtin() {
    let (d, _) = with_echo(None).await;
    d.fx.add_recipe("mysql", &recipe("mysql", "x"));
    let mut c = d.client().await;
    let s = status_until(&mut c, |s| !engine_errors(s).is_empty()).await;
    let errors = engine_errors(&s);
    assert!(errors[0].contains("mysql.toml") && errors[0].contains("reserved"), "{errors:?}");
    assert_eq!(s["engine_errors"][0]["code"], "ENGINE_RECIPE_INVALID");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn broken_edit_keeps_last_good() {
    let (d, port) = with_echo(Some(&recipe("echo", "one"))).await;
    d.fx.add_service("echo", "fake-echo", false);
    let mut c = d.client().await;
    assert!(wait_until(|| served(port), Duration::from_secs(5)).await);
    let before = status_until(&mut c, |s| {
        s["units"]
            .as_array()
            .is_some_and(|u| u.iter().any(|u| u["name"] == "echo" && u["state"] == "running"))
    })
    .await;
    let before =
        before["units"].as_array().unwrap().iter().find(|u| u["name"] == "echo").unwrap()["pid"].clone();

    d.fx.add_recipe("echo", "schema = 1\nkey = \"echo\"\nname = \n");
    let s = status_until(&mut c, |s| !engine_errors(s).is_empty()).await;
    let errors = engine_errors(&s);
    assert!(errors[0].contains("echo.toml:3:") && errors[0].contains("last good"), "{errors:?}");
    assert_eq!(pid(&mut c, "echo").await.map(Value::from), Some(before), "the instance kept running");
    assert_eq!(env_lines(&mut c, "echo").await, json!([format!("ECHO_PORT={port}"), "ECHO_EXTRA=one"]));

    d.fx.add_recipe("echo", &recipe("echo", "one"));
    status_until(&mut c, |s| engine_errors(s).is_empty()).await;
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn broken_at_start_is_unknown() {
    let (d, port) = with_echo(None).await;
    d.shutdown().await.unwrap();
    let port_line = format!("\"echo\" = {{ port = {port}, mode = \"pinned\" }}\n");
    let d = prepared(&port_line).await;
    d.fx.add_fake_formula("fake-echo");
    d.fx.add_recipe("echo", "schema = 1\nkey = ");
    d.fx.add_service("echo", "fake-echo", false);
    let d = d.start().await;
    let mut c = d.client().await;
    let rows = c.call("services.list", Value::Null).await.unwrap();
    let row = rows.as_array().unwrap().iter().find(|r| r["name"] == "echo").unwrap().clone();
    assert_eq!(row["state"]["failed"]["reason"]["code"], "ENGINE_UNKNOWN", "{row}");
    assert!(!served(port));
    assert!(!engine_errors(&status(&mut c).await).is_empty());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn deleted_recipe_leaves_data() {
    let (d, port) = with_echo(Some(&recipe("echo", "one"))).await;
    d.fx.add_service("echo", "fake-echo", false);
    let mut c = d.client().await;
    assert!(wait_until(|| served(port), Duration::from_secs(5)).await);
    std::fs::write(d.fx.paths.service_data("echo").join("keep"), "x").unwrap();

    std::fs::remove_file(d.fx.paths.engines_dir().join("echo.toml")).unwrap();
    assert!(wait_until(|| !served(port), Duration::from_secs(10)).await, "the unit stops");
    let mut failed = Value::Null;
    for _ in 0..100 {
        let rows = c.call("services.list", Value::Null).await.unwrap();
        failed = rows.as_array().unwrap().iter().find(|r| r["name"] == "echo").unwrap()["state"].clone();
        if failed["failed"]["reason"]["code"] == "ENGINE_UNKNOWN" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(failed["failed"]["reason"]["code"], "ENGINE_UNKNOWN", "{failed}");
    assert!(d.fx.paths.service_data("echo").join("keep").exists(), "its data is never touched");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn edited_command_restarts_unit() {
    let (d, port) = with_echo(Some(&recipe("echo", "one"))).await;
    d.fx.add_service("echo", "fake-echo", false);
    let mut c = d.client().await;
    assert!(wait_until(|| served(port), Duration::from_secs(5)).await);
    let first = status_until(&mut c, |s| {
        s["units"]
            .as_array()
            .is_some_and(|u| u.iter().any(|u| u["name"] == "echo" && u["state"] == "running"))
    })
    .await;
    let first =
        first["units"].as_array().unwrap().iter().find(|u| u["name"] == "echo").unwrap()["pid"].as_u64();

    d.fx.add_recipe("echo", &recipe("echo", "two"));
    let log = d.fx.brew.path().join("opt/fake-echo/bin/fake-service.log");
    assert!(
        wait_until(
            || std::fs::read_to_string(&log).is_ok_and(|l| l.contains(" two ")),
            Duration::from_secs(10)
        )
        .await,
        "the new command ran"
    );
    let second = status_until(&mut c, |s| {
        s["units"].as_array().is_some_and(|u| {
            u.iter().any(|u| u["name"] == "echo" && u["state"] == "running" && u["pid"].as_u64() != first)
        })
    })
    .await;
    assert!(second["units"].as_array().unwrap().iter().any(|u| u["name"] == "echo"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn engines_changed_event_fires() {
    let (d, _) = with_echo(None).await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["state"]})).await.unwrap();
    d.fx.add_recipe("echo", &recipe("echo", "one"));
    let event = c.event_where(Duration::from_secs(10), |e| e["type"] == "engines_changed").await;
    assert!(event.is_some(), "engines_changed after the new recipe");
    d.shutdown().await.unwrap();
}
