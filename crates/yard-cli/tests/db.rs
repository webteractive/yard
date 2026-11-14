//! `yard db` against `MockDaemon` (db spec §6, §8).

mod support;

use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::{MockDaemon, stderr, stdout};

fn lists() -> Value {
    json!([
        {"instance": "mysql-8.4", "engine": "mysql", "databases": [
            {"name": "blog", "size_bytes": 3_145_728, "tables": 12},
            {"name": "shop", "size_bytes": 81_920, "tables": 3}
        ]},
        {"instance": "postgres-17", "engine": "postgres", "databases": []}
    ])
}

fn mysql_connect(state: &str, sleeping: bool) -> Value {
    json!({"instance": "mysql-8.4", "engine": "mysql", "state": state, "sleeping": sleeping,
        "host": "127.0.0.1", "port": 3307, "user": "root", "password": null,
        "socket": "/Users/me/.yard/run/mysql-8.4.sock", "url": "mysql://root@127.0.0.1:3307"})
}

fn pg_connect() -> Value {
    json!({"instance": "postgres-17", "engine": "postgres", "state": "running", "sleeping": false,
        "host": "127.0.0.1", "port": 5433, "user": "me", "password": null,
        "socket": "/Users/me/.yard/run", "url": "postgresql://me@127.0.0.1:5433"})
}

fn redis_connect() -> Value {
    json!({"instance": "redis", "engine": "redis", "state": "running", "sleeping": false,
        "host": "127.0.0.1", "port": 6380, "user": null, "password": null, "socket": null,
        "url": "redis://127.0.0.1:6380"})
}

/// An executable script in `dir`.
fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

#[test]
fn list_snapshot() {
    let d = MockDaemon::start(&[("db.list", Ok(lists()))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn list_json_is_proto_shape() {
    let d = MockDaemon::start(&[("db.list", Ok(lists()))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4", "--json"]).output().unwrap();
    let parsed: Vec<yard_proto::DbInstanceList> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed.len(), 2);
}

#[test]
fn db_sends_the_working_directory() {
    let d = MockDaemon::start(&[("db.list", Ok(json!([]))), ("db.connect", Ok(json!([])))]);
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().args(["db", "list"]).current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let p = &d.params("db.list")[0];
    assert_eq!(Path::new(p["path"].as_str().unwrap()), dir.path().canonicalize().unwrap());
    assert!(p.get("site").is_none() && p.get("service").is_none());
}

#[test]
fn service_flag_is_sent() {
    let d = MockDaemon::start(&[("db.list", Ok(json!([])))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("db.list")[0], json!({"service": "mysql-8.4"}));
    assert!(d.params("db.connect").is_empty(), "nothing to warn about with --service");
}

#[test]
fn list_without_service_warns_before_waking() {
    let d = MockDaemon::start(&[
        ("db.connect", Ok(json!([mysql_connect("stopped", true), redis_connect()]))),
        ("db.list", Ok(lists())),
    ]);
    let out = d.yard().args(["db", "list"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("waking mysql-8.4 to list its databases"), "{}", stderr(&out));
    assert!(!stderr(&out).contains("redis"));
}

#[test]
fn create_says_what_it_did() {
    let d = MockDaemon::start(&[(
        "db.create",
        Ok(json!({"instance": "mysql-8.4", "name": "shop", "created": true})),
    )]);
    let out = d.yard().args(["db", "create", "shop", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Created shop on mysql-8.4.\n");
    assert_eq!(d.params("db.create")[0], json!({"service": "mysql-8.4", "name": "shop"}));
    let d = MockDaemon::start(&[(
        "db.create",
        Ok(json!({"instance": "mysql-8.4", "name": "shop", "created": false})),
    )]);
    let out = d.yard().args(["db", "create", "shop", "--service", "mysql-8.4"]).output().unwrap();
    assert_eq!(stdout(&out), "shop already exists on mysql-8.4.\n");
}

#[test]
fn connect_snapshot_mysql_sleeping() {
    let d = MockDaemon::start(&[("db.connect", Ok(json!([mysql_connect("stopped", true)])))]);
    let out = d.yard().args(["db", "connect", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    assert_eq!(d.params("db.connect")[0], json!({"service": "mysql-8.4", "wake": false}));
}

#[test]
fn connect_snapshot_postgres_and_redis() {
    let d = MockDaemon::start(&[("db.connect", Ok(json!([pg_connect(), redis_connect()])))]);
    let out = d.yard().args(["db", "connect"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn open_opens_the_url_with_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("opened");
    let open = script(dir.path(), "open", &format!("echo \"$1\" > '{}'", log.display()));
    let d = MockDaemon::start(&[("db.connect", Ok(json!([mysql_connect("running", false)])))]);
    let out = d
        .yard()
        .args(["db", "open", "shop", "--service", "mysql-8.4"])
        .env("YARD_OPEN", &open)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "mysql://root@127.0.0.1:3307/shop\n");
    assert_eq!(d.params("db.connect")[0], json!({"service": "mysql-8.4", "wake": true}));
}

#[test]
fn open_falls_back_to_details() {
    let dir = tempfile::tempdir().unwrap();
    let open = script(dir.path(), "open", "exit 1");
    let d = MockDaemon::start(&[("db.connect", Ok(json!([mysql_connect("running", false)])))]);
    let out =
        d.yard().args(["db", "open", "--service", "mysql-8.4"]).env("YARD_OPEN", &open).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("URL       mysql://root@127.0.0.1:3307"), "{}", stdout(&out));
    assert!(stderr(&out).contains("error: no app opens mysql:// URLs"), "{}", stderr(&out));
}

#[test]
fn shell_execs_the_answered_program() {
    let dir = tempfile::tempdir().unwrap();
    let program =
        script(dir.path(), "client", "echo \"args: $*\"; echo \"PGHOST=$PGHOST\"; echo \"EXTRA=$EXTRA\"");
    let cmd = json!({"instance": "postgres-17", "program": program, "args": ["-d", "app", "-c", "select 1"],
        "env": [["EXTRA", "yes"]], "env_remove": ["PGHOST"], "filter": {"kind": "none"}, "created": false,
        "notes": ["using postgres-17"]});
    let d = MockDaemon::start(&[("db.command", Ok(cmd))]);
    let out = d
        .yard()
        .args(["db", "shell", "app", "--", "-c", "select 1"])
        .env("PGHOST", "elsewhere")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "args: -d app -c select 1\nPGHOST=\nEXTRA=yes\n");
    assert!(stderr(&out).contains("using postgres-17"));
    let p = &d.params("db.command")[0];
    assert_eq!((p["action"].as_str(), p["name"].as_str()), (Some("shell"), Some("app")));
    assert_eq!(p["args"], json!(["-c", "select 1"]));
}
