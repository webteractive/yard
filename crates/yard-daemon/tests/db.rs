//! `yard db` (db spec): `db.list`, `db.create`, `db.connect` and `db.command` against the fake
//! engines, whose `mysql`/`psql` log their arguments to `<keg>/fake-db.log` and answer queries
//! from `fake-db-exists`, `fake-db-tables` and `fake-db-answer` beside it.

mod support;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use support::daemon::{Client, Daemon, prepared};
use support::free_port;

fn pin(unit: &str, port: u16) -> String {
    format!("\"{unit}\" = {{ port = {port}, mode = \"pinned\" }}\n")
}

/// A daemon with a fake `mysql-8.4` (and, with `pg`, a `postgres-17`), not yet started.
async fn with_db(extra: &str, stopped: bool, pg: bool) -> Daemon {
    let mut ports = pin("mysql-8.4", free_port());
    if pg {
        ports.push_str(&pin("postgres-17", free_port()));
    }
    let d = prepared(&ports).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", stopped);
    if pg {
        d.fx.add_formula("postgresql@17");
        d.fx.add_service("postgres-17", "postgresql@17", false);
    }
    if !extra.is_empty() {
        let file = d.fx.paths.services_toml();
        let text = std::fs::read_to_string(&file)
            .unwrap()
            .replace("formula = \"mysql@8.4\"\n", &format!("formula = \"mysql@8.4\"\n{extra}"));
        std::fs::write(&file, text).unwrap();
    }
    d
}

fn keg(d: &Daemon, formula: &str) -> PathBuf {
    d.fx.brew.path().join("opt").join(formula)
}

fn log(d: &Daemon, formula: &str) -> Vec<Vec<String>> {
    std::fs::read_to_string(keg(d, formula).join("fake-db.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

async fn service(c: &mut Client, name: &str) -> Value {
    let list = c.call("services.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap()
}

async fn until(c: &mut Client, name: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let info = service(c, name).await;
        if done(&info) {
            return info;
        }
        assert!(tokio::time::Instant::now() < deadline, "{name} never got there: {info}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn running(info: &Value) -> bool {
    info["state"] == "running"
}

fn asleep(info: &Value) -> bool {
    info["state"] == "stopped" && info["sleeping_since"].is_string()
}

#[tokio::test]
async fn list_answers_each_instance() {
    let d = with_db("", false, false).await;
    std::fs::write(keg(&d, "mysql@8.4").join("fake-db-answer"), "shop\t0\t0\nblog\t10\t2\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", running).await;
    let list = c.call("db.list", Value::Null).await.unwrap();
    assert_eq!(
        list,
        json!([{"instance": "mysql-8.4", "engine": "mysql", "databases": [
            {"name": "blog", "size_bytes": 10, "tables": 2},
            {"name": "shop", "size_bytes": 0, "tables": 0}
        ]}])
    );
    let call = log(&d, "mysql@8.4").pop().unwrap();
    for arg in ["--no-defaults", "-N", "-B", "-e", "--user=root"] {
        assert!(call.contains(&arg.to_string()), "{arg}: {call:?}");
    }
    let socket = format!("--socket={}", d.fx.paths.mysql_socket("mysql-8.4").display());
    assert!(call.contains(&socket), "{call:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn create_quotes_a_hyphenated_name() {
    let d = with_db("", false, false).await.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", running).await;
    let made = c.call("db.create", json!({"name": "my-app"})).await.unwrap();
    assert_eq!(made, json!({"instance": "mysql-8.4", "name": "my-app", "created": true}));
    let calls = log(&d, "mysql@8.4");
    assert!(
        calls.iter().flatten().any(|a| a.contains("CREATE DATABASE IF NOT EXISTS `my-app`")),
        "{calls:?}"
    );
    std::fs::write(keg(&d, "mysql@8.4").join("fake-db-exists"), "1\n").unwrap();
    let again = c.call("db.create", json!({"name": "my-app"})).await.unwrap();
    assert_eq!(again["created"], false);
    let before = log(&d, "mysql@8.4").len();
    let e = c.call("db.create", json!({"name": "my app"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument);
    assert_eq!(log(&d, "mysql@8.4").len(), before, "no query for a bad name");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn query_failure_carries_the_tail() {
    let d = with_db("", false, false).await;
    std::fs::write(keg(&d, "mysql@8.4").join("fake-db-fail"), "").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", running).await;
    let e = c.call("db.list", Value::Null).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::DbToolFailed);
    assert!(e.data.unwrap()["tail"].to_string().contains("fake db failed"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ready_refuses_a_stopped_instance() {
    let d = with_db("", true, false).await.start().await;
    let mut c = d.client().await;
    let e = c.call("db.list", json!({"service": "mysql-8.4"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::ServiceStopped);
    assert_eq!(e.hint.as_deref(), Some("yard service start mysql-8.4"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_does_not_wake_unless_asked() {
    let d = with_db("sleep_after = \"60s\"\n", false, false).await.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", asleep).await;
    let info = c.call("db.connect", Value::Null).await.unwrap();
    assert_eq!(info[0]["state"], "stopped", "{info}");
    assert!(info[0]["url"].as_str().unwrap().starts_with("mysql://root@127.0.0.1:"), "{info}");
    assert!(asleep(&service(&mut c, "mysql-8.4").await));
    let info = c.call("db.connect", json!({"wake": true})).await.unwrap();
    assert_eq!(info.as_array().unwrap().len(), 1);
    assert_eq!(info[0]["state"], "running", "{info}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ready_times_out() {
    let mut d = with_db("sleep_after = \"60s\"\n", false, false).await;
    d.opts.db_wake_wait = Duration::from_millis(200);
    let d = d.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", asleep).await;
    std::fs::write(d.fx.brew.path().join("fake-listen-delay"), "3000").unwrap();
    let e = c.call("db.connect", json!({"wake": true})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::ServiceWakeTimeout, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some("yard service logs mysql-8.4"));
    std::fs::remove_file(d.fx.brew.path().join("fake-listen-delay")).unwrap();
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_import_creates_then_refuses_when_not_empty() {
    let d = with_db("", false, false).await.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", running).await;
    let params = json!({"action": "import", "name": "shop", "import_kind": "plain", "args": ["--force"]});
    let cmd = c.call("db.command", params.clone()).await.unwrap();
    assert_eq!(cmd["created"], true);
    assert_eq!(cmd["filter"], json!({"kind": "mysql"}));
    assert!(cmd["program"].as_str().unwrap().ends_with("opt/mysql@8.4/bin/mysql"), "{cmd}");
    let args: Vec<String> = serde_json::from_value(cmd["args"].clone()).unwrap();
    assert_eq!(args.last().map(String::as_str), Some("--force"));
    assert!(args.contains(&"--binary-mode".to_string()) && args.contains(&"--database=shop".to_string()));

    std::fs::write(keg(&d, "mysql@8.4").join("fake-db-exists"), "1\n").unwrap();
    std::fs::write(keg(&d, "mysql@8.4").join("fake-db-tables"), "3\n").unwrap();
    let e = c.call("db.command", params.clone()).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::DbNotEmpty);
    assert!(e.hint.unwrap().contains("--into-existing"));
    let mut into = params;
    into["into_existing"] = json!(true);
    let cmd = c.call("db.command", into).await.unwrap();
    assert_eq!(cmd["created"], false);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_refuses_reserved_flags_before_waking() {
    let d = with_db("sleep_after = \"60s\"\n", false, false).await.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", asleep).await;
    let e = c
        .call("db.command", json!({"action": "dump", "name": "shop", "args": ["--host=elsewhere"]}))
        .await
        .unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::DbFlagReserved);
    assert!(asleep(&service(&mut c, "mysql-8.4").await));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_project_fallback_notes_the_instance() {
    let d = with_db("", false, false).await.start().await;
    let mut c = d.client().await;
    until(&mut c, "mysql-8.4", running).await;
    let dir = d.projects.path().join("plain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.php"), "<?php").unwrap();
    c.call("sites.link", json!({"name": "plain", "path": dir.display().to_string()})).await.unwrap();
    let cmd =
        c.call("db.command", json!({"action": "shell", "path": dir.display().to_string()})).await.unwrap();
    assert_eq!(cmd["notes"], json!(["using mysql-8.4"]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_postgres_uses_the_newest_keg_and_import_kinds() {
    let d = with_db("", false, true).await;
    d.fx.add_formula("postgresql@18");
    let d = d.start().await;
    let mut c = d.client().await;
    until(&mut c, "postgres-17", running).await;
    let base = json!({"service": "postgres-17", "action": "import", "name": "app"});
    let mut archive = base.clone();
    archive["import_kind"] = json!("archive");
    let cmd = c.call("db.command", archive.clone()).await.unwrap();
    assert!(cmd["program"].as_str().unwrap().ends_with("opt/postgresql@18/bin/pg_restore"), "{cmd}");
    assert_eq!(cmd["notes"], json!(["using pg_restore 18 for postgres-17"]));
    let args: Vec<String> = serde_json::from_value(cmd["args"].clone()).unwrap();
    assert!(args.contains(&"--no-owner".to_string()), "{args:?}");
    assert!(cmd["env_remove"].as_array().unwrap().contains(&json!("PGPASSFILE")));

    std::fs::write(keg(&d, "postgresql@17").join("fake-db-exists"), "1\n").unwrap();
    std::fs::write(keg(&d, "postgresql@18").join("fake-db-exists"), "1\n").unwrap();
    archive["keep_owners"] = json!(true);
    let cmd = c.call("db.command", archive).await.unwrap();
    assert!(!cmd["args"].as_array().unwrap().contains(&json!("--no-owner")));

    let mut plain = base;
    plain["import_kind"] = json!("plain");
    let cmd = c.call("db.command", plain).await.unwrap();
    assert!(cmd["program"].as_str().unwrap().ends_with("/bin/psql"));
    assert!(cmd["args"].as_array().unwrap().contains(&json!("ON_ERROR_STOP=1")));
    assert_eq!(cmd["filter"], json!({"kind": "postgres", "keep_owners": false}));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn list_never_queries_a_database_whose_name_yard_would_not_take() {
    // A name another role made can hold SQL; it is listed, but never put into a query or -d.
    let d = with_db("", false, true).await;
    std::fs::write(
        keg(&d, "postgresql@17").join("fake-db-answer"),
        "app\t10\t0\nx'; DROP ROLE r; --\t20\t0\n",
    )
    .unwrap();
    std::fs::write(keg(&d, "postgresql@17").join("fake-db-tables"), "4\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    until(&mut c, "postgres-17", running).await;
    let list = c.call("db.list", json!({"service": "postgres-17"})).await.unwrap();
    let dbs = &list[0]["databases"];
    assert_eq!(dbs[0], json!({"name": "app", "size_bytes": 10, "tables": 4}));
    assert_eq!(dbs[1], json!({"name": "x'; DROP ROLE r; --", "size_bytes": 20, "tables": 0}));
    let calls = log(&d, "postgresql@17");
    assert!(!calls.iter().flatten().any(|a| a.contains("DROP ROLE")), "{calls:?}");
    d.shutdown().await.unwrap();
}
