//! `yard node`, the Node offer in `yard link`, and the `node` line of `yard info`.

mod support;

use serde_json::{Value, json};
use support::{MockDaemon, stderr, stdout};

fn node_list() -> Value {
    json!({
        "default": "22",
        "versions": [
            {"version": "22.18.0", "default": true, "sites": []},
            {"version": "16.14.0", "default": false, "sites": ["maker"]},
        ],
    })
}

fn job_events(id: &str) -> Vec<Value> {
    vec![
        json!({"type": "job_progress", "job_id": id, "line": "Installing Node 22.18.0"}),
        json!({"type": "job_finished", "job_id": id, "error": null}),
    ]
}

/// A linked PHP site at `path`, with `node` as its Node part.
fn site(path: &str, needs: Value, node: Value) -> Value {
    let mut s = json!({
        "name": "blog", "host": "blog.yard", "url": "http://blog.yard:8080/", "source": "linked",
        "kind": "php", "path": path, "docroot": path, "php": "8.4", "secure": false,
        "project_file": format!("{path}/.yard"), "needs": needs,
    });
    if !node.is_null() {
        s["node"] = node;
    }
    s
}

fn missing(spec: &str) -> Value {
    json!({"spec": spec, "source": "nvmrc", "file": "/Users/me/code/blog/.nvmrc", "version": null})
}

#[test]
fn node_list_prints_versions_default_and_sites() {
    let d = MockDaemon::start(&[("node.list", Ok(node_list()))]);
    let out = d.yard().args(["node", "list"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("VERSION"), "{text}");
    assert!(text.lines().any(|l| l.starts_with("22.18.0") && l.contains("yes")), "{text}");
    assert!(text.lines().any(|l| l.starts_with("16.14.0") && l.contains("maker")), "{text}");
    assert!(text.ends_with("For agents and scripts: yard node list --json\n"), "{text}");
    let out = d.yard().args(["node", "--json"]).output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), node_list());
    d.yard().args(["node", "list", "--available", "16"]).output().unwrap();
    assert_eq!(d.params("node.list").last().unwrap(), &json!({"available": true, "major": 16}));

    let d = MockDaemon::start(&[("node.list", Ok(json!({"default": "22", "versions": []})))]);
    let out = d.yard().args(["node", "list"]).output().unwrap();
    assert!(
        stdout(&out).starts_with("No Node installed. yard node install 22 installs the default.\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn node_install_without_spec_sends_the_cwd() {
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("node.install", Ok(json!({"job_id": "node-install-1"})))],
        &[("node.install", job_events("node-install-1"))],
    );
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().args(["node", "install"]).current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).ends_with("Installed Node for this directory.\n"), "{}", stdout(&out));
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(d.params("node.install")[0], json!({"path": canonical.display().to_string()}));
    let out = d.yard().args(["node", "install", "22"]).output().unwrap();
    assert!(stdout(&out).ends_with("Installed Node 22.\n"), "{}", stdout(&out));
    assert_eq!(d.params("node.install")[1], json!({"spec": "22"}));
}

#[test]
fn node_use_picks_the_site_containing_the_cwd_and_says_when_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().display().to_string();
    let used = json!({"spec": "22", "source": "sites_toml", "version": null});
    let d = MockDaemon::start(&[
        ("sites.list", Ok(json!([site(&path, json!([]), Value::Null)]))),
        ("node.use", Ok(site(&path, json!([]), used))),
        (
            "node.unuse",
            Ok(site(&path, json!([]), json!({"spec": "22", "source": "default", "version": "22.18.0"}))),
        ),
    ]);
    let out = d.yard().args(["node", "use", "22"]).current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "blog now runs Node 22\nNode 22 is not installed: yard node install 22\n");
    assert_eq!(d.params("node.use"), [json!({"site": "blog", "spec": "22"})]);
    let out = d.yard().args(["node", "unuse", "--site", "blog"]).output().unwrap();
    assert_eq!(stdout(&out), "blog runs Node 22 again (default)\n");
    assert_eq!(d.params("node.unuse"), [json!({"site": "blog"})]);
}

#[test]
fn link_with_only_node_missing_and_no_tty_serves_and_hints() {
    let d =
        MockDaemon::start(&[("sites.link", Ok(site("/Users/me/code/blog", json!([]), missing("16.14.0"))))]);
    let out = d.yard().args(["link", "blog"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("blog runs Node 16.14.0, which is not installed: yard node install 16.14.0"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).ends_with("Serving http://blog.yard:8080/\n"), "{}", stdout(&out));
    assert!(d.params("node.install").is_empty());
}

#[test]
fn link_yes_installs_node_after_php() {
    let path = "/Users/me/code/blog";
    let installed = json!({"spec": "16.14.0", "source": "nvmrc", "file": "/Users/me/code/blog/.nvmrc", "version": "16.14.0"});
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("sites.link", vec![Ok(site(path, json!([{"kind": "php", "php": "7.3"}]), missing("16.14.0")))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("php.install", vec![Ok(json!({"job_id": "php-install-1"}))]),
            ("node.install", vec![Ok(json!({"job_id": "node-install-1"}))]),
            (
                "sites.list",
                vec![
                    Ok(json!([site(path, json!([]), missing("16.14.0"))])),
                    Ok(json!([site(path, json!([]), installed)])),
                ],
            ),
        ],
        &[("php.install", job_events("php-install-1")), ("node.install", job_events("node-install-1"))],
    );
    let out = d.yard().args(["link", "blog", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.contains("  Node 16.14.0  (yard node install 16.14.0)\n"), "{so}");
    assert!(so.ends_with("Serving http://blog.yard:8080/\n"), "{so}");
    let order: Vec<String> =
        d.methods().into_iter().filter(|m| m == "php.install" || m == "node.install").collect();
    assert_eq!(order, ["php.install", "node.install"]);
    assert_eq!(d.params("node.install"), [json!({"spec": "16.14.0"})]);
}

fn detail(node: Value) -> Value {
    json!({
        "site": site("/Users/me/code/blog", json!([]), node),
        "php_full": "8.4.1",
        "sources": {"php": "default", "docroot": "default", "secure": "default"},
        "services": [],
    })
}

fn info(node: Value) -> String {
    let d = MockDaemon::start(&[("sites.info", Ok(detail(node))), ("ext.list", Ok(json!([])))]);
    let out = d.yard().args(["info", "blog"]).env("HOME", "/Users/me").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    stdout(&out)
}

fn node_line(text: &str) -> Option<&str> {
    text.lines().find(|l| l.trim_start().starts_with("node "))
}

#[test]
fn info_shows_the_node_line() {
    let ok = json!({"spec": "16.14.0", "source": "nvmrc", "file": "/Users/me/code/blog/.nvmrc", "version": "16.14.0"});
    let text = info(ok);
    let line = node_line(&text).unwrap_or_else(|| panic!("{text}"));
    assert!(line.contains("16.14.0") && line.ends_with("from .nvmrc") && !line.contains("(16"), "{text}");

    let text = info(json!({"spec": "22", "source": "nvmrc", "version": "22.18.0"}));
    assert!(node_line(&text).unwrap().contains("22.18.0 (22)"), "{text}");

    let text = info(missing("16.14.0"));
    assert!(node_line(&text).unwrap().contains("16.14.0, not installed"), "{text}");
    assert!(text.contains("needs       Node 16.14.0  (yard node install 16.14.0)"), "{text}");

    let error = json!({"code": "INVALID_ARGUMENT", "message": "/Users/me/code/blog/.nvmrc line 1: \"latest\" is not a version: name one, such as 22", "hint": "put a version such as 22 in /Users/me/code/blog/.nvmrc"});
    let text = info(json!({"spec": null, "source": "default", "version": null, "error": error}));
    assert!(node_line(&text).unwrap().contains("unusable pin"), "{text}");
    assert!(text.contains("→ put a version such as 22 in /Users/me/code/blog/.nvmrc"), "{text}");

    assert!(node_line(&info(Value::Null)).is_none());
}
