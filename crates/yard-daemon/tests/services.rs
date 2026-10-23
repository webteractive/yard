//! Reconcile runs the service instances `services.toml` lists (M4b spec §4, §5, §6, §15),
//! with `yard-fake-unit` standing in for mysqld, redis-server and minio.

mod support;

use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::daemon::{Client, prepared, run_job};
use support::{Fixture, fast, free_port, served, spawn_helper, wait_until};
use yard_core::brew::Brew;
use yard_daemon::events::EventBus;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::services::service_info;
use yard_daemon::supervisor::Supervisor;
use yard_proto::{ServiceState, UnitState};

/// A `ports.toml` line pinning `unit` to `port`.
fn pin(unit: &str, port: u16) -> String {
    format!("\"{unit}\" = {{ port = {port}, mode = \"pinned\" }}\n")
}

async fn unit(c: &mut Client, name: &str) -> Option<Value> {
    let status = c.call("status", Value::Null).await.unwrap();
    status["units"].as_array().unwrap().iter().find(|u| u["name"] == name).cloned()
}

async fn state_is(c: &mut Client, name: &str, state: &str) -> bool {
    unit(c, name).await.is_some_and(|u| u["state"] == state)
}

/// Polls until `name` is in `state`; the unit's last seen state when it never is.
async fn reaches(c: &mut Client, name: &str, state: &str) -> Result<Value, Option<Value>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let u = unit(c, name).await;
        if u.as_ref().is_some_and(|u| u["state"] == state) {
            return Ok(u.unwrap_or_default());
        }
        if Instant::now() >= deadline {
            return Err(u);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn site(c: &mut Client, name: &str) -> Value {
    let list = c.call("sites.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap_or_else(|| panic!("{list}"))
}

/// Polls `sites.list` until `name`'s `needs` is `want` (`Null` for none).
async fn needs_become(c: &mut Client, name: &str, want: Value) -> Value {
    let mut needs = Value::Null;
    for _ in 0..150 {
        needs = site(c, name).await["needs"].clone();
        if needs == want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    needs
}

/// Reconciles through a job that installs nothing: PHP 8.4 is already there.
async fn reconcile(c: &mut Client) {
    let (lines, error) = run_job(c, "php.install", json!({"php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
}

fn write_yard(dir: &Path, text: &str) {
    std::fs::write(dir.join(".yard"), text).unwrap();
}

#[tokio::test]
async fn instance_runs_with_its_config() {
    let port = free_port();
    let d = prepared(&pin("mysql-8.4", port)).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    let d = d.start().await;
    let mut c = d.client().await;

    let u = reaches(&mut c, "mysql-8.4", "running").await.expect("mysql-8.4 runs");
    assert!(served(port), "it listens on its pinned port");
    let conf = std::fs::read_to_string(d.fx.paths.service_conf("mysql-8.4", "my.cnf")).unwrap();
    assert!(conf.contains("bind-address = 127.0.0.1"), "{conf}");
    assert!(conf.contains(&format!("port = {port}")), "{conf}");
    let overrides = std::fs::read_to_string(d.fx.paths.service_overrides("mysql-8.4")).unwrap();
    assert_eq!(overrides, "", "the user's file is created empty");
    let pid_file = std::fs::read_to_string(d.fx.paths.service_pid("mysql-8.4")).unwrap();
    assert_eq!(pid_file.trim(), u["pid"].to_string(), "the unit records its pid for a later yardd");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn stopped_instance_stays_stopped() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;

    reconcile(&mut c).await;
    reconcile(&mut c).await;
    assert!(unit(&mut c, "redis").await.is_none(), "no unit for an instance the user stopped");
    assert!(state_is(&mut c, "nginx", "running").await, "everything else runs");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn broken_override_fails_only_that_instance() {
    let (mysql, redis) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    d.fx.add_service("redis", "redis", false);
    d.project("blog");
    // The user's file exists before Yard first creates it, and mysqld rejects a line.
    let overrides = d.fx.paths.service_overrides("mysql-8.4");
    std::fs::create_dir_all(overrides.parent().unwrap()).unwrap();
    std::fs::write(&overrides, "bad\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let failed = reaches(&mut c, "mysql-8.4", "failed").await.expect("mysql-8.4 fails");
    let lines = failed["last_lines"].to_string();
    assert!(lines.contains("unknown variable"), "its last lines say why: {lines}");
    assert!(reaches(&mut c, "redis", "running").await.is_ok(), "redis keeps running");
    assert!(reaches(&mut c, "php-fpm-8.4", "running").await.is_ok(), "the linked site keeps running");
    assert!(served(redis));
    assert_eq!(std::fs::read_to_string(&overrides).unwrap(), "bad\n", "Yard never rewrites the user's file");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_instance_is_a_site_need() {
    let d = prepared("").await;
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let want = json!([{"kind": "service", "engine": "mysql", "version": "8.4"}]);
    assert_eq!(site(&mut c, "blog").await["needs"], want);
    let page = std::fs::read_to_string(d.fx.paths.nginx_setup_page("blog")).unwrap();
    assert!(page.contains("blog needs MySQL 8.4."), "{page}");
    let conf = std::fs::read_to_string(d.fx.paths.nginx_site("blog")).unwrap();
    assert!(conf.contains("return 503;"), "{conf}");

    // `yard service add` would do this; the site is served once the instance exists.
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    assert_eq!(needs_become(&mut c, "blog", Value::Null).await, Value::Null);
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok(), "reconcile allocated its port and ran it");
    assert!(!d.fx.paths.nginx_setup_page("blog").exists(), "the setup page is gone");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn stopped_instance_is_not_a_site_need() {
    let d = prepared(&pin("mysql-8.4", free_port())).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", true);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;

    assert_eq!(site(&mut c, "blog").await["needs"], Value::Null, "a stopped instance still meets the need");
    assert!(!d.fx.paths.nginx_setup_page("blog").exists());
    assert!(unit(&mut c, "mysql-8.4").await.is_none(), "and Yard does not start it");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn minio_gets_its_host() {
    let (api, console) = (free_port(), free_port());
    let d = prepared(&(pin("minio", api) + &pin("minio-console", console))).await;
    d.fx.add_formula("minio");
    d.fx.add_service("minio", "minio/stable/minio", false);
    let d = d.start().await;
    let mut c = d.client().await;
    let conf = d.fx.paths.nginx_service("minio");

    assert!(reaches(&mut c, "minio", "running").await.is_ok());
    assert!(served(api) && served(console), "one unit listens on both ports");
    let text = std::fs::read_to_string(&conf).unwrap();
    assert!(text.contains("server_name minio.yard;"), "{text}");
    assert!(text.contains(&format!("http://127.0.0.1:{console}")), "it proxies the console: {text}");
    assert!(
        text.contains(&d.fx.paths.service_overrides("minio").display().to_string()),
        "it ends with the instance's overrides: {text}"
    );

    // A site named `minio` takes the host.
    d.project("minio");
    assert!(wait_until(|| !conf.exists(), Duration::from_secs(5)).await, "the site suppresses minio.yard");
    assert!(d.fx.paths.nginx_site("minio").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn minio_is_https_under_secure_by_default() {
    let (api, console) = (free_port(), free_port());
    let d = prepared(&(pin("minio", api) + &pin("minio-console", console))).await;
    std::fs::write(d.fx.paths.yard_toml(), "secure_by_default = true\n").unwrap();
    d.fx.add_formula("minio");
    d.fx.add_service("minio", "minio/stable/minio", false);
    let d = d.start().await;
    let mut c = d.client().await;

    assert!(reaches(&mut c, "minio", "running").await.is_ok());
    let certs = d.fx.paths.services_certs_dir();
    assert!(certs.join("minio.pem").exists() && certs.join("minio.key").exists(), "a certificate for it");
    let text = std::fs::read_to_string(d.fx.paths.nginx_service("minio")).unwrap();
    assert!(text.contains("ssl_certificate"), "{text}");
    assert!(text.contains(&certs.join("minio.pem").display().to_string()), "{text}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_services_toml_keeps_serving() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    assert!(reaches(&mut c, "redis", "running").await.is_ok());

    let toml = d.fx.paths.services_toml();
    std::fs::write(&toml, "[services.redis]\nengine = \"redis\"\nformula = \"redis\"\nbogus = 1\n").unwrap();
    let mut code = Value::Null;
    for _ in 0..100 {
        code = c.call("status", Value::Null).await.unwrap()["config_error"]["code"].clone();
        if code == "CONFIG_INVALID" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(code, "CONFIG_INVALID");
    assert!(state_is(&mut c, "redis", "running").await, "the last good snapshot keeps serving");
    assert!(served(port));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn services_toml_edit_applies() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    let d = d.start().await;
    let mut c = d.client().await;
    assert!(unit(&mut c, "redis").await.is_none());

    d.fx.add_service("redis", "redis", false);
    let started = Instant::now();
    assert!(reaches(&mut c, "redis", "running").await.is_ok());
    assert!(started.elapsed() < Duration::from_secs(3), "the watcher applied it in {:?}", started.elapsed());
    assert!(served(port));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_keg_is_a_warning_not_a_unit() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;

    reconcile(&mut c).await;
    assert!(unit(&mut c, "redis").await.is_none());
    assert!(state_is(&mut c, "nginx", "running").await, "a missing keg stops nothing else");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn orphaned_instance_is_stopped_before_the_first_reconcile() {
    // A yardd that was killed left redis running, holding its port.
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "\"$0\" listen-tcp \"$1\" >/dev/null 2>&1 & echo $!", support::FAKE, &port.to_string()])
        .output()
        .unwrap();
    let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert!(wait_until(|| support::ppid(pid) == 1 && served(port), Duration::from_secs(5)).await);
    std::fs::write(d.fx.paths.service_pid("redis"), format!("{pid}\n")).unwrap();

    let d = d.start().await;
    let mut c = d.client().await;
    let u = reaches(&mut c, "redis", "running").await.expect("redis runs on the port the orphan held");
    assert_ne!(u["pid"], json!(pid), "a new process, not the orphan");
    // SAFETY: signal 0 only checks that the pid exists.
    assert!(wait_until(|| unsafe { libc::kill(pid as i32, 0) } != 0, Duration::from_secs(5)).await);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn service_info_reports_state_ports_console_and_users() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;
    let (http, https) = (free_port(), free_port());
    let (mysql, redis, api, console) = (free_port(), free_port(), free_port(), free_port());
    std::fs::write(
        fx.paths.ports_toml(),
        format!(
            "[ports]\nnginx-http = {{ port = {http}, mode = \"pinned\" }}\nnginx-https = {{ port = {https}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n{}{}{}{}",
            free_port(),
            pin("mysql-8.4", mysql),
            pin("redis", redis),
            pin("minio", api),
            pin("minio-console", console),
        ),
    )
    .unwrap();
    for formula in ["redis", "minio"] {
        fx.add_formula(formula);
    }
    fx.add_formula("mysql@8.4");
    fx.add_service("mysql-8.4", "mysql@8.4", true);
    fx.add_service("redis", "redis", false);
    fx.add_service("minio", "minio/stable/minio", false);
    let projects = tempfile::tempdir().unwrap();
    for (name, yard) in
        [("blog", "[services]\nmysql = \"8.4\"\nredis = true\n"), ("shop", "[services]\nredis = true\n")]
    {
        let dir = projects.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        write_yard(&dir, yard);
        let mut file = yard_core::config::TomlFile::load(&fx.paths.sites_toml()).unwrap();
        file.set(&["sites", name, "path"], toml_edit::value(dir.display().to_string()));
        file.save().unwrap();
    }
    let bus = EventBus::new();
    let sockets = Arc::new(PrivilegedSockets::new(helper.clone()));
    let sup = Arc::new(Supervisor::new(fast(), fx.paths.logs_dir(), bus.clone(), sockets));
    let rec = Reconciler::new(
        fx.paths.clone(),
        Brew::new(fx.brew.path()),
        sup.clone(),
        helper,
        bus,
        resolver.path().to_path_buf(),
    )
    .unwrap()
    .with_services(None, vec![]);
    let report = rec.reconcile().await.unwrap();
    // The entries are pinned, so nothing is allocated and nothing binds an engine's usual port.
    assert!(report.warnings.iter().all(|w| !w.contains("is missing")), "{:?}", report.warnings);
    let snapshot = rec.snapshot().await.unwrap();

    let mysql_info = service_info(&snapshot, &sup, "mysql-8.4").unwrap();
    assert_eq!(mysql_info.state, ServiceState::Stopped);
    assert!(mysql_info.stopped_by_user);
    assert_eq!(mysql_info.version.as_deref(), Some("8.4"));
    assert_eq!(mysql_info.formula, "mysql@8.4");
    assert_eq!(mysql_info.ports.iter().map(|p| p.port).collect::<Vec<_>>(), [mysql]);
    assert_eq!(mysql_info.account.user.as_deref(), Some("root"));
    assert_eq!(mysql_info.console, None);
    assert_eq!(mysql_info.used_by, ["blog"]);

    let redis_info = service_info(&snapshot, &sup, "redis").unwrap();
    assert!(!redis_info.stopped_by_user);
    assert_eq!(redis_info.used_by, ["blog", "shop"], "`true` means the one instance there is");
    let started = Instant::now();
    while !matches!(service_info(&snapshot, &sup, "redis").unwrap().state, ServiceState::Running) {
        assert!(started.elapsed() < Duration::from_secs(10), "redis runs");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let minio_info = service_info(&snapshot, &sup, "minio").unwrap();
    assert_eq!(
        minio_info.ports.iter().map(|p| (p.name.as_str(), p.port)).collect::<Vec<_>>(),
        [("minio", api), ("minio-console", console)]
    );
    assert_eq!(
        minio_info.console,
        Some(format!("http://minio.yard:{http}")),
        "nginx's port, not the console's"
    );
    assert_eq!(minio_info.account.password.as_deref(), Some("minioadmin"));
    assert!(service_info(&snapshot, &sup, "postgres-17").is_none());

    // Under secure_by_default the console is https, on nginx's https port.
    std::fs::write(fx.paths.yard_toml(), "secure_by_default = true\n").unwrap();
    rec.reconcile().await.unwrap();
    let snapshot = rec.snapshot().await.unwrap();
    let minio_info = service_info(&snapshot, &sup, "minio").unwrap();
    assert_eq!(minio_info.console, Some(format!("https://minio.yard:{https}")));

    // A site named minio takes the host, so there is no console to point at.
    let dir = projects.path().join("minio");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = yard_core::config::TomlFile::load(&fx.paths.sites_toml()).unwrap();
    file.set(&["sites", "minio", "path"], toml_edit::value(dir.display().to_string()));
    file.save().unwrap();
    rec.reconcile().await.unwrap();
    let snapshot = rec.snapshot().await.unwrap();
    assert_eq!(service_info(&snapshot, &sup, "minio").unwrap().console, None);
    sup.shutdown().await;
    assert!(sup.list().iter().all(|u| u.state == UnitState::Stopped));
}
