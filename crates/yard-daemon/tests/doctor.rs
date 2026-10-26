//! Doctor against a temp data root, a fake brew and an in-process helper.

mod support;

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use support::daemon::prepared;
use support::{Fixture, fast, free_port, held_listener, ports_toml, spawn_helper, wait_until};
use tempfile::TempDir;
use yard_core::brew::Brew;
use yard_core::generated::{Hashes, sha256_hex};
use yard_core::render::resolver::resolver_file;
use yard_daemon::doctor::{self, DoctorInput};
use yard_daemon::events::EventBus;
use yard_daemon::helper_client::HelperClient;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::supervisor::Supervisor;
use yard_daemon::tls;
use yard_proto::{Check, CheckStatus, DoctorReport, UnitInfo, UnitState};

const IDS: [&str; 13] = [
    "brew",
    "formula.nginx",
    "formula.dnsmasq",
    "formula.php",
    "config",
    "daemon",
    "helper",
    "resolver",
    "ports",
    "generated",
    "sites",
    "ca",
    "units",
];

struct Env {
    fx: Fixture,
    resolver: TempDir,
    helper: HelperClient,
    dns: u16,
}

async fn env() -> Env {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;
    let dns = free_port();
    std::fs::write(fx.paths.ports_toml(), ports_toml(free_port(), free_port(), dns)).unwrap();
    Env { fx, resolver, helper, dns }
}

impl Env {
    fn brew(&self) -> Brew {
        Brew::new(self.fx.brew.path())
    }

    /// The CLI's view when yardd is down: no state, no units.
    async fn offline(&self, helper: &HelperClient) -> DoctorReport {
        let brew = self.brew();
        doctor::run(DoctorInput {
            paths: &self.fx.paths,
            brew: &brew,
            helper,
            state: None,
            units: vec![],
            daemon_running: false,
            resolver_dir: self.resolver.path(),
            security: &self.fx.security,
            client_env: Default::default(),
            sleeping: Default::default(),
        })
        .await
    }

    /// `blog`, linked to a temp directory and secured, as `yard secure` leaves it.
    fn secured_site(&self) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let toml = format!("[sites.blog]\npath = {:?}\nsecure = true\n", dir.path().display().to_string());
        std::fs::write(self.fx.paths.sites_toml(), toml).unwrap();
        dir
    }

    fn write_resolver(&self, text: &str) {
        std::fs::write(self.resolver.path().join("yard"), text).unwrap();
    }
}

fn ids(report: &DoctorReport) -> Vec<&str> {
    report.checks.iter().map(|c| c.id.as_str()).collect()
}

fn only<'a>(report: &'a DoctorReport, id: &str) -> &'a Check {
    let found: Vec<&Check> = report.checks.iter().filter(|c| c.id == id).collect();
    assert_eq!(found.len(), 1, "one {id} check in {report:#?}");
    found[0]
}

fn check(id: &str, status: CheckStatus) -> Check {
    Check { id: id.into(), status, message: String::new(), hint: None }
}

#[tokio::test]
async fn all_ok_on_healthy_fixture() {
    let e = env().await;
    let bus = EventBus::new();
    let sup = Arc::new(Supervisor::new(
        fast(),
        e.fx.paths.logs_dir(),
        bus.clone(),
        Arc::new(PrivilegedSockets::new(e.helper.clone())),
    ));
    let rec = Reconciler::new(
        e.fx.paths.clone(),
        e.brew(),
        sup.clone(),
        e.helper.clone(),
        bus,
        e.resolver.path().to_path_buf(),
    )
    .unwrap();
    rec.reconcile().await.unwrap();
    let up = || {
        let units = sup.list();
        !units.is_empty() && units.iter().all(|u| u.state == UnitState::Running)
    };
    assert!(wait_until(up, Duration::from_secs(10)).await, "units run: {:?}", sup.list());

    let state = rec.state().await;
    let brew = e.brew();
    let report = doctor::run(DoctorInput {
        paths: &e.fx.paths,
        brew: &brew,
        helper: &e.helper,
        state: Some(&state),
        units: sup.list(),
        daemon_running: true,
        resolver_dir: e.resolver.path(),
        security: &e.fx.security,
        client_env: Default::default(),
        sleeping: Default::default(),
    })
    .await;
    sup.shutdown().await;

    assert_eq!(ids(&report), IDS);
    for c in &report.checks {
        assert_eq!(c.status, CheckStatus::Ok, "{c:?}");
    }
    assert_eq!(doctor::exit_code(&report), 0);
}

#[tokio::test]
async fn missing_formula_hint_names_brew_install() {
    let e = env().await;
    std::fs::remove_file(e.brew().dnsmasq_bin()).unwrap();

    let report = e.offline(&e.helper).await;

    let c = only(&report, "formula.dnsmasq");
    assert_eq!(c.status, CheckStatus::Fail);
    assert_eq!(c.hint.as_deref(), Some("brew install dnsmasq"));
    assert_eq!(only(&report, "formula.nginx").status, CheckStatus::Ok);
}

#[tokio::test]
async fn foreign_resolver_fails() {
    let e = env().await;
    e.write_resolver("nameserver 10.0.0.1\n");

    let report = e.offline(&e.helper).await;

    let c = only(&report, "resolver");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(c.message.contains("another tool"), "{c:?}");
    assert!(c.hint.as_deref().unwrap().contains("yard.toml"), "{c:?}");
}

#[tokio::test]
async fn resolver_on_wrong_port_warns() {
    let e = env().await;
    e.write_resolver(&resolver_file(e.dns + 1));

    let report = e.offline(&e.helper).await;

    let c = only(&report, "resolver");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains(&e.dns.to_string()), "{c:?}");
    assert!(c.hint.is_some());
}

#[tokio::test]
async fn port_held_by_listener_fails_with_holder() {
    let e = env().await;
    let listener = held_listener();
    let held = listener.local_addr().unwrap().port();
    std::fs::write(e.fx.paths.ports_toml(), ports_toml(held, free_port(), e.dns)).unwrap();

    let report = e.offline(&e.helper).await;

    let c = only(&report, "ports");
    assert_eq!(c.status, CheckStatus::Fail);
    for part in ["nginx-http", &held.to_string(), &format!("pid {}", std::process::id())] {
        assert!(c.message.contains(part), "{part} in {c:?}");
    }
    assert!(c.hint.as_deref().unwrap().contains("yard ports set nginx-http"), "{c:?}");
    drop(listener);
}

#[tokio::test]
async fn hand_edited_file_warns_with_override_path() {
    let e = env().await;
    let conf = e.fx.paths.nginx_site("blog");
    std::fs::write(&conf, "server { listen 1; }\n").unwrap();
    let mut hashes = Hashes::default();
    hashes.0.insert(conf.clone(), sha256_hex("what Yard wrote\n"));
    hashes.save(&e.fx.paths.hashes()).unwrap();

    let report = e.offline(&e.helper).await;

    let c = only(&report, "generated");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains(&conf.display().to_string()), "{c:?}");
    assert!(c.message.contains(&e.fx.paths.nginx_override("blog").display().to_string()), "{c:?}");
    assert!(c.hint.is_some());
}

#[tokio::test]
async fn failed_unit_reports_last_lines() {
    let e = env().await;
    let lines: Vec<String> = (1..=8).map(|n| format!("line {n}")).collect();
    let failed = UnitInfo {
        name: "dnsmasq".into(),
        state: UnitState::Failed { last_lines: lines },
        pid: None,
        restarts: 5,
        last_error: Some("exited with status 1".into()),
    };
    let brew = e.brew();

    let report = doctor::run(DoctorInput {
        paths: &e.fx.paths,
        brew: &brew,
        helper: &e.helper,
        state: None,
        units: vec![failed],
        daemon_running: true,
        resolver_dir: e.resolver.path(),
        security: &e.fx.security,
        client_env: Default::default(),
        sleeping: Default::default(),
    })
    .await;

    let c = only(&report, "units");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(c.message.contains("dnsmasq") && c.message.contains("line 8"), "{c:?}");
    assert!(c.message.contains("line 4") && !c.message.contains("line 3"), "the last 5 lines: {c:?}");
    assert_eq!(c.hint.as_deref(), Some("yard restart dnsmasq"));
}

#[tokio::test]
async fn offline_mode_reports_daemon_not_running_and_still_runs_local_checks() {
    let e = env().await;
    std::fs::write(e.fx.paths.yard_toml(), "tld = [\n").unwrap();
    let gone = tempfile::tempdir().unwrap();
    let no_helper = HelperClient::new(gone.path().join("helper.sock"));

    let report = e.offline(&no_helper).await;

    assert_eq!(ids(&report), IDS);
    let daemon = only(&report, "daemon");
    assert_eq!((daemon.status, daemon.hint.as_deref()), (CheckStatus::Fail, Some("yard start")));
    let helper = only(&report, "helper");
    assert_eq!(
        (helper.status, helper.hint.as_deref()),
        (CheckStatus::Fail, Some("sudo yard helper install"))
    );
    let config = only(&report, "config");
    assert_eq!(config.status, CheckStatus::Fail);
    assert!(config.message.contains("yard.toml line "), "{config:?}");
    for local in ["brew", "formula.nginx", "formula.dnsmasq", "formula.php"] {
        assert_eq!(only(&report, local).status, CheckStatus::Ok, "{local}");
    }
    assert_eq!(doctor::exit_code(&report), 1);
}

#[test]
fn exit_code_ignores_warnings() {
    let warned =
        DoctorReport { checks: vec![check("brew", CheckStatus::Ok), check("sites", CheckStatus::Warn)] };
    assert_eq!(doctor::exit_code(&warned), 0);
    let failed =
        DoctorReport { checks: vec![check("sites", CheckStatus::Warn), check("daemon", CheckStatus::Fail)] };
    assert_eq!(doctor::exit_code(&failed), 1);
}

#[tokio::test]
async fn failed_reconcile_fails_config_and_units() {
    let e = env().await;
    let mut state = yard_daemon::state::State::new(e.fx.paths.clone(), e.brew()).unwrap();
    state.last_error = Some(
        yard_proto::YardError::new(
            yard_proto::ErrorCode::NginxConfigInvalid,
            "nginx rejected the new config",
        )
        .with_hint("fix the file named above"),
    );
    let brew = e.brew();

    let report = doctor::run(DoctorInput {
        paths: &e.fx.paths,
        brew: &brew,
        helper: &e.helper,
        state: Some(&state),
        units: vec![],
        daemon_running: true,
        resolver_dir: e.resolver.path(),
        security: &e.fx.security,
        client_env: Default::default(),
        sleeping: Default::default(),
    })
    .await;

    let config = only(&report, "config");
    assert_eq!(config.status, CheckStatus::Fail);
    assert!(config.message.contains("nginx rejected"), "{config:?}");
    let units = only(&report, "units");
    assert_eq!(units.status, CheckStatus::Fail, "yardd runs but serves nothing: {units:?}");
    assert!(units.hint.is_some());
}

impl Env {
    /// A linked site `blog` whose composer.json requires `php`.
    fn blog(&self, php: &str) -> TempDir {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("public")).unwrap();
        let composer = serde_json::json!({"require": {"php": php}});
        std::fs::write(project.path().join("composer.json"), composer.to_string()).unwrap();
        let sites = format!("[sites.blog]\npath = {:?}\n", project.path().display().to_string());
        std::fs::write(self.fx.paths.sites_toml(), sites).unwrap();
        project
    }
}

#[tokio::test]
async fn composer_php_mismatch_warns_with_the_isolate_command() {
    let e = env().await;
    let _project = e.blog("^7.1");

    let report = e.offline(&e.helper).await;
    let c = only(&report, "php-constraint:blog");
    assert_eq!(c.status, CheckStatus::Warn);
    assert_eq!(c.message, "blog wants php ^7.1 (composer.json) but runs 8.4");
    assert_eq!(c.hint.as_deref(), Some("yard php install 7.1"), "none installed: the lowest that fits");

    e.fx.add_php("7.1");
    e.fx.add_php("7.4");
    let report = e.offline(&e.helper).await;
    assert_eq!(only(&report, "php-constraint:blog").hint.as_deref(), Some("yard isolate 7.4 --site blog"));
}

#[tokio::test]
async fn unparsable_constraint_gives_no_check() {
    let e = env().await;
    for php in ["dev-main", "^8.0"] {
        let _project = e.blog(php);
        let report = e.offline(&e.helper).await;
        assert!(!report.checks.iter().any(|c| c.id.starts_with("php-constraint")), "{php}: {report:#?}");
    }
}

#[tokio::test]
async fn a_composer_json_that_is_not_a_regular_file_is_skipped() {
    let e = env().await;
    let project = e.blog("^7.1");
    let composer = project.path().join("composer.json");
    std::fs::remove_file(&composer).unwrap();
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&composer).status().unwrap().success());
    let report = tokio::time::timeout(Duration::from_secs(5), e.offline(&e.helper))
        .await
        .expect("doctor does not block on a fifo");
    assert!(!report.checks.iter().any(|c| c.id.starts_with("php-constraint")), "{report:#?}");
}

#[tokio::test]
async fn ca_check_ok_without_https() {
    let e = env().await;
    let report = e.offline(&e.helper).await;
    let c = only(&report, "ca");
    assert_eq!(c.status, CheckStatus::Ok, "{c:?}");
    assert_eq!(c.message, "no site uses HTTPS");
    assert!(!e.fx.security_dir().join("security.log").exists(), "security is not asked without a CA");
}

#[tokio::test]
async fn ca_check_warns_when_untrusted() {
    let e = env().await;
    let _project = e.secured_site();
    for made in [false, true] {
        if made {
            tls::ensure_ca(&e.fx.paths, "me", time::OffsetDateTime::now_utc()).unwrap();
        }
        let report = e.offline(&e.helper).await;
        let c = only(&report, "ca");
        assert_eq!(c.status, CheckStatus::Warn, "CA made: {made}, {c:?}");
        assert_eq!(c.message, "browsers will warn on https sites: the Yard CA is not trusted");
        assert_eq!(c.hint.as_deref(), Some("yard trust"));
    }
}

#[tokio::test]
async fn ca_check_names_node_extra_ca_certs_when_trusted() {
    let e = env().await;
    let _project = e.secured_site();
    tls::ensure_ca(&e.fx.paths, "me", time::OffsetDateTime::now_utc()).unwrap();
    std::fs::write(e.fx.security_dir().join("fake-trusted"), "").unwrap();

    let report = e.offline(&e.helper).await;
    let c = only(&report, "ca");
    assert_eq!(c.status, CheckStatus::Ok, "{c:?}");
    let ca = e.fx.paths.ca_cert();
    let expected = format!(
        "the Yard CA is trusted in your login keychain; for Node: NODE_EXTRA_CA_CERTS={}",
        ca.display()
    );
    assert!(c.message.starts_with(&expected), "{c:?}");
}

#[tokio::test]
async fn ca_check_warns_when_security_does_not_answer() {
    let e = env().await;
    let _project = e.secured_site();
    tls::ensure_ca(&e.fx.paths, "me", time::OffsetDateTime::now_utc()).unwrap();
    std::fs::write(e.fx.security_dir().join("fake-hang"), "").unwrap();

    let started = std::time::Instant::now();
    let report = e.offline(&e.helper).await;
    assert!(started.elapsed() < Duration::from_secs(20), "doctor waited {:?}", started.elapsed());
    let c = only(&report, "ca");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("security did not answer"), "{c:?}");
}

#[tokio::test]
async fn security_does_not_inherit_a_listener() {
    let e = env().await;
    tls::ensure_ca(&e.fx.paths, "me", time::OffsetDateTime::now_utc()).unwrap();
    std::fs::write(e.fx.security_dir().join("fake-hang"), "").unwrap();
    let (listener, port) = support::inheritable_listener();

    let (security, ca) = (e.fx.security.clone(), e.fx.paths.ca_cert());
    let check = tokio::spawn(async move { tls::trusted(&security, &ca).await });
    let log = e.fx.security_dir().join("security.log");
    let started = || std::fs::read_to_string(&log).is_ok_and(|l| l.contains("verify-cert"));
    assert!(wait_until(started, Duration::from_secs(5)).await, "security ran");
    drop(listener);
    // Another test's child spawned meanwhile can inherit the listener too, so only this
    // fixture's `security` holding it is a failure.
    let ours = std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
        && holders(port).iter().any(|args| args.starts_with(&e.fx.security.display().to_string()));
    check.abort();
    assert!(!ours, "security kept the listener on {port}");
}

/// The command line of each process listening on `port`.
fn holders(port: u16) -> Vec<String> {
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|pid| {
            let ps =
                std::process::Command::new("ps").args(["-o", "args=", "-p", pid.trim()]).output().ok()?;
            Some(String::from_utf8_lossy(&ps.stdout).trim().to_string())
        })
        .collect()
}

impl Env {
    /// A linked site `name` whose project holds `.yard` with `text`.
    fn yard_site(&self, name: &str, text: &str, dir: &tempfile::TempDir) -> std::path::PathBuf {
        let project = dir.path().join(name);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".yard"), text).unwrap();
        let mut sites = std::fs::read_to_string(self.fx.paths.sites_toml()).unwrap_or_default();
        sites.push_str(&format!("[sites.{name}]\npath = {:?}\n", project.display().to_string()));
        std::fs::write(self.fx.paths.sites_toml(), sites).unwrap();
        project
    }
}

#[tokio::test]
async fn doctor_reports_project_files() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        e.fx.paths.yard_toml(),
        "[php.\"8.4\".extensions]\nxdebug = { source = \"pecl\", enabled = false }\n",
    )
    .unwrap();
    e.yard_site("met", "php = \"8.4\"\n", &dir);
    let needy = e.yard_site("needy", "php = \"7.3\"\n", &dir);
    let bad = e.yard_site("bad", "nodejs = \"20\"\n", &dir);
    let both = e.yard_site("both", "", &dir);
    std::fs::write(both.join(".yard-php"), "7.1\n").unwrap();
    e.yard_site("off", "extensions = [\"xdebug\"]\n", &dir);

    let report = e.offline(&e.helper).await;
    assert_eq!(only(&report, "project-file:met").status, CheckStatus::Ok);
    let c = only(&report, "project-file:needy");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert_eq!(c.message, "needy needs PHP 7.3");
    assert_eq!(c.hint.as_deref(), Some(format!("cd {} && yard init", needy.display()).as_str()));
    let c = only(&report, "project-file:bad");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert!(c.message.contains("node"), "{c:?}");
    assert_eq!(c.hint.as_deref(), Some(format!("edit {}", bad.join(".yard").display()).as_str()));
    let c = only(&report, "yard-php-ignored:both");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.hint.as_deref(), Some(format!("delete {}", both.join(".yard-php").display()).as_str()));
    let c = only(&report, "project-file:off");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.hint.as_deref(), Some("yard ext enable xdebug --php 8.4"));
    assert!(!report.checks.iter().any(|c| c.id == "yard-php-ignored:met"));
}

impl Env {
    /// Yard's managed `.yard` for `name`, with `text`; returns its path.
    fn managed(&self, name: &str, text: &str) -> std::path::PathBuf {
        let file = self.fx.paths.managed_project_file(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        file
    }

    /// A linked site `name` whose project has no `.yard`.
    fn plain_site(&self, name: &str, dir: &tempfile::TempDir) -> std::path::PathBuf {
        let project = self.yard_site(name, "", dir);
        std::fs::remove_file(project.join(".yard")).unwrap();
        project
    }
}

#[tokio::test]
async fn a_managed_file_hidden_by_a_committed_one_warns() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    let project = e.yard_site("control", "php = \"8.4\"\n", &dir);
    let file =
        e.managed("control", &format!("php = \"7.4\"\nproject = {:?}\n", project.display().to_string()));

    let report = e.offline(&e.helper).await;
    let c = only(&report, "managed-file-ignored:control");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.message, format!("control has a committed .yard, so {} is ignored", file.display()));
    assert_eq!(c.hint.as_deref(), Some(format!("rm {}", file.display()).as_str()));
    assert_eq!(only(&report, "project-file:control").status, CheckStatus::Ok);
}

#[tokio::test]
async fn a_managed_file_for_another_directory_warns() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    let app = e.plain_site("app", &dir);
    let file = e.managed("app", "php = \"7.4\"\nproject = \"/elsewhere/app\"\n");
    let shop = e.plain_site("shop", &dir);
    let unnamed = e.managed("shop", "php = \"7.4\"\n");

    let report = e.offline(&e.helper).await;
    let c = only(&report, "managed-file-mismatch:app");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.message, format!("{} is for /elsewhere/app, not {}", file.display(), app.display()));
    assert_eq!(c.hint.as_deref(), Some(format!("cd {} && yard init --replace", app.display()).as_str()));
    let c = only(&report, "managed-file-mismatch:shop");
    assert_eq!(c.message, format!("{} is for no project, not {}", unnamed.display(), shop.display()));
    not_present(&report, "managed-file-orphan");
}

#[tokio::test]
async fn a_managed_file_with_no_linked_site_is_reported_without_a_warning() {
    let e = env().await;
    let gone = e.managed("gone", "php = \"7.4\"\nproject = \"/code/gone\"\n");
    std::fs::write(e.fx.paths.sites_toml(), "[sites.api]\nproxy = \"http://127.0.0.1:3000\"\n").unwrap();
    let api = e.managed("api", "php = \"7.4\"\nproject = \"/code/api\"\n");
    std::fs::create_dir_all(e.fx.paths.managed_sites_dir().join("empty")).unwrap();
    let nameless = e.managed("nameless", "php = \"7.4\"\n");

    let report = e.offline(&e.helper).await;
    // The hint runs yard link where the file was written for, when it says.
    for (name, file, hint) in [
        ("gone", &gone, "cd /code/gone && yard link gone"),
        ("api", &api, "cd /code/api && yard link api"),
        ("nameless", &nameless, "yard link nameless"),
    ] {
        let c = only(&report, &format!("managed-file-orphan:{name}"));
        assert_eq!(c.status, CheckStatus::Ok, "{c:?}");
        assert_eq!(c.message, format!("{} belongs to no linked site", file.display()));
        assert_eq!(c.hint.as_deref(), Some(hint));
    }
    assert!(!report.checks.iter().any(|c| c.id == "managed-file-orphan:empty"));
    not_present(&report, "managed-file-mismatch");
}

#[tokio::test]
async fn an_unreadable_services_toml_makes_no_site_a_service_need() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.yard_site("blog", "[services]\nmysql = \"8.4\"\n", &dir);

    // Readable and empty, there is no instance, so the site needs one.
    let report = e.offline(&e.helper).await;
    let c = only(&report, "project-file:blog");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert_eq!(c.message, "blog needs MySQL 8.4");

    // Unreadable, the instance may be in the file: that is the config check's to say.
    std::fs::write(e.fx.paths.services_toml(), "bogus = 1\n").unwrap();
    let report = e.offline(&e.helper).await;
    let c = only(&report, "project-file:blog");
    assert_eq!(c.status, CheckStatus::Ok, "{c:?}");
    assert_eq!(only(&report, "config").status, CheckStatus::Fail);
}

// ---- Service instances and brew sources (M4b spec §10) ----

fn unit(name: &str, state: UnitState) -> UnitInfo {
    UnitInfo { name: name.into(), state, pid: None, restarts: 0, last_error: None }
}

fn failed(name: &str) -> UnitInfo {
    let last_lines = (1..=8).map(|n| format!("line {n}")).collect();
    UnitInfo {
        last_error: Some("exited with status 1".into()),
        ..unit(name, UnitState::Failed { last_lines })
    }
}

fn running(name: &str) -> UnitInfo {
    unit(name, UnitState::Running)
}

impl Env {
    /// Doctor as yardd runs it, with its units and what the CLI's shell sets.
    async fn with(&self, units: Vec<UnitInfo>, client_env: &[(&str, &str)]) -> DoctorReport {
        let brew = self.brew();
        doctor::run(DoctorInput {
            paths: &self.fx.paths,
            brew: &brew,
            helper: &self.helper,
            state: None,
            units,
            daemon_running: true,
            resolver_dir: self.resolver.path(),
            security: &self.fx.security,
            client_env: client_env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            sleeping: Default::default(),
        })
        .await
    }

    /// An instance as `yard service add` leaves it: its keg and its `services.toml` entry.
    fn instance(&self, instance: &str, formula: &str, stopped: bool) {
        self.fx.add_formula(formula.rsplit('/').next().unwrap());
        self.fx.add_service(instance, formula, stopped);
    }

    /// A data directory the engine's init made.
    fn data(&self, instance: &str) {
        let data = self.fx.paths.service_data(instance);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("ibdata1"), "x").unwrap();
    }

    /// What the fake brew knows of formulae (`brew info --json=v2`'s `formulae`).
    fn formulae(&self, formulae: Value) {
        std::fs::write(self.fx.brew.path().join("fake-formulae.json"), formulae.to_string()).unwrap();
    }

    fn trust(&self, taps: &[&str], formulae: &[&str]) {
        let doc = json!({"taps": taps, "formulae": formulae, "casks": [], "commands": []});
        std::fs::write(self.fx.brew.path().join("fake-trust.json"), doc.to_string()).unwrap();
    }

    fn brew_log(&self) -> String {
        std::fs::read_to_string(self.fx.brew.path().join("fake-brew.log")).unwrap_or_default()
    }
}

fn formula(name: &str, version: &str) -> Value {
    json!({"name": name, "full_name": name, "tap": "homebrew/core", "versions": {"stable": version}})
}

fn not_present(report: &DoctorReport, prefix: &str) {
    assert!(!report.checks.iter().any(|c| c.id.starts_with(prefix)), "no {prefix}* check in {report:#?}");
}

#[tokio::test]
async fn a_failed_service_fails_with_its_last_lines_and_the_restart_hint() {
    let e = env().await;
    e.instance("mysql-8.4", "mysql@8.4", false);
    e.data("mysql-8.4");
    e.instance("redis", "redis", false);
    e.formulae(json!([formula("mysql@8.4", "8.4.13"), formula("redis", "8.0.1")]));

    let report = e.with(vec![failed("mysql-8.4"), running("redis")], &[]).await;

    let c = only(&report, "service:mysql-8.4");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert!(c.message.contains("mysql-8.4 failed: exited with status 1"), "{c:?}");
    assert!(c.message.contains("line 8") && c.message.contains("line 4"), "{c:?}");
    assert!(!c.message.contains("line 3"), "the last 5 lines: {c:?}");
    assert_eq!(c.hint.as_deref(), Some("yard service restart mysql-8.4"));
    assert_eq!(only(&report, "service:redis").status, CheckStatus::Ok);
}

#[tokio::test]
async fn an_instance_without_its_keg_names_the_brew_command() {
    let e = env().await;
    e.fx.add_service("redis", "redis", false);
    e.formulae(json!([formula("redis", "8.0.1")]));

    // yardd runs no unit for it: reconcile left it out for the missing keg.
    let report = e.with(vec![], &[]).await;
    let c = only(&report, "service:redis");
    assert_ne!(c.status, CheckStatus::Ok, "{c:?}");
    let keg = e.fx.brew.path().join("opt/redis");
    assert!(c.message.contains(&keg.display().to_string()), "{c:?}");
    assert_eq!(c.hint.as_deref(), Some("brew install redis"), "{c:?}");
}

#[tokio::test]
async fn a_service_port_held_by_something_else_fails_the_port_check() {
    let e = env().await;
    e.instance("mysql-8.4", "mysql@8.4", false);
    e.data("mysql-8.4");
    let listener = held_listener();
    let held = listener.local_addr().unwrap().port();
    let mut ports = ports_toml(free_port(), free_port(), e.dns);
    ports.push_str(&format!("\"mysql-8.4\" = {{ port = {held}, mode = \"pinned\" }}\n"));
    std::fs::write(e.fx.paths.ports_toml(), ports).unwrap();

    let report = e.with(vec![], &[]).await;

    let c = only(&report, "ports");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert!(c.message.contains("mysql-8.4") && c.message.contains(&held.to_string()), "{c:?}");
    assert!(c.message.contains(&format!("pid {}", std::process::id())), "{c:?}");
    assert!(c.hint.as_deref().unwrap().contains("yard ports set mysql-8.4"), "{c:?}");
    drop(listener);
}

#[tokio::test]
async fn a_database_with_no_data_fails_with_the_commands_to_start_over() {
    let e = env().await;
    e.instance("mysql-8.4", "mysql@8.4", false);
    e.instance("postgres-17", "postgresql@17", false);
    e.instance("redis", "redis", false);
    e.instance("minio", "minio/stable/minio", false);
    e.trust(&["shivammathur/php", "minio/stable"], &[]);

    // No `data/` at all.
    let report = e.offline(&e.helper).await;
    let c = only(&report, "service-data:mysql-8.4");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert_eq!(
        c.hint.as_deref(),
        Some("yard service remove mysql-8.4 && yard service add mysql --version 8.4")
    );
    let c = only(&report, "service-data:postgres-17");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert_eq!(
        c.hint.as_deref(),
        Some("yard service remove postgres-17 && yard service add postgres --version 17")
    );

    // An empty one is as bad; one that holds something is fine.
    std::fs::create_dir_all(e.fx.paths.service_data("mysql-8.4")).unwrap();
    e.data("postgres-17");
    let report = e.offline(&e.helper).await;
    assert_eq!(only(&report, "service-data:mysql-8.4").status, CheckStatus::Fail);
    assert_eq!(only(&report, "service-data:postgres-17").status, CheckStatus::Ok);

    // Redis's directory is made by reconcile and MinIO makes its own: neither is checked.
    for engine in ["redis", "minio"] {
        assert!(!report.checks.iter().any(|c| c.id == format!("service-data:{engine}")), "{engine}");
    }
}

#[tokio::test]
async fn a_site_asking_for_a_stopped_failed_or_missing_instance_warns() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.instance("mysql-8.4", "mysql@8.4", true);
    e.data("mysql-8.4");
    e.instance("redis", "redis", false);
    e.instance("postgres-17", "postgresql@17", false);
    e.data("postgres-17");
    e.formulae(json!([
        formula("mysql@8.4", "8.4.13"),
        formula("redis", "8.0.1"),
        formula("postgresql@17", "17.2")
    ]));
    e.yard_site("stopped", "[services]\nmysql = \"8.4\"\n", &dir);
    e.yard_site("broken", "[services]\nredis = true\n", &dir);
    e.yard_site("missing", "[services]\npostgres = \"16\"\n", &dir);
    e.yard_site("fine", "[services]\npostgres = \"17\"\n", &dir);

    let report = e.with(vec![failed("redis"), running("postgres-17")], &[]).await;

    let c = only(&report, "service-need:stopped");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.message, "stopped's .yard asks for mysql-8.4, which is stopped");
    assert_eq!(c.hint.as_deref(), Some("yard service start mysql-8.4"));
    let c = only(&report, "service-need:broken");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.message, "broken's .yard asks for redis, which has failed");
    assert_eq!(c.hint.as_deref(), Some("yard service restart redis"));
    let c = only(&report, "service-need:missing");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.message, "missing's .yard asks for postgres-16, which is not added");
    assert_eq!(c.hint.as_deref(), Some("yard service add postgres --version 16"));
    assert!(!report.checks.iter().any(|c| c.id == "service-need:fine"), "{report:#?}");

    // With no daemon only what the files say is known: the instance is stopped, not failed.
    let report = e.offline(&e.helper).await;
    assert_eq!(only(&report, "service-need:stopped").status, CheckStatus::Warn);
    assert!(!report.checks.iter().any(|c| c.id == "service-need:broken"), "{report:#?}");
}

#[tokio::test]
async fn a_plain_formula_instance_on_a_newer_line_than_its_name_warns() {
    let e = env().await;
    e.fx.add_service("mysql-9.7", "mysql", false);
    e.fx.add_service("mysql-8.4", "mysql@8.4", false);
    e.fx.add_formula("mysql@8.4");
    e.data("mysql-9.7");
    e.data("mysql-8.4");
    e.formulae(json!([formula("mysql", "26.10.0"), formula("mysql@8.4", "8.4.13")]));
    // `brew upgrade mysql` moved the plain formula's keg from 9.7 to 26.10.
    let keg = |version: &str| {
        let cellar = e.fx.brew.path().join("Cellar/mysql").join(version);
        std::fs::create_dir_all(cellar.join("bin")).unwrap();
        std::fs::write(cellar.join("bin/mysqld"), "").unwrap();
        let opt = e.fx.brew.path().join("opt/mysql");
        let _ = std::fs::remove_file(&opt);
        std::os::unix::fs::symlink(format!("../Cellar/mysql/{version}"), opt).unwrap();
    };
    keg("26.10.0_1");

    let report = e.with(vec![running("mysql-9.7"), running("mysql-8.4")], &[]).await;

    let c = only(&report, "service-line:mysql-9.7");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("26.10") && c.message.contains("mysql-9.7"), "{c:?}");
    assert!(c.hint.as_deref().unwrap().contains("yard service add mysql --version 8.4"), "{c:?}");
    assert!(
        !report.checks.iter().any(|c| c.id == "service-line:mysql-8.4"),
        "a versioned formula never moves"
    );

    // The keg is still on the line its name says: nothing to say.
    keg("9.7.1");
    let report = e.with(vec![running("mysql-9.7"), running("mysql-8.4")], &[]).await;
    not_present(&report, "service-line");
}

#[tokio::test]
async fn a_deprecated_formula_warns_with_its_disable_date_from_one_brew_call() {
    let e = env().await;
    e.instance("mysql-8.0", "mysql@8.0", false);
    e.data("mysql-8.0");
    e.instance("redis", "redis", false);
    let old = json!({
        "name": "mysql@8.0", "full_name": "mysql@8.0", "tap": "homebrew/core", "versions": {"stable": "8.0.46"},
        "deprecated": true, "deprecation_date": "2026-04-30", "disabled": false, "disable_date": "2027-04-30"
    });
    e.formulae(json!([old, formula("redis", "8.0.1")]));

    let report = e.with(vec![running("mysql-8.0"), running("redis")], &[]).await;

    let c = only(&report, "service-deprecated:mysql-8.0");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("mysql@8.0") && c.message.contains("2027-04-30"), "{c:?}");
    assert!(c.hint.is_some(), "{c:?}");
    assert!(!report.checks.iter().any(|c| c.id == "service-deprecated:redis"), "{report:#?}");
    let calls: Vec<String> =
        e.brew_log().lines().filter(|l| l.starts_with("info --json=v2")).map(String::from).collect();
    assert_eq!(calls, ["info --json=v2 mysql@8.0 redis"], "one call for every instance's formula");
}

#[tokio::test]
async fn brew_failing_to_describe_the_formulae_is_a_warning_not_an_error() {
    let e = env().await;
    e.instance("redis", "redis", false);
    // brew does not know redis here, so `info` fails as it does for an unknown formula.
    e.formulae(json!([]));

    let report = e.with(vec![running("redis")], &[]).await;

    let c = only(&report, "service-deprecated");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("brew"), "{c:?}");
    let broken = |c: &Check| c.status == CheckStatus::Fail && c.id.starts_with("service");
    assert!(!report.checks.iter().any(broken), "never a doctor error: {report:#?}");
}

#[tokio::test]
async fn an_installed_formula_from_an_untrusted_tap_warns_with_the_trust_command() {
    let e = env().await;
    e.fx.add_php("7.1");
    e.instance("minio", "minio/stable/minio", false);
    // php@8.4 (the fixture's) and minio are untrusted; php@7.1 is trusted by itself.
    e.trust(&[], &["shivammathur/php/php@7.1"]);

    let report = e.offline(&e.helper).await;

    let trust: Vec<&Check> = report.checks.iter().filter(|c| c.id == "sources:trust").collect();
    let hints: Vec<&str> = trust.iter().filter_map(|c| c.hint.as_deref()).collect();
    assert_eq!(hints, ["yard source trust minio/stable/minio", "yard source trust shivammathur/php/php@8.4"]);
    assert!(trust.iter().all(|c| c.status == CheckStatus::Warn), "{trust:?}");
    let message = &trust[1].message;
    assert!(message.contains("shivammathur/php/php@8.4") && message.contains("trust"), "{message}");

    // Trusted, nothing is said.
    e.trust(&["shivammathur/php", "minio/stable"], &[]);
    let report = e.offline(&e.helper).await;
    not_present(&report, "sources:trust");
}

#[tokio::test]
async fn core_minio_is_never_reported_as_an_untrusted_tap_formula() {
    let e = env().await;
    let core = json!({
        "name": "minio", "full_name": "minio", "tap": "homebrew/core", "versions": {"stable": "2025.9.7"},
        "deprecated": true, "disable_date": "2027-02-17"
    });
    e.formulae(json!([core]));
    e.trust(&["shivammathur/php"], &[]);
    // homebrew/core's minio shares the keg `minio/stable/minio` would use.
    e.instance("minio", "minio", false);

    let report = e.with(vec![running("minio")], &[]).await;

    not_present(&report, "sources:trust");
    let c = only(&report, "service-deprecated:minio");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("2027-02-17"), "{c:?}");
}

#[tokio::test]
async fn brew_older_than_six_warns_that_it_predates_tap_trust() {
    let e = env().await;
    std::fs::write(e.fx.brew.path().join("fake-brew-version"), "5.9.0\n").unwrap();
    e.trust(&[], &[]);

    let report = e.offline(&e.helper).await;

    let c = only(&report, "sources:brew");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("Homebrew 5") && c.message.contains("6.0"), "{c:?}");
    assert!(c.hint.is_some());
    not_present(&report, "sources:trust");

    std::fs::write(e.fx.brew.path().join("fake-brew-version"), "6.0.0\n").unwrap();
    let report = e.offline(&e.helper).await;
    not_present(&report, "sources:brew");
}

#[tokio::test]
async fn a_shell_setting_brew_reads_that_yardd_lacks_warns_with_key_names_only() {
    let e = env().await;
    let id = std::process::id();
    let (xdg, forbidden) = (format!("/xdg-value-{id}"), format!("secret-value-{id}"));
    let env = [
        ("XDG_CONFIG_HOME", xdg.as_str()),
        ("HOMEBREW_FORBIDDEN_FORMULAE", forbidden.as_str()),
        ("HOMEBREW_GITHUB_API_TOKEN", "token-value"),
        ("PATH", "/shell/path"),
    ];

    let report = e.with(vec![], &env).await;

    let c = only(&report, "sources:env");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message.contains("XDG_CONFIG_HOME") && c.message.contains("HOMEBREW_FORBIDDEN_FORMULAE"),
        "{c:?}"
    );
    for private in [&xdg, &forbidden, "token-value", "HOMEBREW_GITHUB_API_TOKEN", "PATH", "/shell/path"] {
        assert!(
            !c.message.contains(private) && !c.hint.as_deref().unwrap().contains(private),
            "{private}: {c:?}"
        );
    }
    assert_eq!(c.hint.as_deref(), Some("put it in ~/.homebrew/brew.env"));

    // Nothing the CLI sets that brew reads: nothing to say.
    let report = e.with(vec![], &[("PATH", "/shell/path")]).await;
    not_present(&report, "sources:env");
}

#[tokio::test]
async fn a_php_70_site_asking_for_mysql_warns_about_authentication() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.fx.add_php("7.0");
    e.fx.add_php("7.3");
    e.instance("mysql-8.4", "mysql@8.4", false);
    e.data("mysql-8.4");
    e.formulae(json!([formula("mysql@8.4", "8.4.13")]));
    e.yard_site("legacy", "php = \"7.0\"\n[services]\nmysql = \"8.4\"\n", &dir);
    e.yard_site("newer", "php = \"7.3\"\n[services]\nmysql = \"8.4\"\n", &dir);
    e.yard_site("cache", "php = \"7.0\"\n[services]\nredis = true\n", &dir);

    let report = e.offline(&e.helper).await;

    let c = only(&report, "php7.0-mysql:legacy");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("authentication method unknown"), "{c:?}");
    assert!(c.message.contains("legacy"), "{c:?}");
    assert!(c.hint.is_some());
    assert!(!report.checks.iter().any(|c| c.id == "php7.0-mysql:newer" || c.id == "php7.0-mysql:cache"));
}

#[tokio::test]
async fn an_unreadable_services_toml_adds_no_service_checks() {
    let e = env().await;
    e.instance("redis", "redis", false);
    std::fs::write(e.fx.paths.services_toml(), "bogus = 1\n").unwrap();

    let report = e.with(vec![running("redis")], &[]).await;

    assert_eq!(only(&report, "config").status, CheckStatus::Fail);
    not_present(&report, "service");
    assert!(!e.brew_log().contains("info --json=v2"), "brew is not asked about instances nobody can list");
}

#[tokio::test]
async fn healthy_services_are_ok() {
    let (mysql, redis) = (free_port(), free_port());
    let pins = |unit: &str, port: u16| format!("\"{unit}\" = {{ port = {port}, mode = \"pinned\" }}\n");
    let d = prepared(&(pins("mysql-8.4", mysql) + &pins("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    d.fx.add_service("redis", "redis", false);
    let data = d.fx.paths.service_data("mysql-8.4");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("ibdata1"), "x").unwrap();
    let formulae = json!([formula("mysql@8.4", "8.4.13"), formula("redis", "8.0.1")]);
    std::fs::write(d.fx.brew.path().join("fake-formulae.json"), formulae.to_string()).unwrap();
    let project = d.project("blog");
    std::fs::write(project.join(".yard"), "[services]\nmysql = \"8.4\"\nredis = true\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let up = |units: &Value, name: &str| {
        units.as_array().unwrap().iter().any(|u| u["name"] == name && u["state"] == "running")
    };
    for _ in 0..200 {
        let status = c.call("status", Value::Null).await.unwrap();
        if up(&status["units"], "mysql-8.4") && up(&status["units"], "redis") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let report: DoctorReport = serde_json::from_value(c.call("doctor", Value::Null).await.unwrap()).unwrap();

    for id in ["service:mysql-8.4", "service:redis", "service-data:mysql-8.4"] {
        assert_eq!(only(&report, id).status, CheckStatus::Ok, "{id}: {report:#?}");
    }
    let problems: Vec<&Check> =
        report.checks.iter().filter(|c| c.id.starts_with("service") || c.id.starts_with("sources")).collect();
    assert!(problems.iter().all(|c| c.status == CheckStatus::Ok), "{problems:#?}");
    not_present(&report, "service-need");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn doctor_takes_null_absent_and_filled_in_params() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let id = std::process::id();

    // Older clients send no params at all.
    for params in [Value::Null, json!({}), json!({"client_env": {}})] {
        let report: DoctorReport = serde_json::from_value(c.call("doctor", params).await.unwrap()).unwrap();
        assert_eq!(only(&report, "daemon").status, CheckStatus::Ok);
        not_present(&report, "sources:env");
    }
    let params = json!({"client_env": {"HOMEBREW_FORBIDDEN_FORMULAE": format!("rpc-value-{id}")}});
    let report: DoctorReport = serde_json::from_value(c.call("doctor", params).await.unwrap()).unwrap();
    let env = only(&report, "sources:env");
    assert_eq!(env.status, CheckStatus::Warn, "{env:?}");
    assert!(!env.message.contains(&format!("rpc-value-{id}")), "{env:?}");
    d.shutdown().await.unwrap();
}

fn minio_tap(deprecated_mysql: bool) -> Value {
    let mysql = json!({
        "name": "mysql@8.0", "full_name": "mysql@8.0", "tap": "homebrew/core", "versions": {"stable": "8.0.46"},
        "deprecated": deprecated_mysql, "disable_date": "2027-04-30"
    });
    let minio = json!({
        "name": "minio", "full_name": "minio/stable/minio", "tap": "minio/stable", "versions": {"stable": "2025.9.7"}
    });
    json!([mysql, formula("redis", "8.0.1"), minio])
}

fn info_calls(e: &Env) -> Vec<String> {
    e.brew_log().lines().filter(|l| l.starts_with("info --json=v2")).map(String::from).collect()
}

#[tokio::test]
async fn the_one_brew_info_call_leaves_out_formulae_brew_would_refuse() {
    let e = env().await;
    e.instance("mysql-8.0", "mysql@8.0", false);
    e.data("mysql-8.0");
    e.instance("redis", "redis", false);
    e.instance("minio", "minio/stable/minio", false);
    e.formulae(minio_tap(true));
    let units = || vec![running("mysql-8.0"), running("redis"), running("minio")];

    // minio/stable is not trusted: brew would refuse the whole call, so it is not asked for.
    e.trust(&["shivammathur/php"], &[]);
    let report = e.with(units(), &[]).await;
    let c = only(&report, "service-deprecated:mysql-8.0");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(!report.checks.iter().any(|c| c.id == "service-deprecated"), "no failure warn: {report:#?}");
    assert_eq!(info_calls(&e), ["info --json=v2 mysql@8.0 redis"], "one call, naming no minio");
    assert_eq!(only(&report, "sources:trust").hint.as_deref(), Some("yard source trust minio/stable/minio"));

    // Trusted but not tapped: asking could make brew tap it, which a read-only doctor must not.
    std::fs::remove_file(e.fx.brew.path().join("fake-brew.log")).unwrap();
    e.trust(&["shivammathur/php", "minio/stable"], &[]);
    let report = e.with(units(), &[]).await;
    assert_eq!(info_calls(&e), ["info --json=v2 mysql@8.0 redis"], "an untapped tap is not asked about");
    assert_eq!(only(&report, "service-deprecated:mysql-8.0").status, CheckStatus::Warn);

    // Trusted and tapped: asked about with the others, still one call.
    std::fs::remove_file(e.fx.brew.path().join("fake-brew.log")).unwrap();
    std::fs::create_dir_all(e.fx.brew.path().join("Library/Taps/minio/homebrew-stable")).unwrap();
    let report = e.with(units(), &[]).await;
    assert_eq!(info_calls(&e), ["info --json=v2 minio/stable/minio mysql@8.0 redis"]);
    assert_eq!(only(&report, "service-deprecated:mysql-8.0").status, CheckStatus::Warn);
    not_present(&report, "sources:trust");
}

#[tokio::test]
async fn trust_is_read_once_per_doctor_run() {
    let e = env().await;
    e.instance("redis", "redis", false);
    e.formulae(json!([formula("redis", "8.0.1")]));

    e.with(vec![running("redis")], &[]).await;

    let log = e.brew_log();
    let count = |call: &str| log.lines().filter(|l| *l == call).count();
    assert_eq!((count("--version"), count("trust --json=v1")), (1, 1), "{log}");
}

impl Env {
    /// A linked project holding `files` (name, text).
    fn node_site(&self, name: &str, files: &[(&str, &str)], dir: &tempfile::TempDir) -> std::path::PathBuf {
        let project = dir.path().join(name);
        std::fs::create_dir_all(&project).unwrap();
        for (file, text) in files {
            std::fs::write(project.join(file), text).unwrap();
        }
        let mut sites = std::fs::read_to_string(self.fx.paths.sites_toml()).unwrap_or_default();
        sites.push_str(&format!("[sites.{name}]\npath = {:?}\n", project.display().to_string()));
        std::fs::write(self.fx.paths.sites_toml(), sites).unwrap();
        project
    }
}

fn node_ids(report: &DoctorReport) -> Vec<&str> {
    report.checks.iter().map(|c| c.id.as_str()).filter(|id| id.starts_with("node")).collect()
}

#[tokio::test]
async fn a_node_site_missing_its_version_warns_with_the_install() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.node_site("maker", &[(".nvmrc", "16.14.0\n")], &dir);
    let report = e.offline(&e.helper).await;
    let c = only(&report, "node:maker");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message.starts_with("maker needs Node 16.14.0 (") && c.message.ends_with("which is not installed"),
        "{c:?}"
    );
    assert_eq!(c.hint.as_deref(), Some("yard node install 16.14.0"));
    e.fx.add_node("16.14.0");
    assert!(node_ids(&e.offline(&e.helper).await).is_empty());
}

#[tokio::test]
async fn a_php_only_site_gets_no_node_check() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.node_site("plain", &[], &dir);
    assert!(node_ids(&e.offline(&e.helper).await).is_empty());
}

#[tokio::test]
async fn an_unusable_nvmrc_is_node_spec() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.node_site("maker", &[(".nvmrc", "latest\n")], &dir);
    let report = e.offline(&e.helper).await;
    let c = only(&report, "node-spec:maker");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains(".nvmrc") && c.message.contains("latest"), "{c:?}");
    assert!(c.hint.as_deref().unwrap_or_default().starts_with("put a version such as 22 in"), "{c:?}");
}

#[tokio::test]
async fn a_missing_default_used_by_a_site_warns_once() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    e.node_site("one", &[("package.json", "{}")], &dir);
    e.node_site("two", &[("package.json", "{}")], &dir);
    let report = e.offline(&e.helper).await;
    let c = only(&report, "node-default");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(c.message, "the default Node (22) is not installed, and one, two use it");
    assert_eq!(c.hint.as_deref(), Some("yard node install 22"));
    assert_eq!(only(&report, "node:one").message, "one needs Node 22 (the default), which is not installed");
    only(&report, "node:two");
}
