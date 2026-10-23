//! Doctor against a temp data root, a fake brew and an in-process helper.

mod support;

use std::sync::Arc;
use std::time::Duration;
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
    let free = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
    check.abort();
    assert!(free, "security kept the listener on {port}");
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
    let bad = e.yard_site("bad", "node = \"20\"\n", &dir);
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
