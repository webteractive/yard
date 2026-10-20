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
        },
        SiteInfo {
            name: "api".into(),
            host: "api.yard".into(),
            url: "http://api.yard:8080/".into(),
            source: SiteSource::Proxy,
            kind: SiteKind::Proxy { target: "http://127.0.0.1:3000".into() },
            secure: false,
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
    assert!(line.starts_with("yard 0.1.0 ("), "{line}");
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
    assert_eq!(v["cli"], "0.1.0");
    assert_eq!(v["protocol"], "2.2");
    assert!(v["commit"].is_string());
    assert!(v["build_date"].is_string());
    assert!(v["daemon"].is_null());
    assert!(v["helper"].is_null());
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

    let out = yard_at(root.path()).arg("doctor").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    for id in ["brew", "formula.nginx", "config", "daemon", "ports"] {
        assert!(text.lines().any(|l| l.split_whitespace().nth(1) == Some(id)), "{id} in {text}");
    }
    assert!(text.contains("yardd is not running") && text.contains("→ yard start"), "{text}");

    let out = yard_at(root.path()).args(["doctor", "--json"]).output().unwrap();
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
        "docroot", "ports", "restart", "helper",
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
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    assert!(first.contains("Downloading"), "{first}");
    let pid = child.id() as libc::pid_t;
    for _ in 0..times {
        // SAFETY: signalling our own child process.
        unsafe { libc::kill(pid, libc::SIGINT) };
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    child.wait_with_output().unwrap()
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
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
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
    assert_eq!(out.status.code(), Some(130), "{}", stderr(&out));
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

#[test]
fn untrust_deletes_by_sha256() {
    let d = MockDaemon::start_sequenced(&[("ca.info", vec![Ok(ca_info(true)), Ok(ca_info(false))])]);
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
