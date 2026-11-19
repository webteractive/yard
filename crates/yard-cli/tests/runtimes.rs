//! `yard runtimes`, `yard runtime …`, the Composer offer in `yard link` and the runtime lines of
//! `yard info` (runtimes spec §12).

mod support;

use serde_json::{Value, json};
use support::{MockDaemon, stderr, stdout};

fn runtimes_list() -> Value {
    json!({
        "runtimes": [
            {"key": "composer", "name": "Composer", "origin": "carried", "shims": ["composer"], "installed": ["2.2.32"], "default": "2.2 below PHP 7.2.5, else 2"},
            {"key": "node", "name": "Node", "origin": "carried", "shims": ["node", "npm", "npx"], "installed": [], "default": "22"},
            {"key": "php", "name": "PHP", "origin": "carried", "shims": ["pecl", "php", "php-config", "phpize"], "installed": ["8.4", "7.1"], "default": "8.4"},
        ],
        "errors": [],
    })
}

fn job_events(id: &str) -> Vec<Value> {
    vec![
        json!({"type": "job_progress", "job_id": id, "line": "Installed Composer 2.2.32"}),
        json!({"type": "job_finished", "job_id": id, "error": null}),
    ]
}

fn composer(version: Value) -> Value {
    json!({"key": "composer", "name": "Composer", "spec": "2.2", "source": "default", "version": version})
}

/// A linked PHP site with `runtimes`.
fn site(runtimes: Value) -> Value {
    let path = "/Users/me/code/shop";
    json!({
        "name": "shop", "host": "shop.yard", "url": "http://shop.yard:8080/", "source": "linked",
        "kind": "php", "path": path, "docroot": path, "php": "7.1", "secure": false,
        "needs": [], "runtimes": runtimes,
    })
}

#[test]
fn runtimes_lists_the_carried_three() {
    let d = MockDaemon::start(&[("runtimes.list", Ok(runtimes_list()))]);
    let out = d.yard().args(["runtimes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("RUNTIME"), "{text}");
    for header in ["FROM", "SHIMS", "INSTALLED", "DEFAULT"] {
        assert!(text.lines().next().unwrap().contains(header), "{text}");
    }
    let composer = text.lines().find(|l| l.starts_with("composer")).unwrap();
    assert!(
        composer.contains("carried")
            && composer.contains("2.2.32")
            && composer.contains("2.2 below PHP 7.2.5"),
        "{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("node") && l.contains("node, npm, npx") && l.contains(" - ")),
        "{text}"
    );
    assert!(text.ends_with("For agents and scripts: yard runtimes --json\n"), "{text}");
    let out = d.yard().args(["runtimes", "--json"]).output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), runtimes_list());
}

#[test]
fn runtime_install_composer_runs_a_job() {
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("runtime.install", Ok(json!({"job_id": "runtime-install-1"})))],
        &[("runtime.install", job_events("runtime-install-1"))],
    );
    let out = d.yard().args(["runtime", "install", "composer", "2.2"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).ends_with("Installed composer 2.2.\n"), "{}", stdout(&out));
    assert_eq!(d.params("runtime.install")[0], json!({"key": "composer", "spec": "2.2"}));
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().args(["runtime", "install", "composer"]).current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        d.params("runtime.install")[1],
        json!({"key": "composer", "path": canonical.display().to_string()})
    );
}

#[test]
fn runtime_list_default_use_and_uninstall_send_their_key() {
    let list = json!({"key": "composer", "default": "2", "versions": [{"version": "2.11.2", "default": true, "sites": ["shop"]}]});
    let d = MockDaemon::start_with_events(
        &[
            ("runtime.list", Ok(list.clone())),
            ("runtime.default", Ok(json!(null))),
            ("runtime.use", Ok(site(json!([composer(json!("2.2.32"))])))),
            ("runtime.unuse", Ok(site(json!([composer(json!("2.2.32"))])))),
            ("subscribe", Ok(json!(null))),
            ("runtime.uninstall", Ok(json!({"job_id": "runtime-uninstall-1"}))),
        ],
        &[(
            "runtime.uninstall",
            vec![json!({"type": "job_finished", "job_id": "runtime-uninstall-1", "error": null})],
        )],
    );
    let out = d.yard().args(["runtime", "list", "composer"]).output().unwrap();
    let text = stdout(&out);
    assert!(
        text.lines().any(|l| l.starts_with("2.11.2") && l.contains("yes") && l.contains("shop")),
        "{text}"
    );
    let out = d.yard().args(["runtime", "default", "composer", "2"]).output().unwrap();
    assert_eq!(stdout(&out), "composer 2 is now the default.\n");
    let out = d.yard().args(["runtime", "use", "composer", "2.2", "--site", "shop"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "shop now runs Composer 2.2\n");
    assert_eq!(d.params("runtime.use"), [json!({"key": "composer", "site": "shop", "spec": "2.2"})]);
    let out = d.yard().args(["runtime", "unuse", "composer", "--site", "shop"]).output().unwrap();
    assert_eq!(stdout(&out), "shop runs Composer 2.2 again (default)\n");
    let out = d.yard().args(["runtime", "uninstall", "composer", "2.2.32"]).output().unwrap();
    assert_eq!(stdout(&out), "Removed composer 2.2.32.\n");
    assert_eq!(d.params("runtime.uninstall"), [json!({"key": "composer", "version": "2.2.32"})]);
}

fn detail(runtimes: Value) -> Value {
    json!({"site": site(runtimes), "php_full": "7.1.33", "sources": {"php": "default", "docroot": "default", "secure": "default"}, "services": []})
}

fn info(runtimes: Value) -> String {
    let d = MockDaemon::start(&[("sites.info", Ok(detail(runtimes))), ("ext.list", Ok(json!([])))]);
    let out = d.yard().args(["info", "shop"]).env("HOME", "/Users/me").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    stdout(&out)
}

#[test]
fn info_shows_a_sites_composer() {
    let text = info(json!([composer(json!("2.2.32"))]));
    let row =
        text.lines().find(|l| l.trim_start().starts_with("composer ")).unwrap_or_else(|| panic!("{text}"));
    assert!(row.contains("2.2.32 (2.2)") && row.ends_with("from default"), "{text}");
    let text = info(json!([composer(Value::Null)]));
    assert!(
        text.lines().any(|l| l.trim_start().starts_with("composer ") && l.contains("2.2, not installed")),
        "{text}"
    );
    assert!(text.contains("Composer 2.2  (yard runtime install composer 2.2)"), "{text}");
    let unknown = json!({"key": "bun", "name": "bun", "spec": "1", "source": "project_file", "version": null,
        "error": {"code": "RUNTIME_UNKNOWN", "message": "no runtime is called bun", "hint": "add ~/.yard/runtimes/bun.toml"}});
    let text = info(json!([unknown]));
    assert!(
        text.lines().any(|l| l.trim_start().starts_with("bun ") && l.contains("unknown runtime")),
        "{text}"
    );
    assert!(text.contains("→ add ~/.yard/runtimes/bun.toml"), "{text}");
}

#[test]
fn link_offers_composer_and_installs_it_with_yes() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("sites.link", vec![Ok(site(json!([composer(Value::Null)])))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("runtime.install", vec![Ok(json!({"job_id": "runtime-install-1"}))]),
            ("sites.list", vec![Ok(json!([site(json!([composer(json!("2.2.32"))]))]))]),
        ],
        &[("runtime.install", job_events("runtime-install-1"))],
    );
    let out = d.yard().args(["link", "shop", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.contains("  Composer 2.2  (yard runtime install composer 2.2)\n"), "{so}");
    assert!(so.ends_with("Serving http://shop.yard:8080/\n"), "{so}");
    assert_eq!(d.params("runtime.install"), [json!({"key": "composer", "spec": "2.2"})]);
}

#[test]
fn link_without_yes_and_no_tty_serves_and_hints_composer() {
    let d = MockDaemon::start(&[("sites.link", Ok(site(json!([composer(Value::Null)]))))]);
    let out = d.yard().args(["link", "shop"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("shop runs Composer 2.2, which is not installed: yard runtime install composer 2.2"),
        "{}",
        stderr(&out)
    );
    assert!(d.params("runtime.install").is_empty());
}

#[test]
fn there_is_no_yard_composer_command() {
    let d = MockDaemon::start(&[]);
    let out = d.yard().args(["composer", "install"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}
