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
