mod support;

use serde_json::{Value, json};
use support::{MOCK_VERSION, MockDaemon, free_port, short_root, stderr, stdout, yard_at};
use yard_proto::{ErrorCode, SiteInfo, SiteKind, SiteSource, YardError};

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
        },
        SiteInfo {
            name: "api".into(),
            host: "api.yard".into(),
            url: "http://api.yard:8080/".into(),
            source: SiteSource::Proxy,
            kind: SiteKind::Proxy { target: "http://127.0.0.1:3000".into() },
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
    assert_eq!(v["protocol"], "2.0");
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
