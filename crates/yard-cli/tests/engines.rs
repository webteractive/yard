//! `yard engines`, `yard engines check` and `yard engines install` (recipes spec §9).

mod support;

use serde_json::{Value, json};
use support::{MockDaemon, short_root, stderr, stdout, yard_at};

fn list() -> Value {
    json!({
        "engines": [
            {"key": "mysql", "name": "MySQL", "description": "", "source": "brew", "origin": {"kind": "built_in"},
             "installed": true, "error": null, "ports": [{"name": "main", "usual": 3306}],
             "console": false, "databases": true, "connect": true, "version": null},
            {"key": "redis", "name": "Redis", "description": "", "source": "brew", "origin": {"kind": "carried"},
             "installed": false, "error": null, "ports": [{"name": "main", "usual": 6379}],
             "console": false, "databases": false, "connect": true, "version": null},
            {"key": "typesense", "name": "Typesense", "description": "", "source": "download",
             "origin": {"kind": "user", "file": "/Users/me/.yard/engines/typesense.toml"},
             "installed": false,
             "error": {"code": "ENGINE_RECIPE_INVALID", "message": "/Users/me/.yard/engines/typesense.toml:4: name is empty", "hint": "yard engines check /Users/me/.yard/engines/typesense.toml"},
             "ports": [{"name": "main", "usual": 8108}],
             "console": false, "databases": false, "connect": false, "version": "30.1"},
        ],
        "errors": [
            {"code": "ENGINE_RECIPE_INVALID", "message": "/Users/me/.yard/engines/soketi.toml:2: \"Soketi\" is not an engine key", "hint": "yard engines check /Users/me/.yard/engines/soketi.toml"}
        ],
    })
}

#[test]
fn engines_lists_all_origins() {
    let d = MockDaemon::start(&[("engines.list", Ok(list()))]);
    let out = d.yard().arg("engines").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn engines_json_is_the_wire_type() {
    let d = MockDaemon::start(&[("engines.list", Ok(list()))]);
    let out = d.yard().args(["engines", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v, list());
}

const GOOD: &str = "schema = 1\nkey = \"echo\"\nname = \"Echo\"\n\n[source]\nkind = \"brew\"\nformula = \"echo\"\n\n\
[ports]\nmain = 7000\nadmin = 7001\n\n[run]\ncommand = [\"{home}/bin/echo\", \"--port\", \"{port.main}\"]\n\n\
[health]\ntcp = \"main\"\n\n[console]\nport = \"admin\"\n\n[env]\nlines = [\"ECHO_URL=http://127.0.0.1:{port.main}\"]\n";

#[test]
fn engines_check_good_file() {
    let root = short_root();
    let file = root.path().join("echo.toml");
    std::fs::write(&file, GOOD).unwrap();
    // No yardd answers on this root: the check is offline.
    let out = yard_at(root.path()).args(["engines", "check"]).arg(&file).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("echo.toml loads: Echo (echo), from brew echo"), "{text}");
    assert!(text.contains("echo  7000"), "the main port, named for the instance: {text}");
    assert!(text.contains("echo-admin  7001"), "{text}");
    assert!(text.contains("/bin/echo --port 7000"), "the command with its usual ports: {text}");
    assert!(text.contains("ECHO_URL=http://127.0.0.1:7000"), "{text}");
    assert!(text.contains("echo.yard"), "its console: {text}");
}

#[test]
fn engines_check_reports_line() {
    let root = short_root();
    let file = root.path().join("echo.toml");
    std::fs::write(&file, GOOD.replace("{port.main}\"]", "{port.api}\"]")).unwrap();
    let out = yard_at(root.path()).args(["engines", "check"]).arg(&file).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains("echo.toml:14:") && err.contains("{port.api}"), "{err}");
}

#[test]
fn engines_install_runs_job() {
    let id = "engine-install-1";
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("engines.install", Ok(json!({"job_id": id})))],
        &[(
            "engines.install",
            vec![
                json!({"type": "job_progress", "job_id": id, "line": "downloading https://x/t.tgz"}),
                json!({"type": "job_finished", "job_id": id, "error": null}),
            ],
        )],
    );
    let out = d.yard().args(["engines", "install", "typesense"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("downloading https://x/t.tgz"), "{}", stdout(&out));
    assert_eq!(d.params("engines.install"), [json!({"key": "typesense"})]);
}

#[test]
fn services_shows_unknown_engine() {
    let rows = json!([{
        "name": "ghost", "engine": "ghost", "engine_name": "ghost", "version": null, "formula": "",
        "ports": [{"name": "ghost", "port": 7000}],
        "state": {"failed": {"last_lines": [], "reason": {"code": "ENGINE_UNKNOWN", "message": "no engine named ghost: its recipe is missing", "hint": "restore ~/.yard/engines/ghost.toml, or yard service remove ghost"}}},
        "stopped_by_user": false, "account": {"user": null, "password": null}, "console": null, "used_by": []
    }]);
    let d = MockDaemon::start(&[("services.list", Ok(rows))]);
    let out = d.yard().arg("services").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("no engine named ghost"), "{text}");
}

fn script(dir: &std::path::Path, body: &str) -> String {
    let path = dir.join("client");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn command_for(program: &str) -> Value {
    json!({"instance": "mysql-8.4", "program": program, "args": ["-e", "select 1"], "env": [], "env_remove": [],
        "filter": {"kind": "none"}, "created": false, "notes": []})
}

#[test]
fn yard_mysql_bare_is_shell() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(dir.path(), "echo \"args: $*\"");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program)))]);
    let out = d.yard().arg("mysql").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "args: -e select 1\n");
    let p = &d.params("db.command")[0];
    assert_eq!((p["action"].as_str(), p["engine"].as_str()), (Some("shell"), Some("mysql")));
}

#[test]
fn yard_pgsql_is_postgres() {
    let d = MockDaemon::start(&[("db.list", Ok(json!([])))]);
    let out = d.yard().args(["pgsql", "list", "--service", "postgres-17"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let p = &d.params("db.list")[0];
    assert_eq!((p["engine"].as_str(), p["service"].as_str()), (Some("postgres"), Some("postgres-17")));
}

#[test]
fn yard_mysql_service_of_other_engine_refused() {
    let refused = yard_proto::YardError::new(
        yard_proto::ErrorCode::InvalidArgument,
        "postgres-17 is a postgres instance, not a mysql one",
    );
    let d = MockDaemon::start(&[("db.list", Err(refused))]);
    let out = d.yard().args(["mysql", "list", "--service", "postgres-17"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not a mysql one"), "{}", stderr(&out));
    assert_eq!(d.params("db.list")[0]["engine"], "mysql");
}

#[test]
fn yard_redis_list_names_its_verbs() {
    let root = short_root();
    let out = yard_at(root.path()).args(["redis", "list"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("redis has no list: it has shell, connect, open"), "{}", stderr(&out));
}

#[test]
fn recipe_engine_command_needs_yardd_not_a_known_command() {
    let root = short_root();
    std::fs::create_dir_all(root.path().join("engines")).unwrap();
    let recipe = GOOD.to_string() + "\n[connect]\nurl = \"echo://127.0.0.1:{port.main}\"\n";
    std::fs::write(root.path().join("engines/echo.toml"), recipe).unwrap();
    let out = yard_at(root.path()).args(["echo", "connect"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "yardd is not running, which a known command says: {}",
        stderr(&out)
    );
    let unknown = yard_at(root.path()).arg("nope").output().unwrap();
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("nope"), "{}", stderr(&unknown));
}

#[test]
fn yard_db_still_works_with_notice() {
    let d = MockDaemon::start(&[("db.list", Ok(json!([])))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("yard db is going away: use yard <engine> <verb>"), "{}", stderr(&out));
    assert!(d.params("db.list")[0].get("engine").is_none());
}

#[test]
fn help_lists_engine_commands() {
    let root = short_root();
    let out = yard_at(root.path()).arg("--help").output().unwrap();
    let text = stdout(&out);
    assert!(
        text.contains("yard mysql") && text.contains("yard postgres") && text.contains("yard redis"),
        "{text}"
    );
    assert!(!text.contains("\n  db "), "yard db is hidden: {text}");
}
