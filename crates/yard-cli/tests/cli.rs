mod support;

use serde_json::{Value, json};
use std::path::Path;
use support::{MOCK_VERSION, MockDaemon, free_port, short_root, stderr, stdout, yard_at};
use tempfile::TempDir;
use yard_proto::{CaInfo, ErrorCode, SiteInfo, SiteKind, SiteSource, YardError};

/// `yard` against an empty data root, so no daemon answers.
fn offline(args: &[&str]) -> (tempfile::TempDir, std::process::Output) {
    let root = short_root();
    let out = yard_at(root.path()).args(args).output().unwrap();
    (root, out)
}

fn first_line(out: &std::process::Output) -> String {
    stdout(out).lines().next().unwrap_or_default().to_string()
}

fn sites() -> Vec<SiteInfo> {
    vec![
        SiteInfo {
            name: "blog".into(),
            host: "blog.yard".into(),
            url: "http://blog.yard:8080/".into(),
            source: SiteSource::Linked,
            kind: SiteKind::Php {
                path: "/Users/me/code/blog".into(),
                docroot: "/Users/me/code/blog/public".into(),
                php: "8.4".into(),
            },
            secure: false,
            project_file: None,
            project_file_kind: None,
            ignored_managed_file: None,
            needs: vec![],
            project_error: None,
            node: None,
        },
        SiteInfo {
            name: "api".into(),
            host: "api.yard".into(),
            url: "http://api.yard:8080/".into(),
            source: SiteSource::Proxy,
            kind: SiteKind::Proxy { target: "http://127.0.0.1:3000".into() },
            secure: false,
            project_file: None,
            project_file_kind: None,
            ignored_managed_file: None,
            needs: vec![],
            project_error: None,
            node: None,
        },
    ]
}

/// Free ports for every default entry, so an offline probe never touches 8080, 8443 or 53535.
fn write_free_ports(root: &std::path::Path, http: u16) {
    let ports = format!(
        "[ports]\nnginx-http = {{ port = {http}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n",
        free_port(),
        free_port()
    );
    std::fs::write(root.join("ports.toml"), ports).unwrap();
}

#[test]
fn version_flag_prints_version_and_commit() {
    let (_root, out) = offline(&["--version"]);
    assert!(out.status.success());
    let line = first_line(&out);
    // Every release bumps the workspace version, so the test reads it rather than naming one.
    assert!(line.starts_with(&format!("yard {} (", env!("CARGO_PKG_VERSION"))), "{line}");
    assert!(line.ends_with(')'), "{line}");
}

#[test]
fn short_flag_and_subcommand_match_long_flag() {
    let (_a, long) = offline(&["--version"]);
    let (_b, short) = offline(&["-V"]);
    let (_c, sub) = offline(&["version"]);
    assert!(long.status.success() && short.status.success() && sub.status.success());
    assert!(first_line(&long).starts_with("yard "), "{}", first_line(&long));
    assert_eq!(first_line(&short), first_line(&long));
    assert_eq!(first_line(&sub), first_line(&long));
}

#[test]
fn version_json_has_stable_shape() {
    let (_root, out) = offline(&["version", "--json"]);
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["cli"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["protocol"], "2.11");
    assert!(v["commit"].is_string());
    assert!(v["build_date"].is_string());
    assert!(v["daemon"].is_null());
    assert!(v["helper"].is_null());
    assert_eq!(v["needs_helper_revision"], yard_helper::HELPER_REVISION);
}

#[test]
fn version_reports_daemon_when_running() {
    let status = json!({
        "version": {"cli": null, "daemon": MOCK_VERSION, "helper": "0.1.0", "protocol": "1.0", "commit": "mock", "build_date": "2026-10-19"},
        "profile": "standard", "tld": "yard", "units": [], "sites": 0, "config_error": null
    });
    let d = MockDaemon::start(&[("status", Ok(status))]);
    let out = d.yard().args(["version", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["daemon"], MOCK_VERSION);
    assert_eq!(v["helper"], "0.1.0");

    let human = stdout(&d.yard().arg("--version").output().unwrap());
    assert!(human.contains(&format!("yardd {MOCK_VERSION}")), "{human}");
}

#[test]
fn sites_human_table_snapshot() {
    let d = MockDaemon::start(&[("sites.list", Ok(serde_json::to_value(sites()).unwrap()))]);
    let out = d.yard().arg("sites").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn ports_list_warns_when_ports_toml_is_invalid() {
    let infos =
        json!([{"unit": "dnsmasq", "port": 53535, "mode": "pinned", "status": "yard", "holder": null}]);
    let bad = json!({
        "code": "CONFIG_INVALID",
        "message": "/r/ports.toml line 9, column 1: port 53535 is registered to both dnsmasq and redis",
        "hint": "fix /r/ports.toml",
        "data": {"file": "/r/ports.toml", "line": 9, "column": 1}
    });
    let status = json!({
        "version": {"cli": null, "daemon": MOCK_VERSION, "helper": null, "protocol": "1.0", "commit": "mock", "build_date": "2026-10-19"},
        "profile": "standard", "tld": "yard", "units": [], "sites": 0, "config_error": bad
    });
    let d = MockDaemon::start(&[("ports.list", Ok(infos)), ("status", Ok(status))]);
    let out = d.yard().arg("ports").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("warning: /r/ports.toml does not parse"), "{err}");
    assert!(err.contains("last good registry"), "{err}");
    assert!(err.contains("registered to both dnsmasq and redis"), "{err}");
}

#[test]
fn ports_list_is_quiet_when_status_has_no_ports_error() {
    let infos = json!([]);
    let d = MockDaemon::start(&[("ports.list", Ok(infos))]);
    let out = d.yard().arg("ports").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "", "a status call that fails is not a warning");
}

#[test]
fn sites_json_is_proto_shape() {
    let d = MockDaemon::start(&[("sites.list", Ok(serde_json::to_value(sites()).unwrap()))]);
    let out = d.yard().args(["sites", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let parsed: Vec<SiteInfo> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed, sites());
}

#[test]
fn empty_sites_says_what_to_do() {
    let d = MockDaemon::start(&[("sites.list", Ok(json!([])))]);
    let out = d.yard().arg("sites").output().unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("No sites yet. Serve a project with yard link, run inside it."), "{text}");
    assert!(text.contains("For agents and scripts: yard sites --json"), "{text}");
}

#[test]
fn daemon_down_exits_3_with_message() {
    let (_root, out) = offline(&["sites"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("yardd is not running → yard start"), "{}", stderr(&out));

    let (_root, out) = offline(&["sites", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "DAEMON_NOT_RUNNING");
}

#[test]
fn yard_error_prints_message_and_hint_exit_1() {
    let e = YardError::new(ErrorCode::PortInUse, "port 8080 is held by Herd (pid 12, herd)")
        .with_hint("quit Herd, or yard ports set nginx-http <port>");
    let d = MockDaemon::start(&[("units.restart", Err(e.clone()))]);

    let out = d.yard().args(["restart", "nginx"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "error: port 8080 is held by Herd (pid 12, herd)\n  hint: quit Herd, or yard ports set nginx-http <port>\n"
    );
    assert_eq!(d.params("units.restart"), vec![json!({"unit": "nginx"})]);

    let out = d.yard().args(["restart", "nginx", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(serde_json::from_value::<YardError>(v["error"].clone()).unwrap(), e);
}

#[test]
fn ports_check_works_offline() {
    let root = short_root();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let held = listener.local_addr().unwrap().port();
    write_free_ports(root.path(), held);

    let out = yard_at(root.path()).args(["ports", "check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    let row = text.lines().find(|l| l.starts_with("nginx-http ")).unwrap_or_else(|| panic!("{text}"));
    assert!(row.contains(&held.to_string()) && row.contains("in use"), "{row}");
    assert!(row.contains(&format!("pid {}", std::process::id())), "{row}");
    drop(listener);
}

#[test]
fn doctor_offline_runs_local_checks() {
    let root = short_root();
    write_free_ports(root.path(), free_port());

    // No brew here: doctor asks brew about trust and service formulae, and this test must
    // not run the machine's own.
    let brew = root.path().join("no-brew");
    let out = yard_at(root.path()).env("YARD_BREW_PREFIX", &brew).arg("doctor").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    for id in ["brew", "formula.nginx", "config", "daemon", "ports"] {
        assert!(text.lines().any(|l| l.split_whitespace().nth(1) == Some(id)), "{id} in {text}");
    }
    assert!(text.contains("yardd is not running") && text.contains("→ yard start"), "{text}");

    let out =
        yard_at(root.path()).env("YARD_BREW_PREFIX", &brew).args(["doctor", "--json"]).output().unwrap();
    let report: yard_proto::DoctorReport = serde_json::from_slice(&out.stdout).unwrap();
    let daemon = report.checks.iter().find(|c| c.id == "daemon").unwrap();
    assert_eq!(daemon.status, yard_proto::CheckStatus::Fail);
}

#[test]
fn link_defaults_name_from_cwd() {
    let site = sites().remove(0);
    let d = MockDaemon::start(&[("sites.link", Ok(serde_json::to_value(&site).unwrap()))]);
    let projects = tempfile::tempdir().unwrap();
    let cwd = projects.path().join("My App");
    std::fs::create_dir(&cwd).unwrap();

    let out = d.yard().arg("link").current_dir(&cwd).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let params = d.params("sites.link");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["name"], "my-app");
    assert!(params[0]["path"].as_str().unwrap().ends_with("/My App"), "{params:?}");
}

#[test]
fn helper_install_requires_root() {
    // SAFETY: geteuid cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let (_root, out) = offline(&["helper", "install"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("run with sudo"), "{}", stderr(&out));
}

#[test]
fn park_is_not_a_command() {
    for cmd in ["park", "unpark"] {
        let (_root, out) = offline(&[cmd]);
        assert_eq!(out.status.code(), Some(2), "{cmd}");
    }
}

#[test]
fn every_command_has_help() {
    for cmd in [
        "start", "stop", "status", "version", "doctor", "sites", "link", "unlink", "proxy", "unproxy",
        "docroot", "ports", "restart", "helper", "init",
    ] {
        let (_root, out) = offline(&[cmd, "--help"]);
        assert!(out.status.success(), "{cmd}");
        let text = stdout(&out);
        assert!(text.lines().next().is_some_and(|l| !l.trim().is_empty()), "{cmd}: {text}");
        assert!(text.contains("--json"), "{cmd}: {text}");
    }
}

#[test]
fn profile_accepts_old_names() {
    let d = MockDaemon::start(&[("ports.profile", Ok(json!(null)))]);
    let out = d.yard().args(["ports", "profile", "trial"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("ports.profile"), vec![json!({"profile": "standard"})]);
}

fn php_list() -> Value {
    json!([
        {"version": "7.1", "default": false, "installed": true, "sites": ["legacy"], "pool": "running"},
        {"version": "8.4", "default": true, "installed": true, "sites": ["blog", "shop"], "pool": "running"},
        {"version": "8.5", "default": false, "installed": true, "sites": [], "pool": null}
    ])
}

#[test]
fn php_list_table_snapshot() {
    let d = MockDaemon::start(&[("php.list", Ok(php_list()))]);
    let out = d.yard().args(["php", "list"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    let bare = d.yard().arg("php").output().unwrap();
    assert_eq!(stdout(&bare), stdout(&out), "yard php lists too");
}

#[test]
fn php_list_available_sends_the_flag() {
    let mut list = php_list();
    list.as_array_mut().unwrap().insert(
        0,
        json!({"version": "7.0", "default": false, "installed": false, "sites": [], "pool": null}),
    );
    let d = MockDaemon::start(&[("php.list", Ok(list))]);
    let out = d.yard().args(["php", "list", "--available"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("php.list"), vec![json!({"available": true})]);
    assert!(stdout(&out).contains("not installed → yard php install 7.0"), "{}", stdout(&out));
}

fn job_events(id: &str, error: Value) -> Vec<Value> {
    vec![
        json!({"type": "job_progress", "job_id": id, "line": "==> Downloading php@7.1"}),
        json!({"type": "job_progress", "job_id": "other-9", "line": "not this job"}),
        json!({"type": "job_progress", "job_id": id, "line": "==> Pouring php@7.1"}),
        json!({"type": "job_finished", "job_id": id, "error": error}),
    ]
}

#[test]
fn php_install_streams_progress() {
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("php.install", Ok(json!({"job_id": "php-install-1"})))],
        &[("php.install", job_events("php-install-1", json!(null)))],
    );
    let out = d.yard().args(["php", "install", "7.1"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("==> Downloading php@7.1\n==> Pouring php@7.1\n"), "{text}");
    assert!(!text.contains("not this job"), "{text}");
    assert!(text.contains("Installed PHP 7.1."), "{text}");
    assert_eq!(d.params("subscribe"), [json!({"topics": ["jobs"]})]);
    assert_eq!(d.params("php.install"), [json!({"php": "7.1"})]);
}

#[test]
fn php_install_failure_exits_1_with_the_hint() {
    let error = json!({"code": "JOB_FAILED", "message": "brew install shivammathur/php/php@7.1 exited with 1", "hint": "brew doctor"});
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("php.install", Ok(json!({"job_id": "php-install-1"})))],
        &[("php.install", job_events("php-install-1", error))],
    );
    let out = d.yard().args(["php", "install", "7.1"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("==> Pouring php@7.1"), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("error: brew install shivammathur/php/php@7.1 exited with 1"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("hint: brew doctor"), "{}", stderr(&out));
}

#[test]
fn php_install_json_prints_each_event() {
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("php.install", Ok(json!({"job_id": "php-install-1"})))],
        &[("php.install", job_events("php-install-1", json!(null)))],
    );
    let out = d.yard().args(["php", "install", "7.1", "--json"]).output().unwrap();
    assert!(out.status.success());
    let events: Vec<Value> = stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[2]["type"], "job_finished");
}

#[test]
fn a_job_result_leaves_run_job_output_unchanged() {
    let mut events = job_events("php-install-1", json!(null));
    events[3]["result"] = json!({"site": "blog"});
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("php.install", Ok(json!({"job_id": "php-install-1"})))],
        &[("php.install", events.clone())],
    );
    let out = d.yard().args(["php", "install", "7.1"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "==> Downloading php@7.1\n==> Pouring php@7.1\nInstalled PHP 7.1.\n");
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("php.install", Ok(json!({"job_id": "php-install-1"})))],
        &[("php.install", events)],
    );
    let out = d.yard().args(["php", "install", "7.1", "--json"]).output().unwrap();
    let last: Value = serde_json::from_str(stdout(&out).lines().last().unwrap()).unwrap();
    assert_eq!(last["result"], json!({"site": "blog"}));
}

#[test]
fn php_default_and_uninstall_send_the_version() {
    let d = MockDaemon::start(&[("php.default", Ok(json!(null)))]);
    let out = d.yard().args(["php", "default", "8.3"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("PHP 8.3 is now the default."), "{}", stdout(&out));
    assert_eq!(d.params("php.default"), [json!({"php": "8.3"})]);
}

fn php_site(name: &str, path: &str, php: &str) -> Value {
    serde_json::to_value(SiteInfo {
        name: name.into(),
        host: format!("{name}.yard"),
        url: format!("http://{name}.yard:8080/"),
        source: SiteSource::Linked,
        kind: SiteKind::Php { path: path.into(), docroot: path.into(), php: php.into() },
        secure: false,
        project_file: None,
        project_file_kind: None,
        ignored_managed_file: None,
        needs: vec![],
        project_error: None,
        node: None,
    })
    .unwrap()
}

#[test]
fn isolate_defaults_site_from_cwd() {
    let project = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(project.path()).unwrap();
    let path = dir.display().to_string();
    let d = MockDaemon::start(&[
        ("sites.list", Ok(json!([php_site("old", &path, "8.4")]))),
        ("sites.isolate", Ok(php_site("old", &path, "7.1"))),
    ]);
    std::fs::create_dir(dir.join("app")).unwrap();
    let out = d.yard().args(["isolate", "7.1"]).current_dir(dir.join("app")).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("sites.isolate"), [json!({"site": "old", "php": "7.1"})]);
    assert!(stdout(&out).contains("old now runs PHP 7.1"), "{}", stdout(&out));

    let out = d.yard().args(["unisolate", "--site", "old"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("sites.isolate")[1], json!({"site": "old", "php": null}));
}

#[test]
fn isolate_outside_a_site_names_the_flag() {
    let d = MockDaemon::start(&[("sites.list", Ok(json!([])))]);
    let elsewhere = tempfile::tempdir().unwrap();
    let out = d.yard().args(["isolate", "7.1"]).current_dir(elsewhere.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("hint: yard isolate 7.1 --site <name>"), "{}", stderr(&out));
}

#[test]
fn php_config_set_get_unset() {
    let d = MockDaemon::start(&[
        ("php.config.set", Ok(json!(null))),
        ("php.config.get", Ok(json!("1G"))),
        ("php.config.unset", Ok(json!(false))),
    ]);
    let out = d.yard().args(["php", "config", "set", "memory_limit", "1G", "--php", "7.1"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("memory_limit = 1G for PHP 7.1"), "{}", stdout(&out));
    assert_eq!(d.params("php.config.set"), [json!({"php": "7.1", "key": "memory_limit", "value": "1G"})]);

    let out = d.yard().args(["php", "config", "get", "memory_limit"]).output().unwrap();
    assert_eq!(stdout(&out), "1G\n");
    assert_eq!(d.params("php.config.get"), [json!({"php": null, "key": "memory_limit", "value": null})]);

    let out = d.yard().args(["php", "config", "unset", "memory_limit"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "memory_limit was not overridden.\n");
}

#[test]
fn php_config_get_unset_key_says_so() {
    let d = MockDaemon::start(&[("php.config.get", Ok(json!(null)))]);
    let out = d.yard().args(["php", "config", "get", "memory_limit"]).output().unwrap();
    assert!(out.status.success());
    assert!(stdout(&out).contains("memory_limit is not overridden"), "{}", stdout(&out));
}

#[test]
fn ext_add_streams_and_reports_verify_failure() {
    let error = json!({
        "code": "EXT_VERIFY_FAILED",
        "message": "redis did not load in PHP 8.4",
        "hint": "yard ext remove redis --php 8.4",
        "data": {"output": "PHP Warning:  Unable to load dynamic library 'redis.so'"}
    });
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("ext.add", Ok(json!({"job_id": "ext-add-1"})))],
        &[("ext.add", job_events("ext-add-1", error))],
    );
    let out = d.yard().args(["ext", "add", "redis", "--php", "8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("==> Pouring php@7.1"), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(
        err.contains("error: redis did not load in PHP 8.4\n  hint: yard ext remove redis --php 8.4"),
        "{err}"
    );
    assert_eq!(d.params("ext.add"), [json!({"name": "redis", "php": "8.4"})]);
}

#[test]
fn ext_add_sends_a_relative_so_as_an_absolute_path() {
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("ext.add", Ok(json!({"job_id": "ext-add-1"})))],
        &[("ext.add", job_events("ext-add-1", json!(null)))],
    );
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().current_dir(dir.path()).args(["ext", "add", "build/custom.so"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Added custom to the default PHP."), "{}", stdout(&out));
    let cwd = dir.path().canonicalize().unwrap();
    let sent = d.params("ext.add")[0]["name"].as_str().unwrap().to_string();
    assert_eq!(std::path::Path::new(&sent).canonicalize().ok(), None, "the file need not exist yet");
    assert!(sent.ends_with("/build/custom.so") && sent.starts_with('/'), "{sent}");
    assert!(sent.contains(cwd.file_name().unwrap().to_str().unwrap()), "{sent} under {}", cwd.display());
}

#[test]
fn ext_list_table_snapshot() {
    let d = MockDaemon::start(&[(
        "ext.list",
        Ok(json!([
            {"name": "opcache", "source": "bundled", "enabled": true},
            {"name": "redis", "source": "brew", "enabled": true},
            {"name": "xdebug", "source": "pecl", "enabled": false}
        ])),
    )]);
    let out = d.yard().args(["ext", "list", "--php", "8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    assert_eq!(d.params("ext.list"), [json!({"php": "8.4"})]);
}

#[test]
fn ext_toggles_and_xdebug_send_their_params() {
    let d = MockDaemon::start(&[
        ("ext.enable", Ok(json!(null))),
        ("ext.disable", Ok(json!(null))),
        ("xdebug.set", Ok(json!(null))),
    ]);
    let out = d.yard().args(["ext", "disable", "redis"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Disabled redis for the default PHP."), "{}", stdout(&out));
    let out = d.yard().args(["ext", "enable", "redis", "--php", "8.3"]).output().unwrap();
    assert!(stdout(&out).contains("Enabled redis for PHP 8.3."), "{}", stdout(&out));
    let out = d.yard().args(["xdebug", "off"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Xdebug is off for the default PHP."), "{}", stdout(&out));
    assert_eq!(d.params("ext.disable"), [json!({"name": "redis"})]);
    assert_eq!(d.params("ext.enable"), [json!({"name": "redis", "php": "8.3"})]);
    assert_eq!(d.params("xdebug.set"), [json!({"on": false})]);
}

fn php_on(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("php"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(dir.join("php"), std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn install_shims_prints_the_path_line() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    php_on(&bin);
    let out = yard_at(root.path())
        .arg("install-shims")
        .env("PATH", format!("{}:/usr/bin", bin.display()))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with(&format!("export PATH=\"{}:$PATH\"\n", bin.display())), "{text}");
    assert!(text.contains("shell profile"), "{text}");
    assert!(!stderr(&out).contains("warning"), "{}", stderr(&out));

    let out =
        yard_at(root.path()).args(["install-shims", "--json"]).env("PATH", "/nonexistent").output().unwrap();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["bin"], bin.display().to_string());
    assert_eq!(v["line"], format!("export PATH=\"{}:$PATH\"", bin.display()));
    assert_eq!(v["first_php"], Value::Null);
}

#[test]
fn install_shims_warns_when_herd_php_comes_first() {
    let root = tempfile::tempdir().unwrap();
    let herd = tempfile::tempdir().unwrap();
    let herd_bin = herd.path().join("Library/Application Support/Herd/bin");
    php_on(&herd_bin);
    let out = yard_at(root.path())
        .arg("install-shims")
        .env("PATH", format!("{}:{}/bin", herd_bin.display(), root.path().display()))
        .output()
        .unwrap();
    assert!(out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains(&format!(
            "warning: php on your PATH is {} (Herd's) — Yard's bin must come before it",
            herd_bin.join("php").display()
        )),
        "{err}"
    );
}

/// Kills `pid` if it still runs after 60 s, so a hung command fails its test instead of
/// stalling the suite. Hold the guard until the child is reaped: dropping it calls the kill off.
fn watchdog(pid: libc::pid_t) -> std::sync::mpsc::Sender<()> {
    let (guard, dropped) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if dropped.recv_timeout(std::time::Duration::from_secs(60))
            == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            // SAFETY: signalling our own child process, which has not been reaped yet.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    });
    guard
}

/// Starts `yard php install 7.1`, waits for its first output line, and sends SIGINT.
fn interrupted_install(d: &MockDaemon, times: usize) -> std::process::Output {
    use std::io::{BufRead, BufReader};
    let mut child = d
        .yard_process()
        .args(["php", "install", "7.1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as libc::pid_t;
    let guard = watchdog(pid);
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    assert!(first.contains("Downloading"), "{first}");
    for _ in 0..times {
        // SAFETY: signalling our own child process.
        unsafe { libc::kill(pid, libc::SIGINT) };
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let output = child.wait_with_output().unwrap();
    drop(guard);
    assert_ne!(
        std::os::unix::process::ExitStatusExt::signal(&output.status),
        Some(libc::SIGKILL),
        "yard was still running 60 s after Ctrl-C"
    );
    output
}

#[test]
fn ctrl_c_cancels_the_job_and_waits_for_it() {
    let cancelled = json!({"code": "JOB_CANCELLED", "message": "php-install-1 was cancelled"});
    let d = MockDaemon::start_with_events(
        &[
            ("subscribe", Ok(json!(null))),
            ("php.install", Ok(json!({"job_id": "php-install-1"}))),
            ("jobs.cancel", Ok(json!(null))),
        ],
        &[
            (
                "php.install",
                vec![
                    json!({"type": "job_progress", "job_id": "php-install-1", "line": "==> Downloading php@7.1"}),
                ],
            ),
            (
                "jobs.cancel",
                vec![json!({"type": "job_finished", "job_id": "php-install-1", "error": cancelled})],
            ),
        ],
    );
    let out = interrupted_install(&d, 1);
    assert_eq!(out.status.code(), Some(1), "{:?}: {}", out.status, stderr(&out));
    assert_eq!(d.params("jobs.cancel"), [json!({"job_id": "php-install-1"})]);
    let err = stderr(&out);
    assert!(err.contains("Cancelling php-install-1"), "{err}");
    assert!(err.contains("error: php-install-1 was cancelled"), "{err}");
}

#[test]
fn a_second_ctrl_c_stops_waiting() {
    let d = MockDaemon::start_with_events(
        &[
            ("subscribe", Ok(json!(null))),
            ("php.install", Ok(json!({"job_id": "php-install-1"}))),
            ("jobs.cancel", Ok(json!(null))),
        ],
        &[(
            "php.install",
            vec![
                json!({"type": "job_progress", "job_id": "php-install-1", "line": "==> Downloading php@7.1"}),
            ],
        )],
    );
    let out = interrupted_install(&d, 2);
    assert_eq!(out.status.code(), Some(130), "{:?}: {}", out.status, stderr(&out));
}

#[test]
fn bare_yard_prints_the_command_list() {
    let (_root, out) = offline(&[]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("Usage: yard"), "{err}");
    assert!(err.contains("Commands:") && err.contains("link"), "{err}");
}

fn secured_site(name: &str, path: &str) -> Value {
    let mut site = php_site(name, path, "8.4");
    site["secure"] = json!(true);
    site["url"] = json!(format!("https://{name}.yard:8443/"));
    site
}

fn ca_info(trusted: bool) -> Value {
    json!({"cert": "/r/ca/yard-ca.pem", "sha256": "AB12", "trusted": trusted, "expires": "2036-10-19"})
}

/// A `security` that appends its arguments to `security.log` beside it and exits `code`.
fn recording_security(code: i32) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("security");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/security.log\"\nexit {code}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn security_log(dir: &TempDir) -> Option<String> {
    std::fs::read_to_string(dir.path().join("security.log")).ok()
}

/// `yard <args>` with `security` from `dir` and `HOME` at `home`.
fn yard_with_security(d: &MockDaemon, dir: &TempDir, home: &Path, args: &[&str]) -> std::process::Output {
    d.yard().env("YARD_SECURITY", dir.path().join("security")).env("HOME", home).args(args).output().unwrap()
}

#[test]
fn secure_prints_the_https_url_and_the_trust_hint() {
    let d = MockDaemon::start(&[
        ("sites.secure", Ok(secured_site("blog", "/p/blog"))),
        ("sites.unsecure", Ok(php_site("blog", "/p/blog", "8.4"))),
        ("ca.info", Ok(ca_info(false))),
    ]);
    let out = d.yard().args(["secure", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("sites.secure"), [json!({"site": "blog"})]);
    assert_eq!(
        stdout(&out),
        "Secured blog → https://blog.yard:8443/\nThe Yard CA is not trusted yet, so browsers will warn. \
         Run yard trust (macOS asks for your password once).\n"
    );

    let out = d.yard().args(["secure", "blog", "--json"]).output().unwrap();
    let site: SiteInfo = serde_json::from_slice(&out.stdout).unwrap();
    assert!(site.secure);

    let out = d.yard().args(["unsecure", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "blog is back on http://blog.yard:8080/\n");
    let out = d.yard().args(["unsecure", "blog", "--json"]).output().unwrap();
    let site: SiteInfo = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!site.secure);
}

#[test]
fn secure_defaults_to_the_site_containing_the_cwd() {
    let project = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(project.path()).unwrap();
    let path = dir.display().to_string();
    let d = MockDaemon::start(&[
        ("sites.list", Ok(json!([php_site("blog", &path, "8.4")]))),
        ("sites.secure", Ok(secured_site("blog", &path))),
        ("ca.info", Ok(ca_info(true))),
    ]);
    let out = d.yard().arg("secure").current_dir(&dir).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("sites.secure"), [json!({"site": "blog"})]);
    assert_eq!(stdout(&out), "Secured blog → https://blog.yard:8443/\n", "no trust hint once trusted");

    let elsewhere = tempfile::tempdir().unwrap();
    let out = d.yard().arg("unsecure").current_dir(elsewhere.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("hint: yard unsecure <site>"), "{}", stderr(&out));
}

#[test]
fn trust_runs_add_trusted_cert_on_the_login_keychain() {
    let d = MockDaemon::start_sequenced(&[("ca.info", vec![Ok(ca_info(false)), Ok(ca_info(true))])]);
    let security = recording_security(0);
    let home = tempfile::tempdir().unwrap();
    let out = yard_with_security(&d, &security, home.path(), &["trust"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Trusted the Yard CA in your login keychain.\n");
    let keychain = home.path().join("Library/Keychains/login.keychain-db");
    assert_eq!(
        security_log(&security).unwrap(),
        format!("add-trusted-cert -r trustRoot -k {} /r/ca/yard-ca.pem\n", keychain.display())
    );

    let d = MockDaemon::start_sequenced(&[("ca.info", vec![Ok(ca_info(false)), Ok(ca_info(true))])]);
    let out = yard_with_security(&d, &security, home.path(), &["trust", "--json"]);
    let info: CaInfo = serde_json::from_slice(&out.stdout).unwrap();
    assert!(info.trusted);
}

#[test]
fn trust_when_trusted_runs_nothing() {
    let d = MockDaemon::start(&[("ca.info", Ok(ca_info(true)))]);
    let security = recording_security(0);
    let home = tempfile::tempdir().unwrap();
    let out = yard_with_security(&d, &security, home.path(), &["trust"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "The Yard CA is already trusted.\n");
    assert_eq!(security_log(&security), None);
}

#[test]
fn trust_that_does_not_take_is_ca_not_trusted() {
    let home = tempfile::tempdir().unwrap();
    for (code, message) in [(0, "the CA is still not trusted"), (3, "security exited with 3")] {
        let d = MockDaemon::start(&[("ca.info", Ok(ca_info(false)))]);
        let security = recording_security(code);
        let out = yard_with_security(&d, &security, home.path(), &["trust"]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains(&format!("error: {message}")) && err.contains("hint: yard trust"), "{err}");

        let out = yard_with_security(&d, &security, home.path(), &["trust", "--json"]);
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["error"]["code"], "CA_NOT_TRUSTED", "{v}");
    }
}

/// A CA file in the mock's data root, as yardd leaves one once it has made it.
fn with_ca(d: &MockDaemon) {
    std::fs::create_dir_all(d.root.path().join("ca")).unwrap();
    std::fs::write(d.root.path().join("ca/yard-ca.pem"), "").unwrap();
}

#[test]
fn untrust_deletes_by_sha256() {
    let d = MockDaemon::start_sequenced(&[("ca.info", vec![Ok(ca_info(true)), Ok(ca_info(false))])]);
    with_ca(&d);
    let security = recording_security(0);
    let home = tempfile::tempdir().unwrap();
    let out = yard_with_security(&d, &security, home.path(), &["untrust"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Removed the Yard CA from your login keychain.\n");
    let keychain = home.path().join("Library/Keychains/login.keychain-db");
    assert_eq!(
        security_log(&security).unwrap(),
        format!("delete-certificate -Z AB12 -t {}\n", keychain.display())
    );

    let out = yard_with_security(&d, &security, home.path(), &["untrust"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "The Yard CA is not in your login keychain.\n");
    let out = yard_with_security(&d, &security, home.path(), &["untrust", "--json"]);
    let info: CaInfo = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!info.trusted);
}

#[test]
fn untrust_that_does_not_take_is_ca_still_trusted() {
    let home = tempfile::tempdir().unwrap();
    for (code, message) in [
        (0, "the CA is still trusted: its trust is set outside your login keychain"),
        (44, "security exited with 44, and the CA is still trusted"),
    ] {
        let d = MockDaemon::start(&[("ca.info", Ok(ca_info(true)))]);
        with_ca(&d);
        let security = recording_security(code);
        let out = yard_with_security(&d, &security, home.path(), &["untrust"]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains(&format!("error: {message}")), "{err}");
        assert!(err.contains("hint: sudo security remove-trusted-cert -d /r/ca/yard-ca.pem"), "{err}");

        let out = yard_with_security(&d, &security, home.path(), &["untrust", "--json"]);
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["error"]["code"], "CA_STILL_TRUSTED", "{v}");
    }
}

#[test]
fn untrust_without_a_ca_makes_none() {
    let d = MockDaemon::start(&[("ca.info", Ok(ca_info(false)))]);
    let security = recording_security(0);
    let home = tempfile::tempdir().unwrap();
    let out = yard_with_security(&d, &security, home.path(), &["untrust"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "There is no Yard CA, so there is nothing to untrust.\n");
    let out = yard_with_security(&d, &security, home.path(), &["untrust", "--json"]);
    assert_eq!(stdout(&out), "null\n");
    assert!(d.params("ca.info").is_empty(), "ca.info would make a CA");
    assert_eq!(security_log(&security), None);
}

/// `blog` served from its `.yard`, needing what `needs` says.
fn project_site(needs: Value) -> Value {
    let mut site = serde_json::to_value(sites().remove(0)).unwrap();
    site["project_file"] = json!("/Users/me/code/blog/.yard");
    site["needs"] = needs;
    site
}

fn init_result(site: Value) -> Value {
    json!({
        "file": "/Users/me/code/blog/.yard",
        "text": "# .yard: how Yard serves this project. Commit it.\nphp = \"7.3\"\ndocroot = \"public\"\n",
        "written": true,
        "site": site,
    })
}

#[test]
fn init_without_needs_prints_file_and_url() {
    let d = MockDaemon::start(&[("sites.init", Ok(init_result(project_site(json!([])))))]);
    let projects = tempfile::tempdir().unwrap();
    let out = d.yard().arg("init").current_dir(projects.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Wrote /Users/me/code/blog/.yard:\n  # .yard: how Yard serves this project. Commit it.\n  php = \"7.3\"\n  docroot = \"public\"\nServing http://blog.yard:8080/\n"
    );
    let params = d.params("sites.init");
    assert_eq!(params.len(), 1);
    assert_eq!((params[0]["name"].clone(), params[0]["force"].clone()), (json!(null), json!(false)));
    assert_eq!(params[0]["local"], false, "the managed file is the default");
    let path = Path::new(params[0]["path"].as_str().unwrap()).canonicalize().unwrap();
    assert_eq!(path, projects.path().canonicalize().unwrap());
}

#[test]
fn init_local_sends_local_and_prints_a_move() {
    let mut result = init_result(project_site(json!([])));
    result["from_managed"] = json!("/r/sites/blog/.yard");
    let d = MockDaemon::start(&[("sites.init", Ok(result))]);
    let out = d.yard().args(["init", "--local"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("Moved /r/sites/blog/.yard to /Users/me/code/blog/.yard:\n  # .yard:"),
        "{}",
        stdout(&out)
    );
    assert_eq!(d.params("sites.init")[0]["local"], true);
}

#[test]
fn init_names_the_backup_a_replace_kept() {
    let mut result = init_result(project_site(json!([])));
    result["backup"] = json!("/r/sites/blog/.yard-backup");
    let d = MockDaemon::start(&[("sites.init", Ok(result))]);
    let out = d.yard().args(["init", "--replace"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("Kept the old file as /r/sites/blog/.yard-backup.\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn link_names_the_managed_file_it_uses_or_ignores() {
    let mut site = serde_json::to_value(sites().remove(0)).unwrap();
    site["project_file"] = json!("/r/sites/blog/.yard");
    site["project_file_kind"] = json!("managed");
    let d = MockDaemon::start(&[("sites.link", Ok(site))]);
    let out = d.yard().args(["link", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).ends_with("Using /r/sites/blog/.yard\n"), "{}", stdout(&out));

    let mut site = serde_json::to_value(sites().remove(0)).unwrap();
    site["ignored_managed_file"] =
        json!({"file": "/r/sites/blog/.yard", "because": "other_project", "project": "/code/old-blog"});
    let d = MockDaemon::start(&[("sites.link", Ok(site))]);
    let out = d.yard().args(["link", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).ends_with("/r/sites/blog/.yard is for /code/old-blog, so it is not used here.\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn commands_that_edit_a_managed_file_name_it() {
    let mut site = serde_json::to_value(sites().remove(0)).unwrap();
    site["project_file"] = json!("/r/sites/blog/.yard");
    site["project_file_kind"] = json!("managed");
    let mut secured = site.clone();
    secured["secure"] = json!(true);
    secured["url"] = json!("https://blog.yard:8443/");
    let d = MockDaemon::start(&[
        ("sites.isolate", Ok(site.clone())),
        ("sites.docroot", Ok(site.clone())),
        ("sites.unsecure", Ok(site)),
        ("sites.secure", Ok(secured)),
        ("ca.info", Ok(ca_info(true))),
    ]);
    let run = |args: &[&str]| {
        let out = d.yard().args(args).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        stdout(&out)
    };
    let set_in = " (set in /r/sites/blog/.yard)\n";
    assert_eq!(run(&["isolate", "8.4", "--site", "blog"]), format!("blog now runs PHP 8.4{set_in}"));
    assert_eq!(
        run(&["docroot", "public", "--site", "blog"]),
        format!("blog now serves /Users/me/code/blog/public{set_in}")
    );
    assert_eq!(run(&["unsecure", "blog"]), format!("blog is back on http://blog.yard:8080/{set_in}"));
    assert_eq!(run(&["secure", "blog"]), format!("Secured blog → https://blog.yard:8443/{set_in}"));
}

#[test]
fn php_list_available_refuses_a_yardd_that_would_ignore_it() {
    let old = json!({"protocol": "2.10", "daemon_version": "0.1.0", "commit": "old"});
    let d = MockDaemon::start(&[("hello", Ok(old)), ("php.list", Ok(json!([])))]);
    let out = d.yard().args(["php", "list", "--available"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("yardd speaks protocol 2.10, which lists only installed PHP versions"), "{err}");
    assert!(err.contains("hint: yard stop && yard start"), "{err}");
    assert!(d.params("php.list").is_empty(), "nothing is sent to the old yardd");
}

#[test]
fn init_refuses_a_yardd_that_would_write_into_the_project() {
    let old = json!({"protocol": "2.5", "daemon_version": "0.1.0", "commit": "old"});
    let d = MockDaemon::start(&[("hello", Ok(old.clone()))]);
    let out = d.yard().args(["init", "blog"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("protocol 2.5"), "{err}");
    assert!(err.contains("hint: yard stop && yard start"), "{err}");
    assert!(d.params("sites.init").is_empty(), "nothing is sent to the old yardd");

    // --local asks for what the old yardd does anyway.
    let d =
        MockDaemon::start(&[("hello", Ok(old)), ("sites.init", Ok(init_result(project_site(json!([])))))]);
    let out = d.yard().args(["init", "blog", "--local"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_method_an_older_yardd_lacks_names_the_restart() {
    let old = json!({"protocol": "2.6", "daemon_version": "0.1.0", "commit": "old"});
    let d = MockDaemon::start(&[("hello", Ok(old))]);
    let out = d.yard().args(["node", "list"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("yardd speaks protocol 2.6 and has no node.list"), "{err}");
    assert!(err.contains("hint: yard stop && yard start"), "{err}");
}

#[test]
fn init_without_terminal_or_yes_is_needs_install() {
    let mut result = init_result(project_site(json!([{"kind": "php", "php": "7.3"}])));
    result["written"] = json!(false);
    let d = MockDaemon::start(&[("sites.init", Ok(result))]);
    let out = d.yard().args(["init", "--force", "blog"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let (so, err) = (stdout(&out), stderr(&out));
    assert!(so.starts_with("/Users/me/code/blog/.yard is already there:\n"), "{so}");
    assert!(so.contains("  PHP 7.3  (yard php install 7.3)\n"), "{so}");
    assert!(err.contains("error: blog needs PHP 7.3"), "{err}");
    assert!(err.contains("  hint: yard init --yes"), "{err}");
    assert!(d.params("php.install").is_empty(), "nothing is installed without asking");
    assert_eq!(d.params("sites.init")[0]["force"], true);
    assert_eq!(d.params("sites.init")[0]["name"], "blog");
}

#[test]
fn init_yes_installs_php_then_remaining_extensions() {
    let needs = json!([
        {"kind": "php", "php": "7.3"},
        {"kind": "extension", "name": "redis", "php": "7.3"},
        {"kind": "extension", "name": "mbstring", "php": "7.3"},
    ]);
    let after_php = project_site(json!([{"kind": "extension", "name": "redis", "php": "7.3"}]));
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("sites.init", vec![Ok(init_result(project_site(needs)))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("php.install", vec![Ok(json!({"job_id": "php-install-1"}))]),
            ("ext.add", vec![Ok(json!({"job_id": "ext-add-1"}))]),
            ("sites.list", vec![Ok(json!([after_php])), Ok(json!([project_site(json!([]))]))]),
        ],
        &[
            ("php.install", job_events("php-install-1", json!(null))),
            ("ext.add", job_events("ext-add-1", json!(null))),
        ],
    );
    let out = d.yard().args(["init", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("php.install"), [json!({"php": "7.3"})]);
    assert_eq!(d.params("ext.add"), [json!({"name": "redis", "php": "7.3"})]);
    let so = stdout(&out);
    assert!(so.ends_with("Serving http://blog.yard:8080/\n"), "{so}");
}

#[test]
fn link_yes_settles_needs() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("sites.link", vec![Ok(project_site(json!([{"kind": "php", "php": "7.3"}])))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("php.install", vec![Ok(json!({"job_id": "php-install-1"}))]),
            ("sites.list", vec![Ok(json!([project_site(json!([]))]))]),
        ],
        &[("php.install", job_events("php-install-1", json!(null)))],
    );
    let out = d.yard().args(["link", "blog", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("php.install"), [json!({"php": "7.3"})]);
    assert!(stdout(&out).ends_with("Serving http://blog.yard:8080/\n"), "{}", stdout(&out));
}

#[test]
fn init_json_needs_install_error() {
    let needs = json!([{"kind": "extension", "name": "redis", "php": "8.4"}]);
    let d = MockDaemon::start(&[("sites.init", Ok(init_result(project_site(needs.clone()))))]);
    let out = d.yard().args(["init", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["error"]["code"], "NEEDS_INSTALL");
    assert_eq!(printed["error"]["message"], "blog needs the redis extension for PHP 8.4");
    assert_eq!(printed["error"]["hint"], "yard init --yes");
    assert_eq!(printed["error"]["data"]["needs"], needs);
}

#[test]
fn sites_shows_needs_setup() {
    let mut list = serde_json::to_value(sites()).unwrap();
    list[0] = project_site(json!([{"kind": "php", "php": "7.3"}]));
    let d = MockDaemon::start(&[("sites.list", Ok(list))]);
    let out = d.yard().arg("sites").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    let header = so.lines().next().unwrap();
    assert!(header.ends_with("PHP  STATUS"), "{so}");
    assert!(so.lines().nth(1).unwrap().ends_with("needs setup"), "{so}");
    assert!(so.lines().nth(2).unwrap().ends_with("serving"), "{so}");
}

#[test]
fn link_with_an_invalid_project_file_fails() {
    let mut site = project_site(json!([]));
    let bad = json!({
        "code": "PROJECT_FILE_INVALID",
        "message": "/Users/me/code/blog/.yard line 1, column 1: unknown field `node`",
        "hint": "edit /Users/me/code/blog/.yard",
    });
    site["project_error"] = bad;
    let d = MockDaemon::start(&[("sites.link", Ok(site))]);
    let out = d.yard().args(["link", "blog"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("error: /Users/me/code/blog/.yard line 1, column 1: unknown field `node`"), "{err}");
    assert!(err.contains("  hint: edit /Users/me/code/blog/.yard"), "{err}");
    assert!(!stdout(&out).contains("→"), "no URL for a site that is not serving: {}", stdout(&out));
}

#[test]
fn init_yes_reports_what_is_still_missing() {
    let needs = json!([{"kind": "extension", "name": "opcache", "php": "8.4"}]);
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("sites.init", vec![Ok(init_result(project_site(needs.clone())))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("ext.add", vec![Ok(json!({"job_id": "ext-add-1"}))]),
            ("sites.list", vec![Ok(json!([project_site(needs)]))]),
        ],
        &[("ext.add", job_events("ext-add-1", json!(null)))],
    );
    let out = d.yard().args(["init", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!stdout(&out).contains("Serving"), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains("error: blog still needs the opcache extension for PHP 8.4"), "{err}");
    assert!(err.contains("  hint: yard doctor"), "{err}");
}

/// Yard installs what it can after the yes and leaves emailulator's installer to the user.
#[test]
fn settle_yes_leaves_manual_need() {
    let installer =
        "curl -fsSL https://raw.githubusercontent.com/webteractive/emailulator/main/install.sh | sh";
    let manual = json!({"kind": "manual", "what": "emailulator", "command": installer});
    let d = MockDaemon::start_sequenced_with_events(
        &[
            (
                "sites.init",
                vec![Ok(init_result(project_site(json!([{"kind": "php", "php": "7.3"}, manual]))))],
            ),
            ("subscribe", vec![Ok(json!(null))]),
            ("php.install", vec![Ok(json!({"job_id": "php-install-1"}))]),
            ("sites.list", vec![Ok(json!([project_site(json!([manual]))]))]),
        ],
        &[("php.install", job_events("php-install-1", json!(null)))],
    );
    let out = d.yard().args(["init", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(d.params("php.install"), [json!({"php": "7.3"})]);
    let (so, err) = (stdout(&out), stderr(&out));
    assert!(so.contains(&format!("  emailulator  ({installer})\n")), "the list names it: {so}");
    assert!(so.contains(&format!("Still needed: emailulator  ({installer})\n")), "{so}");
    assert!(!so.contains("Serving"), "{so}");
    assert!(err.contains("error: blog still needs emailulator"), "{err}");
    assert!(err.contains(&format!("  hint: {installer}")), "{err}");
}

#[test]
fn init_says_where_it_moved_the_site_from() {
    let mut result = init_result(project_site(json!([])));
    result["moved_from"] = json!("/Users/me/code/blog/site");
    let d = MockDaemon::start(&[("sites.init", Ok(result))]);
    let out = d.yard().arg("init").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.starts_with("Moved blog here from /Users/me/code/blog/site.\nWrote "), "{so}");
}

#[test]
fn init_name_taken_without_terminal_is_the_error() {
    let taken = YardError::new(ErrorCode::SiteNameTaken, "blog already serves /Users/me/old/blog")
        .with_hint("yard init --replace")
        .with_data(json!({"name": "blog", "serves": "/Users/me/old/blog"}));
    let d = MockDaemon::start(&[("sites.init", Err(taken))]);
    let out = d.yard().arg("init").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("error: blog already serves /Users/me/old/blog"), "{err}");
    assert!(err.contains("  hint: yard init --replace"), "{err}");
    assert_eq!(d.params("sites.init").len(), 1, "no retry without a yes");
    assert_eq!(d.params("sites.init")[0]["replace"], false);
}

#[test]
fn init_replace_is_sent() {
    let d = MockDaemon::start(&[("sites.init", Ok(init_result(project_site(json!([])))))]);
    let out = d.yard().args(["init", "--replace"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("sites.init")[0]["replace"], true);
}

// ---- M4b: services, env, info and sources ----

fn account(user: Option<&str>, password: Option<&str>) -> Value {
    json!({"user": user, "password": password})
}

fn service(name: &str, engine: &str, version: Option<&str>, port: u16, state: Value, who: Value) -> Value {
    json!({
        "name": name, "engine": engine, "version": version, "formula": format!("{engine}@x"),
        "ports": [{"name": name, "port": port}], "state": state, "stopped_by_user": state == json!("stopped"),
        "account": who, "console": null, "used_by": [],
    })
}

fn mysql(state: Value) -> Value {
    service("mysql-8.4", "mysql", Some("8.4"), 3307, state, account(Some("root"), Some("")))
}

fn services_list() -> Vec<Value> {
    let mut redis = service("redis", "redis", None, 6380, json!("running"), account(None, None));
    redis["used_by"] = json!(["blog", "shop"]);
    let mut mine = mysql(json!("running"));
    mine["used_by"] = json!(["blog"]);
    let mut minio = service(
        "minio",
        "minio",
        None,
        9000,
        json!({"failed": {"last_lines": ["boom"]}}),
        account(Some("minioadmin"), Some("minioadmin")),
    );
    minio["console"] = json!("http://127.0.0.1:9001");
    vec![
        mine,
        service(
            "postgres-17",
            "postgres",
            Some("17"),
            5433,
            json!("stopped"),
            account(Some("dev"), Some("")),
        ),
        redis,
        minio,
    ]
}

#[test]
fn services_table_snapshot() {
    let d = MockDaemon::start(&[("services.list", Ok(json!(services_list())))]);
    let out = d.yard().arg("services").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn services_json_is_proto_shape() {
    let d = MockDaemon::start(&[("services.list", Ok(json!(services_list())))]);
    let out = d.yard().args(["services", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let printed: Vec<yard_proto::ServiceInfo> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed.len(), 4);
    assert_eq!(printed[0].name, "mysql-8.4");
}

#[test]
fn empty_services_says_what_to_do() {
    let d = MockDaemon::start(&[("services.list", Ok(json!([])))]);
    let out = d.yard().arg("services").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.starts_with("No services yet. Add one with yard service add <engine>"), "{so}");
    assert!(so.contains("mysql, postgres, redis, minio or emailulator"), "{so}");
}

fn mysql_env() -> Value {
    json!([{
        "instance": "mysql-8.4",
        "lines": ["DB_CONNECTION=mysql", "DB_HOST=127.0.0.1", "DB_PORT=3307", "DB_USERNAME=root", "DB_PASSWORD="],
        "note": null,
    }])
}

#[test]
fn service_add_runs_the_job_then_prints_the_env_lines() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("services.list", vec![Ok(json!([])), Ok(json!([mysql(json!("running"))]))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("services.add", vec![Ok(json!({"job_id": "service-add-1"}))]),
            ("env.print", vec![Ok(mysql_env())]),
        ],
        &[("services.add", job_events("service-add-1", json!(null)))],
    );
    let out =
        d.yard().args(["service", "add", "mysql", "--version", "8.4", "--port", "3307"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.contains("==> Downloading php@7.1\n==> Pouring php@7.1\n"), "job lines: {so}");
    assert!(so.contains("Added mysql-8.4."), "{so}");
    assert!(so.contains("# mysql-8.4\nDB_CONNECTION=mysql\n"), "{so}");
    assert!(so.contains("DB_PORT=3307\n"), "{so}");
    assert_eq!(d.params("services.add"), [json!({"engine": "mysql", "version": "8.4", "port": 3307})]);
    assert_eq!(d.params("env.print"), [json!({"service": "mysql-8.4"})]);
}

#[test]
fn service_add_without_a_version_sends_none() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("services.list", vec![Ok(json!([])), Ok(json!([mysql(json!("running"))]))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("services.add", vec![Ok(json!({"job_id": "service-add-1"}))]),
            ("env.print", vec![Ok(mysql_env())]),
        ],
        &[("services.add", job_events("service-add-1", json!(null)))],
    );
    let out = d.yard().args(["service", "add", "mysql"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("services.add"), [json!({"engine": "mysql"})]);
    assert_eq!(d.params("env.print"), [json!({"service": "mysql-8.4"})], "the instance the add created");
}

#[test]
fn service_add_json_prints_only_the_job_events() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("services.list", vec![Ok(json!([])), Ok(json!([mysql(json!("running"))]))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("services.add", vec![Ok(json!({"job_id": "service-add-1"}))]),
        ],
        &[("services.add", job_events("service-add-1", json!(null)))],
    );
    let out = d.yard().args(["service", "add", "mysql", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let events: Vec<Value> = stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(events.last().unwrap()["type"], "job_finished");
}

fn untrusted(formula: &str, tap: &str) -> YardError {
    YardError::new(
        ErrorCode::FormulaUntrusted,
        format!("{formula} comes from the {tap} tap, which brew does not trust yet"),
    )
    .with_hint(format!("yard source trust {formula}"))
    .with_data(json!({"formula": formula, "tap": tap}))
}

#[test]
fn untrusted_install_without_tty_fails_with_hint() {
    let formula = "shivammathur/php/php@8.3";
    let d = MockDaemon::start(&[
        ("subscribe", Ok(json!(null))),
        ("php.install", Err(untrusted(formula, "shivammathur/php"))),
        ("sources.trust", Ok(json!({"job_id": "source-trust-1"}))),
    ]);
    let out = d.yard().args(["php", "install", "8.3"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("comes from the shivammathur/php tap"), "{err}");
    assert!(err.contains("  hint: yard source trust shivammathur/php/php@8.3"), "{err}");
    assert!(d.params("sources.trust").is_empty(), "no terminal, no trust");
    assert_eq!(d.params("php.install").len(), 1, "no retry");
}

#[test]
fn untrusted_service_and_extension_installs_fail_the_same_way() {
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([]))),
        ("subscribe", Ok(json!(null))),
        ("services.add", Err(untrusted("minio/stable/minio", "minio/stable"))),
        ("ext.add", Err(untrusted("shivammathur/extensions/xdebug@8.4", "shivammathur/extensions"))),
    ]);
    let out = d.yard().args(["service", "add", "minio"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("  hint: yard source trust minio/stable/minio"), "{}", stderr(&out));
    let out = d.yard().args(["ext", "add", "xdebug"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("yard source trust shivammathur/extensions/xdebug@8.4"),
        "{}",
        stderr(&out)
    );
    let json_out = d.yard().args(["php", "install", "8.3", "--json"]).output().unwrap();
    assert_eq!(json_out.status.code(), Some(1));
    assert!(d.params("sources.trust").is_empty());
}

#[test]
fn yes_does_not_grant_trust() {
    let needs = json!([{"kind": "php", "php": "8.3"}]);
    let d = MockDaemon::start(&[
        ("sites.init", Ok(init_result(project_site(needs)))),
        ("subscribe", Ok(json!(null))),
        ("php.install", Err(untrusted("shivammathur/php/php@8.3", "shivammathur/php"))),
        ("sources.trust", Ok(json!({"job_id": "source-trust-1"}))),
    ]);
    let out = d.yard().args(["init", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("comes from the shivammathur/php tap"), "{err}");
    assert!(err.contains("  hint: yard source trust shivammathur/php/php@8.3"), "{err}");
    assert!(d.params("sources.trust").is_empty());
    assert_eq!(d.params("php.install").len(), 1);
}

#[test]
fn init_yes_adds_missing_services() {
    let needs = json!([
        {"kind": "php", "php": "7.3"},
        {"kind": "extension", "name": "redis", "php": "7.3"},
        {"kind": "service", "engine": "mysql", "version": "8.4"},
        {"kind": "service", "engine": "redis"},
    ]);
    let services_needed = json!([
        {"kind": "service", "engine": "mysql", "version": "8.4"},
        {"kind": "service", "engine": "redis"},
    ]);
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("sites.init", vec![Ok(init_result(project_site(needs)))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("php.install", vec![Ok(json!({"job_id": "php-install-1"}))]),
            ("ext.add", vec![Ok(json!({"job_id": "ext-add-1"}))]),
            ("services.add", vec![Ok(json!({"job_id": "service-add-1"}))]),
            (
                "services.list",
                vec![
                    Ok(json!([])),
                    Ok(json!([mysql(json!("running"))])),
                    Ok(json!([mysql(json!("running"))])),
                    Ok(json!([
                        mysql(json!("running")),
                        service("redis", "redis", None, 6380, json!("running"), account(None, None))
                    ])),
                ],
            ),
            ("env.print", vec![Ok(mysql_env())]),
            (
                "sites.list",
                vec![
                    Ok(json!([project_site(json!([
                        {"kind": "extension", "name": "redis", "php": "7.3"},
                    ]))])),
                    Ok(json!([project_site(services_needed)])),
                    Ok(json!([project_site(json!([]))])),
                ],
            ),
        ],
        &[
            ("php.install", job_events("php-install-1", json!(null))),
            ("ext.add", job_events("ext-add-1", json!(null))),
            ("services.add", job_events("service-add-1", json!(null))),
        ],
    );
    let out = d.yard().args(["init", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("php.install"), [json!({"php": "7.3"})]);
    assert_eq!(d.params("ext.add"), [json!({"name": "redis", "php": "7.3"})]);
    assert_eq!(
        d.params("services.add"),
        [json!({"engine": "mysql", "version": "8.4"}), json!({"engine": "redis"})],
        "each service need is added after PHP and the extensions"
    );
    assert!(stdout(&out).ends_with("Serving http://blog.yard:8080/\n"), "{}", stdout(&out));
}

#[test]
fn service_remove_needs_confirmation_without_tty() {
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([mysql(json!("running"))]))),
        ("services.remove", Ok(json!(null))),
    ]);
    let out = d.yard().args(["service", "remove", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("error: "), "{err}");
    assert!(err.contains("  hint: yard service remove mysql-8.4 --yes\n"), "{err}");
    assert!(d.params("services.remove").is_empty(), "nothing is deleted without a yes");

    let out = d.yard().args(["service", "remove", "mysql-8.4", "--keep-data", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["error"]["code"], "NEEDS_CONFIRMATION");
    assert_eq!(printed["error"]["hint"], "yard service remove mysql-8.4 --keep-data --yes");
    assert!(d.params("services.remove").is_empty());
}

#[test]
fn service_remove_yes_calls_remove() {
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([mysql(json!("running"))]))),
        ("services.remove", Ok(json!(null))),
    ]);
    let out = d.yard().args(["service", "remove", "mysql-8.4", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Removed mysql-8.4.\n");
    assert_eq!(d.params("services.remove"), [json!({"name": "mysql-8.4", "keep_data": false})]);

    let out = d.yard().args(["service", "remove", "mysql-8.4", "--keep-data", "--yes"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.starts_with("Removed mysql-8.4. Its data stays in "), "{so}");
    assert!(so.trim_end().ends_with("services/mysql-8.4/data."), "{so}");
    assert_eq!(d.params("services.remove")[1], json!({"name": "mysql-8.4", "keep_data": true}));
}

#[test]
fn service_remove_in_use_refuses_without_asking() {
    let mut used = mysql(json!("running"));
    used["used_by"] = json!(["blog", "shop"]);
    let blog = project_site(json!([]));
    let mut shop = project_site(json!([]));
    shop["name"] = json!("shop");
    shop["project_file"] = json!("/Users/me/code/shop/.yard");
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([used]))),
        ("sites.list", Ok(json!([blog, shop]))),
        ("services.remove", Ok(json!(null))),
    ]);
    // Without a terminal and without --yes it is still the in-use refusal, not a request for a yes.
    for extra in [vec![], vec!["--yes"]] {
        let mut args = vec!["service", "remove", "mysql-8.4"];
        args.extend(extra);
        let out = d.yard().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let err = stderr(&out);
        assert!(err.contains("error: mysql-8.4 is used by blog, shop, from its .yard"), "{err}");
        assert!(err.contains("  hint: edit /Users/me/code/blog/.yard\n"), "{err}");
        assert_eq!(stdout(&out), "", "no question was printed");
    }
    assert!(d.params("services.remove").is_empty(), "the delete is never sent");
    let out = d.yard().args(["service", "remove", "mysql-8.4", "--json"]).output().unwrap();
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["error"]["code"], "SERVICE_IN_USE");
    assert_eq!(printed["error"]["data"]["sites"], json!(["blog", "shop"]));
    assert_eq!(
        printed["error"]["data"]["files"],
        json!(["/Users/me/code/blog/.yard", "/Users/me/code/shop/.yard"])
    );
}

#[test]
fn service_remove_unknown_instance_is_not_found_before_asking() {
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([mysql(json!("running"))]))),
        ("services.remove", Ok(json!(null))),
    ]);
    let out = d.yard().args(["service", "remove", "redis"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("error: no service instance named \"redis\""), "{err}");
    assert!(err.contains("  hint: yard services lists the instances"), "{err}");
    assert!(d.params("services.remove").is_empty());
}

#[test]
fn service_remove_refused_by_the_daemon_shows_its_error() {
    let in_use = YardError::new(ErrorCode::ServiceInUse, "mysql-8.4 is used by blog, from its .yard")
        .with_hint("edit /Users/me/code/blog/.yard");
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([mysql(json!("running"))]))),
        ("services.remove", Err(in_use)),
    ]);
    let out = d.yard().args(["service", "remove", "mysql-8.4", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("hint: edit /Users/me/code/blog/.yard"), "{}", stderr(&out));
}

#[test]
fn service_start_waits_until_it_is_running() {
    let d = MockDaemon::start_sequenced(&[
        ("services.start", vec![Ok(mysql(json!("starting")))]),
        ("services.list", vec![Ok(json!([mysql(json!("starting"))])), Ok(json!([mysql(json!("running"))]))]),
    ]);
    let out = d.yard().args(["service", "start", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Started mysql-8.4 on 127.0.0.1:3307.\n");
    assert_eq!(d.params("services.start"), [json!({"name": "mysql-8.4"})]);
    assert_eq!(d.params("services.list").len(), 2, "polled until running");
}

#[test]
fn service_restart_of_a_failed_instance_polls_to_running() {
    // yardd answers once the restart is under way, so the state is the new attempt's.
    let d = MockDaemon::start_sequenced(&[
        ("services.restart", vec![Ok(mysql(json!("starting")))]),
        ("services.list", vec![Ok(json!([mysql(json!("starting"))])), Ok(json!([mysql(json!("running"))]))]),
    ]);
    let out = d.yard().args(["service", "restart", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Restarted mysql-8.4 on 127.0.0.1:3307.\n");
    assert_eq!(d.params("services.list").len(), 2, "polled until running");
}

fn held_by_dbngin() -> Value {
    json!({"failed": {"last_lines": [], "reason": {
        "code": "PORT_IN_USE",
        "message": "port 3307 is held by mysqld (pid 77034, DBngin)",
        "hint": "yard service port mysql-8.4 <port>",
        "data": {"port": 3307},
    }}})
}

#[test]
fn service_start_on_a_held_port_names_the_holder() {
    let d = MockDaemon::start(&[("services.start", Ok(mysql(held_by_dbngin())))]);
    let out = d.yard().args(["service", "start", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("error: mysql-8.4 failed to start: port 3307 is held by mysqld (pid 77034, DBngin)\n"),
        "{err}"
    );
    assert!(err.contains("  hint: yard service port mysql-8.4 <port>\n"), "{err}");
    assert!(!err.contains("no output"), "{err}");
    assert!(!stdout(&out).contains("Started"), "{}", stdout(&out));

    let out = d.yard().args(["service", "start", "mysql-8.4", "--json"]).output().unwrap();
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["error"]["code"], "PORT_IN_USE", "{printed}");
    assert_eq!(printed["error"]["hint"], "yard service port mysql-8.4 <port>", "{printed}");
}

#[test]
fn a_failure_reason_comes_before_the_log() {
    let failed = json!({"failed": {"last_lines": ["InnoDB: upgrading the data dictionary"], "reason": {
        "code": "JOB_FAILED",
        "message": "not healthy within 120 s of starting",
        "hint": "yard service logs mysql-8.4",
    }}});
    let d = MockDaemon::start(&[("services.start", Ok(mysql(failed)))]);
    let out = d.yard().args(["service", "start", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("  InnoDB: upgrading the data dictionary\n"), "{err}");
    assert!(
        err.contains("error: mysql-8.4 failed to start: not healthy within 120 s of starting; its log ends: InnoDB: upgrading the data dictionary\n"),
        "{err}"
    );
}

#[test]
fn services_and_info_name_why_an_instance_failed() {
    let d = MockDaemon::start(&[("services.list", Ok(json!([mysql(held_by_dbngin())])))]);
    let out = d.yard().arg("services").output().unwrap();
    let so = stdout(&out);
    assert!(
        so.contains("mysql-8.4 failed: port 3307 is held by mysqld (pid 77034, DBngin)  (yard service port mysql-8.4 <port>)\n"),
        "{so}"
    );

    let mut detail = site_detail();
    detail["services"][0]["service"]["state"] = held_by_dbngin();
    let d = MockDaemon::start(&[("sites.info", Ok(detail)), ("ext.list", Ok(ext_rows()))]);
    let out = d.yard().arg("info").env("HOME", "/Users/me").output().unwrap();
    let so = stdout(&out);
    assert!(
        so.contains("  mysql-8.4 failed: port 3307 is held by mysqld (pid 77034, DBngin)  (yard service port mysql-8.4 <port>)\n"),
        "{so}"
    );
}

#[test]
fn service_start_never_says_started_unless_it_runs() {
    // An instance yardd runs no unit for answers `stopped` though the user never stopped it.
    let mut left_out = mysql(json!("stopped"));
    left_out["stopped_by_user"] = json!(false);
    let d = MockDaemon::start(&[("services.start", Ok(left_out))]);
    let out = d.yard().args(["service", "start", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(!stdout(&out).contains("Started"), "{}", stdout(&out));
    assert!(stderr(&out).contains("error: mysql-8.4 did not start"), "{}", stderr(&out));
    // Even a stopped answer that says the user stopped it is not a start.
    let d = MockDaemon::start(&[("services.start", Ok(mysql(json!("stopped"))))]);
    let out = d.yard().args(["service", "start", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(!stdout(&out).contains("Started"), "{}", stdout(&out));

    let missing = json!({"failed": {"last_lines": [], "reason": {
        "code": "FORMULA_MISSING",
        "message": "/opt/homebrew/opt/mysql@8.4 is missing",
        "hint": "brew install mysql@8.4",
    }}});
    let d = MockDaemon::start(&[("services.restart", Ok(mysql(missing)))]);
    let out = d.yard().args(["service", "restart", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("error: mysql-8.4 failed to start: /opt/homebrew/opt/mysql@8.4 is missing\n"),
        "{err}"
    );
    assert!(err.contains("  hint: brew install mysql@8.4\n"), "{err}");
    assert!(!stdout(&out).contains("Restarted"), "{}", stdout(&out));
}

#[test]
fn service_start_that_fails_shows_the_last_lines_and_exits_1() {
    let failed = json!({"failed": {"last_lines": ["[ERROR] Can't start server", "[ERROR] Aborting"]}});
    let d = MockDaemon::start_sequenced(&[
        ("services.start", vec![Ok(mysql(json!("starting")))]),
        ("services.list", vec![Ok(json!([mysql(failed)]))]),
    ]);
    let out = d.yard().args(["service", "start", "mysql-8.4"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("  [ERROR] Aborting\n"), "last lines are shown: {err}");
    assert!(err.contains("error: mysql-8.4 failed to start: [ERROR] Aborting"), "{err}");
    assert!(err.contains("  hint: yard service logs mysql-8.4"), "{err}");
    assert!(!stdout(&out).contains("Started"), "{}", stdout(&out));
}

#[test]
fn service_stop_restart_and_port_send_their_params() {
    let d = MockDaemon::start(&[
        ("services.stop", Ok(mysql(json!("stopped")))),
        ("services.restart", Ok(mysql(json!("running")))),
        (
            "services.port",
            Ok(service(
                "mysql-8.4",
                "mysql",
                Some("8.4"),
                3310,
                json!("running"),
                account(Some("root"), Some("")),
            )),
        ),
    ]);
    let out = d.yard().args(["service", "stop", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Stopped mysql-8.4.\n");
    let out = d.yard().args(["service", "restart", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Restarted mysql-8.4 on 127.0.0.1:3307.\n");
    let out = d.yard().args(["service", "port", "mysql-8.4", "3310"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "mysql-8.4 now listens on 127.0.0.1:3310.\n");
    assert_eq!(d.params("services.stop"), [json!({"name": "mysql-8.4"})]);
    assert_eq!(d.params("services.restart"), [json!({"name": "mysql-8.4"})]);
    assert_eq!(d.params("services.port"), [json!({"name": "mysql-8.4", "port": 3310})]);
    let out = d.yard().args(["service", "stop", "mysql-8.4", "--json"]).output().unwrap();
    let info: yard_proto::ServiceInfo = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(info.name, "mysql-8.4");
}

#[test]
fn service_logs_prints_the_tail() {
    let d = MockDaemon::start(&[("services.logs", Ok(json!(["one", "two"])))]);
    let out = d.yard().args(["service", "logs", "redis", "-n", "50"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "one\ntwo\n");
    assert_eq!(d.params("services.logs"), [json!({"name": "redis", "lines": 50})]);

    let out = d.yard().args(["service", "logs", "redis", "--json"]).output().unwrap();
    assert_eq!(serde_json::from_slice::<Vec<String>>(&out.stdout).unwrap(), ["one", "two"]);
    assert_eq!(d.params("services.logs")[1], json!({"name": "redis"}));
}

#[test]
fn service_logs_follow_prints_the_tail_then_the_units_new_lines() {
    use std::io::{BufRead, BufReader};
    let d = MockDaemon::start_with_events(
        &[("subscribe", Ok(json!(null))), ("services.logs", Ok(json!(["old one", "old two"])))],
        &[(
            "services.logs",
            vec![
                json!({"type": "log_line", "unit": "mysql-8.4", "line": "not this unit"}),
                json!({"type": "log_line", "unit": "redis", "line": "a new line"}),
            ],
        )],
    );
    let mut child = d
        .yard_process()
        .args(["service", "logs", "redis", "-f"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as libc::pid_t;
    // A hung follow must fail the test rather than stall the suite.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(20));
        // SAFETY: signalling our own child process.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut seen = vec![];
    while seen.last().is_none_or(|l: &String| l != "a new line") {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            break;
        }
        seen.push(line.trim_end().to_string());
    }
    // SAFETY: signalling our own child process.
    unsafe { libc::kill(pid, libc::SIGINT) };
    let status = child.wait().unwrap();
    assert_eq!(seen, ["old one", "old two", "a new line"]);
    assert_eq!(status.code(), Some(0), "Ctrl-C ends a follow cleanly");
    assert_eq!(d.params("subscribe"), [json!({"topics": ["logs"]})]);
}

fn two_blocks() -> Value {
    json!([
        {"instance": "mysql-8.4", "lines": ["DB_CONNECTION=mysql", "DB_HOST=127.0.0.1", "DB_PORT=3307", "DB_USERNAME=root", "DB_PASSWORD="], "note": null},
        {"instance": "postgres-17", "lines": ["DB_CONNECTION=pgsql", "DB_HOST=127.0.0.1", "DB_PORT=5433", "DB_USERNAME=dev", "DB_PASSWORD="], "note": "both set DB_*; the second block wins in a .env"},
        {"instance": "redis-7", "lines": [], "note": "redis-7 is not added: yard service add redis"},
        {"instance": "redis", "lines": ["REDIS_HOST=127.0.0.1", "REDIS_PORT=6380", "REDIS_PASSWORD=null"], "note": null},
    ])
}

#[test]
fn env_snapshot() {
    let d = MockDaemon::start(&[("env.print", Ok(two_blocks()))]);
    let out = d.yard().arg("env").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn env_json_is_proto_shape() {
    let d = MockDaemon::start(&[("env.print", Ok(two_blocks()))]);
    let out = d.yard().args(["env", "--json"]).output().unwrap();
    let blocks: Vec<yard_proto::EnvBlock> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(blocks.len(), 4);
}

#[test]
fn env_sends_the_canonical_working_directory() {
    let d = MockDaemon::start(&[("env.print", Ok(mysql_env()))]);
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().arg("env").current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let params = d.params("env.print");
    let sent = Path::new(params[0]["path"].as_str().unwrap()).to_path_buf();
    assert!(sent.is_absolute());
    assert_eq!(sent, dir.path().canonicalize().unwrap(), "symlinks resolved, as /var is /private/var");
    assert!(params[0].get("site").is_none() && params[0].get("service").is_none());
}

#[test]
fn env_site_and_service_flags_are_sent() {
    let d = MockDaemon::start(&[("env.print", Ok(mysql_env()))]);
    let out = d.yard().args(["env", "--site", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = d.yard().args(["env", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("env.print")[0], json!({"site": "blog"}));
    assert_eq!(d.params("env.print")[1], json!({"service": "mysql-8.4"}));
}

#[test]
fn env_for_a_project_with_no_services_says_so_on_stderr() {
    let detail = json!({
        "site": project_site(json!([])), "php_full": "8.4.3",
        "sources": {"php": "project_file", "docroot": "default", "secure": "default"}, "services": [],
    });
    let d = MockDaemon::start(&[("env.print", Ok(json!([]))), ("sites.info", Ok(detail))]);
    let out = d.yard().arg("env").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(err.contains("blog's .yard lists no services"), "{err}");
    assert!(err.contains("yard env --service <instance>"), "{err}");
    assert_eq!(err.trim_end().lines().count(), 1, "one line: {err}");
    assert_eq!(d.params("sites.info").len(), 1);
    assert!(d.params("sites.info")[0]["path"].is_string());
}

#[test]
fn env_with_no_blocks_outside_a_project_says_there_are_no_instances() {
    let outside = YardError::new(ErrorCode::SiteNotFound, "x is not inside a linked project")
        .with_hint("yard info <site> or yard link");
    let d = MockDaemon::start(&[("env.print", Ok(json!([]))), ("sites.info", Err(outside))]);
    let out = d.yard().arg("env").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("No service instances yet. Add one with yard service add <engine>"),
        "{}",
        stderr(&out)
    );
    let out = d.yard().args(["env", "--json"]).output().unwrap();
    assert_eq!(stdout(&out).trim(), "[]");
    assert_eq!(stderr(&out), "");
}

fn site_detail() -> Value {
    let mut site = project_site(json!([
        {"kind": "service", "engine": "postgres", "version": "17"},
    ]));
    site["php"] = json!("7.3");
    site["kind"] = json!("php");
    site["path"] = json!("/Users/me/code/blog");
    site["docroot"] = json!("/Users/me/code/blog/public");
    let mut mine = mysql(json!("running"));
    mine["used_by"] = json!(["blog"]);
    let mut minio = service(
        "minio",
        "minio",
        None,
        9000,
        json!("running"),
        account(Some("minioadmin"), Some("minioadmin")),
    );
    minio["console"] = json!("http://127.0.0.1:9001");
    json!({
        "site": site,
        "php_full": "7.3.33",
        "sources": {"php": "project_file", "docroot": "project_file", "secure": "default"},
        "services": [
            {"engine": "mysql", "want": "8.4", "service": mine},
            {"engine": "postgres", "want": "17", "service": null},
            {"engine": "redis", "want": null, "service": service("redis", "redis", None, 6380, json!("stopped"), account(None, None))},
            {"engine": "minio", "want": null, "service": minio},
        ],
    })
}

fn ext_rows() -> Value {
    json!([
        {"name": "opcache", "source": "bundled", "enabled": true},
        {"name": "redis", "source": "brew", "enabled": true},
        {"name": "imagick", "source": "pecl", "enabled": true},
        {"name": "xdebug", "source": "pecl", "enabled": false},
    ])
}

#[test]
fn info_snapshot() {
    let d = MockDaemon::start(&[("sites.info", Ok(site_detail())), ("ext.list", Ok(ext_rows()))]);
    let out = d.yard().arg("info").env("HOME", "/Users/me").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    assert_eq!(d.params("ext.list"), [json!({"php": "7.3"})]);
}

#[test]
fn info_managed_snapshot() {
    let mut detail = site_detail();
    detail["site"]["php"] = json!("7.4");
    detail["site"]["project_file"] = json!("/Users/me/Library/Application Support/Yard/sites/blog/.yard");
    detail["site"]["project_file_kind"] = json!("managed");
    detail["php_full"] = json!("7.4.33");
    detail["sources"] = json!({"php": "managed_file", "docroot": "default", "secure": "default"});
    detail["services"] = json!([]);
    detail["site"]["needs"] = json!([]);
    let d = MockDaemon::start(&[("sites.info", Ok(detail)), ("ext.list", Ok(json!([])))]);
    let out = d.yard().arg("info").env("HOME", "/Users/me").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn info_names_a_source_from_a_newer_yardd_elsewhere() {
    let mut detail = site_detail();
    detail["sources"]["docroot"] = json!("something_new");
    let d = MockDaemon::start(&[("sites.info", Ok(detail)), ("ext.list", Ok(ext_rows()))]);
    let out = d.yard().arg("info").env("HOME", "/Users/me").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("from elsewhere"), "{}", stdout(&out));
}

#[test]
fn info_for_a_proxy_shows_its_target() {
    let mut proxy = serde_json::to_value(sites().remove(1)).unwrap();
    proxy["secure"] = json!(true);
    let detail =
        json!({"site": proxy, "php_full": null, "sources": {"secure": "sites_toml"}, "services": []});
    let d = MockDaemon::start(&[("sites.info", Ok(detail))]);
    let out = d.yard().args(["info", "api"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    assert!(d.params("ext.list").is_empty(), "a proxy has no PHP");
}

#[test]
fn info_sends_the_site_or_the_canonical_directory() {
    let d = MockDaemon::start(&[("sites.info", Ok(site_detail())), ("ext.list", Ok(ext_rows()))]);
    let out = d.yard().args(["info", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().arg("info").current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let params = d.params("sites.info");
    assert_eq!(params[0], json!({"site": "blog"}));
    assert_eq!(params[1], json!({"path": dir.path().canonicalize().unwrap().display().to_string()}));
}

#[test]
fn info_json_is_the_site_detail() {
    let d = MockDaemon::start(&[("sites.info", Ok(site_detail()))]);
    let out = d.yard().args(["info", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let detail: yard_proto::SiteDetail = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(detail.php_full.as_deref(), Some("7.3.33"));
    assert!(d.params("ext.list").is_empty(), "--json prints the SiteDetail and nothing else");
}

#[test]
fn info_outside_a_project_is_the_daemons_error() {
    let outside = YardError::new(ErrorCode::SiteNotFound, "/tmp is not inside a linked project")
        .with_hint("yard info <site> or yard link");
    let d = MockDaemon::start(&[("sites.info", Err(outside))]);
    let out = d.yard().arg("info").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("hint: yard info <site> or yard link"), "{}", stderr(&out));
}

fn sources_list() -> Value {
    json!([
        {"tap": "shivammathur/php", "tapped": true, "trusted": false, "formulae": [
            {"name": "shivammathur/php/php@7.1", "trusted": true, "installed": true},
            {"name": "shivammathur/php/php@8.4", "trusted": false, "installed": true},
        ]},
        {"tap": "shivammathur/extensions", "tapped": true, "trusted": true, "formulae": [
            {"name": "shivammathur/extensions/xdebug@8.4", "trusted": true, "installed": false},
        ]},
        {"tap": "minio/stable", "tapped": false, "trusted": false, "formulae": []},
    ])
}

#[test]
fn sources_snapshot() {
    let d = MockDaemon::start(&[("sources.list", Ok(sources_list()))]);
    let out = d.yard().arg("sources").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    let out = d.yard().args(["sources", "--json"]).output().unwrap();
    let rows: Vec<yard_proto::SourceInfo> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn source_trust_and_untrust_run_their_jobs() {
    let d = MockDaemon::start_with_events(
        &[
            ("subscribe", Ok(json!(null))),
            ("sources.trust", Ok(json!({"job_id": "source-trust-1"}))),
            ("sources.untrust", Ok(json!({"job_id": "source-untrust-1"}))),
        ],
        &[
            ("sources.trust", job_events("source-trust-1", json!(null))),
            ("sources.untrust", job_events("source-untrust-1", json!(null))),
        ],
    );
    let out = d.yard().args(["source", "trust", "minio/stable/minio"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).ends_with("Trusted minio/stable/minio.\n"), "{}", stdout(&out));
    let out = d.yard().args(["source", "untrust", "shivammathur/php", "--tap"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).ends_with("Untrusted shivammathur/php.\n"), "{}", stdout(&out));
    assert_eq!(d.params("sources.trust"), [json!({"target": "minio/stable/minio", "tap": false})]);
    assert_eq!(d.params("sources.untrust"), [json!({"target": "shivammathur/php", "tap": true})]);
}

#[test]
fn doctor_sends_client_env() {
    let d = MockDaemon::start(&[("doctor", Ok(json!({"checks": []})))]);
    let out = d
        .yard()
        .arg("doctor")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOMEBREW_FORBIDDEN_FORMULAE", "php@7.1")
        .env("HOMEBREW_FORBIDDEN_TAPS", "bad/tap")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_GITHUB_API_TOKEN", "must-not-be-sent")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        d.params("doctor"),
        [
            json!({"client_env": {"HOMEBREW_FORBIDDEN_FORMULAE": "php@7.1", "HOMEBREW_FORBIDDEN_TAPS": "bad/tap"}})
        ]
    );
    let out = d.yard().arg("doctor").env("XDG_CONFIG_HOME", "/x/config").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("doctor")[1]["client_env"]["XDG_CONFIG_HOME"], "/x/config");
}

#[test]
fn the_new_commands_have_help_and_json() {
    for args in [
        vec!["services"],
        vec!["service", "add"],
        vec!["service", "remove"],
        vec!["service", "start"],
        vec!["service", "stop"],
        vec!["service", "restart"],
        vec!["service", "logs"],
        vec!["service", "port"],
        vec!["service", "sleep-after"],
        vec!["env"],
        vec!["info"],
        vec!["sources"],
        vec!["source", "trust"],
        vec!["source", "untrust"],
    ] {
        let mut full = args.clone();
        full.push("--help");
        let (_root, out) = offline(&full);
        assert!(out.status.success(), "{args:?}");
        let text = stdout(&out);
        assert!(text.lines().next().is_some_and(|l| !l.trim().is_empty()), "{args:?}: {text}");
        assert!(text.contains("--json"), "{args:?}: {text}");
    }
}

#[test]
fn env_with_no_blocks_and_a_failed_lookup_says_why() {
    let broken = YardError::new(ErrorCode::Internal, "the registry is unreadable");
    let d = MockDaemon::start(&[("env.print", Ok(json!([]))), ("sites.info", Err(broken))]);
    let out = d.yard().arg("env").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert_eq!(err.trim_end().lines().count(), 1, "{err}");
    assert!(err.contains("could not say why: the registry is unreadable"), "{err}");
}

/// A daemon that answers `hello` and then never answers anything, so a command waits on it.
fn silent_daemon() -> TempDir {
    use std::io::{BufRead, BufReader, Write};
    let root = short_root();
    let listener = std::os::unix::net::UnixListener::bind(root.path().join("yardd.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut write = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { return };
                    let request: yard_proto::rpc::Request = serde_json::from_str(&line).unwrap();
                    if request.method == "hello" {
                        let hello = json!({"protocol": yard_proto::PROTOCOL_VERSION, "daemon_version": MOCK_VERSION, "commit": "mock"});
                        let response = yard_proto::rpc::Response::ok(request.id.unwrap_or_default(), hello);
                        let _ = write.write_all(&yard_proto::rpc::encode_line(&response));
                    }
                }
            });
        }
    });
    root
}

#[test]
fn a_plain_logs_command_does_not_swallow_ctrl_c() {
    let root = silent_daemon();
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_yard"));
    // A test run started from a background job inherits an ignored SIGINT; this one needs the default.
    // SAFETY: `signal` is async-signal-safe and the closure touches nothing else.
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = cmd
        .env_remove("YARD_ROOT")
        .arg("--root")
        .arg(root.path())
        .args(["service", "logs", "redis"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id() as libc::pid_t;
    std::thread::sleep(std::time::Duration::from_millis(700));
    assert!(child.try_wait().unwrap().is_none(), "the command is waiting on the daemon");
    // SAFETY: signalling our own child process.
    unsafe { libc::kill(pid, libc::SIGINT) };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(10));
        // SAFETY: signalling our own child process.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    let status = child.wait().unwrap();
    assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGINT), "{status:?}");
}

// ---- M4e: sleeping services ----

/// RFC 3339 for `ago` before now, as yardd sends `sleeping_since`.
fn since(ago: time::Duration) -> String {
    (time::OffsetDateTime::now_utc() - ago).format(&time::format_description::well_known::Rfc3339).unwrap()
}

fn sleeping_mysql() -> Value {
    let mut mine = mysql(json!("stopped"));
    mine["stopped_by_user"] = json!(false);
    mine["sleeping_since"] = json!(since(time::Duration::hours(2)));
    mine["sleep_after_secs"] = json!(900);
    mine
}

#[test]
fn services_shows_sleeping_and_sleep_column() {
    let redis = service("redis", "redis", None, 6380, json!("running"), account(None, None));
    let d = MockDaemon::start(&[("services.list", Ok(json!([sleeping_mysql(), redis])))]);
    let out = d.yard().arg("services").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    let header = so.lines().next().unwrap();
    assert!(header.contains("STATE") && header.find("SLEEP") > header.find("STATE"), "{so}");
    let mysql_row = so.lines().find(|l| l.starts_with("mysql-8.4")).unwrap();
    assert!(mysql_row.contains("sleeping 2h") && mysql_row.contains("15m"), "{so}");
    let redis_row = so.lines().find(|l| l.starts_with("redis")).unwrap();
    assert!(redis_row.contains(" off "), "{so}");
}

#[test]
fn a_waking_instance_says_so() {
    let mut mine = sleeping_mysql();
    mine["state"] = json!("starting");
    let d = MockDaemon::start(&[("services.list", Ok(json!([mine])))]);
    let so = stdout(&d.yard().arg("services").output().unwrap());
    assert!(so.lines().any(|l| l.starts_with("mysql-8.4") && l.contains("waking")), "{so}");
}

#[test]
fn status_lists_sleeping_instances() {
    let status = json!({
        "version": {"cli": null, "daemon": MOCK_VERSION, "helper": null, "protocol": "2.8", "commit": "mock", "build_date": "2026-10-26"},
        "profile": "standard", "tld": "yard", "sites": 1, "config_error": null,
        "units": [{"name": "nginx", "state": "running", "pid": 10, "restarts": 0}],
        "sleeping": [{"name": "mysql-8.4", "since": since(time::Duration::minutes(5))}],
    });
    let d = MockDaemon::start(&[("status", Ok(status))]);
    let out = d.yard().arg("status").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    let row = so.lines().find(|l| l.starts_with("mysql-8.4")).expect("a row for the sleeping instance");
    assert_eq!(row.split_whitespace().collect::<Vec<_>>(), ["mysql-8.4", "sleeping", "-", "-"], "{so}");
}

#[test]
fn info_says_how_a_sleeping_instance_wakes() {
    let mut detail = site_detail();
    let mut mine = sleeping_mysql();
    mine["used_by"] = json!(["blog"]);
    detail["services"][0]["service"] = mine;
    let d = MockDaemon::start(&[("sites.info", Ok(detail)), ("ext.list", Ok(ext_rows()))]);
    let out = d.yard().arg("info").env("HOME", "/Users/me").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.lines().any(|l| l.contains("mysql-8.4") && l.contains("sleeping 2h")), "{so}");
    assert!(so.contains("  wakes on the next request, or: yard service start mysql-8.4\n"), "{so}");
}

#[test]
fn sleep_after_sends_and_prints() {
    let redis = |secs: Value, source: &str| {
        let mut r = service("redis", "redis", None, 6380, json!("running"), account(None, None));
        r["sleep_after_secs"] = secs;
        r["sleep_after_source"] = json!(source);
        r
    };
    let d = MockDaemon::start_sequenced(&[(
        "services.sleep.set",
        vec![
            Ok(redis(json!(3600), "services_toml")),
            Ok(redis(Value::Null, "services_toml")),
            Ok(redis(json!(900), "yard_toml")),
            Ok(redis(json!(900), "default")),
        ],
    )]);
    let run =
        |after: &str| stdout(&d.yard().args(["service", "sleep-after", "redis", after]).output().unwrap());
    assert_eq!(run("1h"), "redis now sleeps after 1h without use\n");
    assert_eq!(run("off"), "redis never sleeps\n");
    assert_eq!(run("default"), "redis now follows yard.toml (15m)\n");
    assert_eq!(run("default"), "redis now follows the default (15m)\n");
    assert_eq!(
        d.params("services.sleep.set"),
        [
            json!({"name": "redis", "after": "1h"}),
            json!({"name": "redis", "after": "off"}),
            json!({"name": "redis", "after": "default"}),
            json!({"name": "redis", "after": "default"}),
        ]
    );
}

#[test]
fn service_help_mentions_sleep() {
    for args in [vec!["services"], vec!["service", "start"], vec!["service", "sleep-after"]] {
        let mut full = args.clone();
        full.push("--help");
        let (_root, out) = offline(&full);
        let text = stdout(&out);
        assert!(
            text.contains("service_sleep_after in yard.toml, or yard service sleep-after"),
            "{args:?}: {text}"
        );
        assert!(text.contains("GUI clients cannot wake one."), "{args:?}: {text}");
    }
}

#[test]
fn port_on_a_sleeping_instance_succeeds() {
    let mut moved = sleeping_mysql();
    moved["ports"] = json!([{"name": "mysql-8.4", "port": 3310}]);
    let d = MockDaemon::start(&[("services.port", Ok(moved))]);
    let out = d.yard().args(["service", "port", "mysql-8.4", "3310"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "mysql-8.4 will listen on 127.0.0.1:3310 when it wakes.\n");
}

// ---- M4d: emailulator ----

fn emailulator_row() -> Value {
    json!({
        "name": "emailulator", "engine": "emailulator", "version": "0.1.3",
        "formula": "/Users/x/.local/bin/emailulator",
        "ports": [{"name": "emailulator-smtp", "port": 1025}, {"name": "emailulator-http", "port": 8025}],
        "state": "running", "stopped_by_user": false, "account": account(None, None),
        "console": "http://mail.yard:8080", "used_by": ["shop"],
        "managed_by": {"kind": "launch_agent", "label": "com.webteractive.emailulator", "pid": 3261},
    })
}

fn mail_env(note: Option<&str>) -> Value {
    json!([{
        "instance": "emailulator",
        "lines": ["MAIL_MAILER=smtp", "MAIL_HOST=127.0.0.1", "MAIL_PORT=1025", "MAIL_USERNAME=null",
                  "MAIL_PASSWORD=null", "MAIL_ENCRYPTION=null", "MAIL_SCHEME=null"],
        "note": note,
    }])
}

fn mail_add_daemon() -> MockDaemon {
    MockDaemon::start_sequenced_with_events(
        &[
            ("services.list", vec![Ok(json!([])), Ok(json!([emailulator_row()]))]),
            ("subscribe", vec![Ok(json!(null))]),
            ("services.add", vec![Ok(json!({"job_id": "service-add-1"}))]),
            ("env.print", vec![Ok(mail_env(None))]),
        ],
        &[("services.add", job_events("service-add-1", json!(null)))],
    )
}

#[test]
fn service_add_sends_resolved_path() {
    let bin = tempfile::tempdir().unwrap();
    let exe = bin.path().join("emailulator");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // A directory and a file that is not executable, earlier on PATH, are passed over.
    let earlier = tempfile::tempdir().unwrap();
    std::fs::create_dir(earlier.path().join("emailulator")).unwrap();
    let plain = tempfile::tempdir().unwrap();
    std::fs::write(plain.path().join("emailulator"), "").unwrap();
    let path = format!(
        "{}:{}:{}:/usr/bin:/bin",
        earlier.path().display(),
        plain.path().display(),
        bin.path().display()
    );

    let d = mail_add_daemon();
    let out = d.yard().env("PATH", &path).args(["service", "add", "emailulator"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        d.params("services.add"),
        [json!({"engine": "emailulator", "path": exe.display().to_string()})]
    );
    let so = stdout(&out);
    assert!(
        so.contains("emailulator catches mail; its web UI is http://mail.yard:8080. For your .env:"),
        "{so}"
    );
    assert!(so.contains("# emailulator\nMAIL_MAILER=smtp\n"), "{so}");
    assert_eq!(d.params("env.print"), [json!({"service": "emailulator"})]);

    // Not on PATH: yardd looks where the installer puts it.
    let d = mail_add_daemon();
    let out = d.yard().env("PATH", "/usr/bin:/bin").args(["service", "add", "emailulator"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("services.add"), [json!({"engine": "emailulator"})]);
}

#[test]
fn services_shows_launch_agent() {
    let mut list = services_list();
    list.push(emailulator_row());
    let d = MockDaemon::start(&[("services.list", Ok(json!(list)))]);
    let out = d.yard().arg("services").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(so.contains("MANAGED BY"), "{so}");
    let row = so.lines().find(|l| l.starts_with("emailulator ")).unwrap_or_else(|| panic!("{so}"));
    assert!(row.contains("LaunchAgent com.webteractive.emailulator"), "{row}");
    // Yard never puts a LaunchAgent to sleep, so it has no SLEEP setting to show.
    assert_eq!(row.split_whitespace().nth(5), Some("-"), "{row}");
    let mysql = so.lines().find(|l| l.starts_with("mysql-8.4 ")).unwrap();
    assert!(mysql.contains(" yard"), "{mysql}");
    assert!(so.contains("emailulator web UI: http://mail.yard:8080\n"), "{so}");
}

#[test]
fn service_stop_external_prints_hint() {
    let refused = YardError::new(
        ErrorCode::ServiceExternal,
        "emailulator is run by its LaunchAgent com.webteractive.emailulator; Yard does not start or stop it",
    )
    .with_hint("emailulator service status");
    let d = MockDaemon::start(&[("services.stop", Err(refused))]);
    let out = d.yard().args(["service", "stop", "emailulator"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let se = stderr(&out);
    assert!(se.contains("error: emailulator is run by its LaunchAgent com.webteractive.emailulator"), "{se}");
    assert!(se.contains("hint: emailulator service status"), "{se}");
}

#[test]
fn service_remove_emailulator_asks_nothing() {
    let refused = YardError::new(
        ErrorCode::ServiceExternal,
        "emailulator is run by its LaunchAgent com.webteractive.emailulator; Yard does not start or stop it",
    )
    .with_hint("emailulator service uninstall");
    let d = MockDaemon::start(&[
        ("services.list", Ok(json!([emailulator_row()]))),
        ("services.remove", Err(refused)),
    ]);
    let out = d.yard().args(["service", "remove", "emailulator"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let se = stderr(&out);
    assert!(se.contains("hint: emailulator service uninstall"), "{se}");
    assert!(!se.contains("NEEDS_CONFIRMATION") && !se.contains("needs a yes"), "{se}");
}

#[test]
fn service_remove_emailulator_without_agent_forgets_it() {
    let d = MockDaemon::start(&[("services.remove", Ok(json!(null)))]);
    let out = d.yard().args(["service", "remove", "emailulator"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Yard no longer records emailulator; it was not installed.\n");
}

#[test]
fn env_warning_on_stderr() {
    let d = MockDaemon::start(&[(
        "env.print",
        Ok(mail_env(Some("emailulator is not answering; see yard doctor"))),
    )]);
    let out = d.yard().args(["env", "--service", "emailulator"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "warning: emailulator is not answering; see yard doctor\n");
    let so = stdout(&out);
    assert!(so.starts_with("# emailulator\nMAIL_MAILER=smtp\n"), "{so}");
    assert!(!so.contains("not answering"), "{so}");
}

#[test]
fn service_add_help_mentions_emailulator() {
    let out = yard_at(short_root().path()).args(["service", "add", "--help"]).output().unwrap();
    assert!(out.status.success());
    let so = stdout(&out);
    assert!(so.contains("mysql, postgres, redis, minio or emailulator"), "{so}");
    assert!(so.contains("Yard never installs emailulator itself"), "{so}");
}

/// One LaunchAgent label serves one data root: `yard --root <scratch> start` must not rewrite the
/// agent that serves another root, nor `stop` boot it out (found by the M6 checklist: Yard.app's
/// Start, Stop and Restart yardd run these for the root it was opened with). A fake `launchctl`
/// first on PATH records any call, so the real agent is never touched, even by a failing run.
#[test]
fn start_and_stop_leave_another_roots_agent_alone() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let agents = home.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    // The root the agent serves exists: one that is gone is no conflict.
    std::fs::create_dir_all(home.path().join(".yard")).unwrap();
    let plist = yard_core::render::launchd::daemon_plist(Path::new("/y/yardd"), None, Path::new("/l.log"));
    let file = agents.join("dev.more.yard.daemon.plist");
    std::fs::write(&file, &plist).unwrap();
    let bin = TempDir::new().unwrap();
    let log = bin.path().join("launchctl.log");
    let fake = bin.path().join("launchctl");
    std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 0\n", log.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin.path().display());
    for verb in ["start", "stop"] {
        let out =
            yard_at(root.path()).env("HOME", home.path()).env("PATH", &path).arg(verb).output().unwrap();
        assert!(!out.status.success(), "{verb} went through: {}", stdout(&out));
        let served = home.path().join(".yard");
        assert!(stderr(&out).contains(&format!("serves {}", served.display())), "{verb}: {}", stderr(&out));
    }
    assert_eq!(std::fs::read(&file).unwrap(), plist, "the agent's plist is untouched");
    assert!(!log.exists(), "launchctl ran: {:?}", std::fs::read_to_string(&log).ok());
}

#[test]
fn self_update_refuses_a_development_build() {
    let root = short_root();
    let out = yard_at(root.path()).args(["self-update", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("development build"), "{err}");
    assert!(err.contains("hint: cargo build --release"), "{err}");
}

#[test]
fn self_update_check_reads_the_feed() {
    let root = short_root();
    let feed = tempfile::tempdir().unwrap();
    let latest = json!({"version": "9.9.9", "zip": "file:///nowhere/Yard-9.9.9-arm64.zip", "sha256": "00", "notes_url": "https://example.invalid/n"});
    std::fs::write(feed.path().join("latest.json"), latest.to_string()).unwrap();
    let out = yard_at(root.path())
        .env("YARD_RELEASE_URL", format!("file://{}", feed.path().display()))
        .args(["self-update", "--check", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["latest"], "9.9.9");
    assert_eq!(v["installed"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["available"], true);
    assert_eq!(v["kind"], "dev");
}

#[test]
fn start_without_nginx_asks_for_yes_when_not_interactive() {
    let root = short_root();
    let home = TempDir::new().unwrap();
    let brew = TempDir::new().unwrap();
    // A fake launchctl first on PATH, so a regression here can never load a real agent.
    use std::os::unix::fs::PermissionsExt;
    let fake = brew.path().join("launchctl");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", brew.path().display());
    let out = yard_at(root.path())
        .env("HOME", home.path())
        .env("PATH", path)
        .env("YARD_BREW_PREFIX", brew.path())
        .arg("start")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("nginx and dnsmasq"), "{err}");
    assert!(err.contains("hint: yard start --yes"), "{err}");
    assert!(!home.path().join("Library/LaunchAgents").exists(), "the agent was written");
}

/// A scratch root with a few files in its data, for `yard uninstall`.
fn uninstall_root() -> (TempDir, TempDir) {
    let root = short_root();
    std::fs::create_dir_all(root.path().join("data/redis")).unwrap();
    std::fs::write(root.path().join("data/redis/dump.rdb"), "x").unwrap();
    std::fs::write(root.path().join("sites.toml"), "").unwrap();
    (root, TempDir::new().unwrap())
}

#[test]
fn uninstall_refuses_without_yes_when_not_interactive() {
    let (root, home) = uninstall_root();
    let out = yard_at(root.path()).env("HOME", home.path()).arg("uninstall").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("hint: yard uninstall --yes"), "{}", stderr(&out));
    assert!(root.path().join("data/redis/dump.rdb").exists());
}

#[test]
fn uninstall_yes_on_a_scratch_root_removes_it() {
    let (root, home) = uninstall_root();
    let out = yard_at(root.path())
        .env("HOME", home.path())
        .env("YARD_SECURITY", "/usr/bin/false")
        .args(["uninstall", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(!root.path().exists(), "the scratch root is still there");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let steps: Vec<(String, String)> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["step"].as_str().unwrap().to_string(), s["outcome"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(steps, [("remove_data".to_string(), "done".to_string())]);
}

#[test]
fn uninstall_keep_data_keeps_the_root() {
    let (root, home) = uninstall_root();
    let out = yard_at(root.path())
        .env("HOME", home.path())
        .env("YARD_SECURITY", "/usr/bin/false")
        .args(["uninstall", "--yes", "--keep-data"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(root.path().join("data/redis/dump.rdb").exists());
}

/// A PATH whose `launchctl` succeeds at everything but `print` (so an unloaded agent reads as
/// gone at once), logging its arguments.
fn fake_launchctl(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let fake = dir.join("launchctl");
    let log = dir.join("launchctl.log");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n[ \"$1\" = print ] && exit 1\nexit 0\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.display())
}

#[test]
fn uninstall_removes_this_roots_agent_plist() {
    let (root, home) = uninstall_root();
    let agents = home.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    let plist = agents.join("dev.more.yard.daemon.plist");
    let bytes = yard_core::render::launchd::daemon_plist(
        Path::new("/y/yardd"),
        Some(root.path()),
        Path::new("/l.log"),
    );
    std::fs::write(&plist, bytes).unwrap();
    let bin = TempDir::new().unwrap();
    let out = yard_at(root.path())
        .env("HOME", home.path())
        .env("PATH", fake_launchctl(bin.path()))
        .env("YARD_SECURITY", "/usr/bin/false")
        .args(["uninstall", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(!plist.exists(), "the LaunchAgent plist is still there: launchd would start yardd at login");
}

#[test]
fn uninstall_of_a_root_that_is_not_there_skips_its_data() {
    let home = TempDir::new().unwrap();
    let parent = short_root();
    let gone = parent.path().join("never-made");
    let out = yard_at(&gone)
        .env("HOME", home.path())
        .env("YARD_SECURITY", "/usr/bin/false")
        .args(["uninstall", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["steps"], json!([]), "{v}");
}

#[test]
fn uninstall_untrusts_a_ca_it_cannot_check() {
    let (root, home) = uninstall_root();
    let paths = yard_core::Paths::new(root.path().to_path_buf());
    yard_daemon::tls::ensure_ca(&paths, "me", time::OffsetDateTime::now_utc()).unwrap();
    // A security that cannot run: whether the CA is trusted is unknown, so it is still removed.
    let out = yard_at(root.path())
        .env("HOME", home.path())
        .env("YARD_SECURITY", "/nonexistent/security")
        .args(["uninstall", "--yes", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let untrust = v["steps"].as_array().unwrap().iter().find(|s| s["step"] == "untrust");
    assert!(untrust.is_some(), "the CA was not untrusted before its file was deleted: {v}");
}
