//! Reconcile runs the service instances `services.toml` lists (M4b spec §4, §5, §6, §15),
//! with `yard-fake-unit` standing in for mysqld, redis-server and minio.

mod support;

use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::daemon::{Client, Daemon, prepared, run_job};
use support::{Fixture, fast, free_port, held_listener, served, spawn_helper, wait_until};
use yard_core::brew::Brew;
use yard_core::config::load_services;
use yard_core::ports::Registry;
use yard_daemon::events::EventBus;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::services::service_info;
use yard_daemon::supervisor::Supervisor;
use yard_proto::{ErrorCode, PortMode, ServiceState, UnitState};

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
    assert!(overrides.starts_with("# USER-OWNED"), "the user's file says whose it is: {overrides}");
    assert!(overrides.ends_with("\n[mysqld]\n"), "and has the group a setting needs: {overrides}");
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
    // Whatever were left unpinned would start at a free port, never at 3306, 6379 or 9000.
    .with_services(
        Some(
            ["mysql", "postgres", "redis", "minio", "minio-console"]
                .into_iter()
                .map(|key| (key.to_string(), free_port()))
                .collect(),
        ),
        vec![],
    );
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

#[tokio::test]
async fn redis_gets_its_data_directory_and_the_databases_do_not() {
    // Real redis-server exits at config load when `dir` is missing; the databases' own init
    // wants an empty or absent data directory, and MinIO makes its own.
    let (redis, mysql) = (free_port(), free_port());
    let d = prepared(&(pin("redis", redis) + &pin("mysql-8.4", mysql))).await;
    d.fx.add_formula("redis");
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("redis", "redis", false);
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    let d = d.start().await;
    let mut c = d.client().await;

    assert!(reaches(&mut c, "redis", "running").await.is_ok(), "redis runs only with its dir");
    assert!(d.fx.paths.service_data("redis").is_dir());
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    assert!(!d.fx.paths.service_data("mysql-8.4").exists(), "mysqld --initialize wants it absent");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_database_gets_longer_than_the_start_grace_to_open_its_port() {
    // mysqld opens its port only after crash recovery or a data upgrade: a SIGTERM at the
    // supervisor's start grace (2 s in tests, 10 s in yardd) would cut that off.
    let port = free_port();
    let d = prepared(&pin("mysql-8.4", port)).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    let grace = d.opts.supervisor.start_grace;
    std::fs::write(d.fx.brew.path().join("fake-listen-delay"), (grace.as_millis() + 1500).to_string())
        .unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let u = reaches(&mut c, "mysql-8.4", "running").await.expect("mysql-8.4 runs once its port opens");
    assert_eq!(u["restarts"], json!(0), "it was never stopped for being slow: {u}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn each_engine_gets_the_user_file_it_reads() {
    // PostgreSQL reads `postgresql.conf` in its data directory, so an `overrides.conf` would be
    // a file nothing includes. Redis's include takes a bare file.
    let (redis, postgres) = (free_port(), free_port());
    let d = prepared(&(pin("redis", redis) + &pin("postgres-17", postgres))).await;
    d.fx.add_formula("redis");
    d.fx.add_formula("postgresql@17");
    d.fx.add_service("redis", "redis", false);
    d.fx.add_service("postgres-17", "postgresql@17", false);
    let d = d.start().await;
    let mut c = d.client().await;

    assert!(reaches(&mut c, "postgres-17", "running").await.is_ok());
    assert!(reaches(&mut c, "redis", "running").await.is_ok());
    assert!(!d.fx.paths.service_overrides("postgres-17").exists(), "no overrides.conf for PostgreSQL");
    assert_eq!(std::fs::read_to_string(d.fx.paths.service_overrides("redis")).unwrap(), "");
    d.shutdown().await.unwrap();
}

async fn pid_of(c: &mut Client, name: &str) -> u64 {
    unit(c, name).await.and_then(|u| u["pid"].as_u64()).unwrap_or_else(|| panic!("{name} has a pid"))
}

#[tokio::test]
async fn a_moved_port_restarts_only_its_instance() {
    let (mysql, moved, redis) = (free_port(), free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "mysql-8.4", "running").await.expect("mysql-8.4 runs");
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    let (mysql_pid, redis_pid) = (pid_of(&mut c, "mysql-8.4").await, pid_of(&mut c, "redis").await);

    // `yard service port` edits ports.toml like this; the rendered my.cnf changes with it.
    let ports = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    let was = format!("\"mysql-8.4\" = {{ port = {mysql},");
    assert!(ports.contains(&was), "{ports}");
    std::fs::write(
        d.fx.paths.ports_toml(),
        ports.replace(&was, &format!("\"mysql-8.4\" = {{ port = {moved},")),
    )
    .unwrap();

    assert!(wait_until(|| served(moved), Duration::from_secs(10)).await, "mysql-8.4 listens on the new port");
    let conf = std::fs::read_to_string(d.fx.paths.service_conf("mysql-8.4", "my.cnf")).unwrap();
    assert!(conf.contains(&format!("port = {moved}")), "{conf}");
    assert_ne!(pid_of(&mut c, "mysql-8.4").await, mysql_pid, "the instance was restarted");
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    assert_eq!(pid_of(&mut c, "redis").await, redis_pid, "another instance is left alone");
    assert!(served(redis));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_deleted_port_entry_is_pinned_again() {
    let (mysql, redis) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    let usual = d.opts.usual_ports.as_ref().unwrap()["mysql"];
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "mysql-8.4", "running").await.expect("mysql-8.4 runs");

    let ports = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    let kept: String =
        ports.lines().filter(|l| !l.starts_with("\"mysql-8.4\"")).map(|l| format!("{l}\n")).collect();
    assert_ne!(kept, ports);
    std::fs::write(d.fx.paths.ports_toml(), kept).unwrap();

    // Reconcile pinned it from the engine's usual port (a free port in tests) and saved it.
    let pinned = || {
        Registry::load(&d.fx.paths)
            .ok()
            .and_then(|r| r.entries.get("mysql-8.4").copied())
            .is_some_and(|e| e.port == usual && e.mode == PortMode::Pinned)
    };
    assert!(wait_until(pinned, Duration::from_secs(10)).await, "{:?}", Registry::load(&d.fx.paths));
    assert!(wait_until(|| served(usual), Duration::from_secs(10)).await, "mysql-8.4 runs on the new pin");
    d.shutdown().await.unwrap();
}

// `services.add`, `services.remove` and `services.list` (M4b spec §4 to §8, §12).

/// One `brew info --json=v2` formula object, as the fake brew's `fake-formulae.json` holds it.
fn formula(name: &str, full_name: &str, tap: &str, version: &str) -> Value {
    json!({
        "name": name, "full_name": full_name, "tap": tap, "aliases": [],
        "versions": {"stable": version}, "deprecated": false, "disabled": false,
        "deprecation_date": null, "disable_date": null,
    })
}

fn core(name: &str, version: &str) -> Value {
    formula(name, name, "homebrew/core", version)
}

/// `entry` deprecated, and brew disabling it on `date`.
fn deprecated(mut entry: Value, date: &str) -> Value {
    entry["deprecated"] = json!(true);
    entry["deprecation_date"] = json!("2026-04-30");
    entry["disable_date"] = json!(date);
    entry
}

fn write_formulae(d: &Daemon, entries: &[Value]) {
    std::fs::write(d.fx.brew.path().join("fake-formulae.json"), json!(entries).to_string()).unwrap();
}

fn brew_log(d: &Daemon) -> String {
    std::fs::read_to_string(d.fx.brew.path().join("fake-brew.log")).unwrap_or_default()
}

/// The `usual_ports` override the daemon was given for `key`.
fn usual(d: &Daemon, key: &str) -> u16 {
    d.opts.usual_ports.as_ref().unwrap()[key]
}

async fn add(
    c: &mut Client,
    engine: &str,
    version: Option<&str>,
) -> (Vec<String>, Option<yard_proto::YardError>) {
    let mut params = json!({"engine": engine});
    if let Some(v) = version {
        params["version"] = json!(v);
    }
    run_job(c, "services.add", params).await
}

async fn list(c: &mut Client) -> Vec<Value> {
    c.call("services.list", Value::Null).await.unwrap().as_array().unwrap().clone()
}

fn port_of(d: &Daemon, unit: &str) -> Option<(u16, PortMode)> {
    Registry::load(&d.fx.paths).unwrap().entries.get(unit).map(|e| (e.port, e.mode))
}

fn has_entry(d: &Daemon, instance: &str) -> bool {
    load_services(&d.fx.paths).unwrap().services.contains_key(instance)
}

/// Nothing of `instance` is left: no directory, no entry, no port entry.
fn assert_gone(d: &Daemon, instance: &str) {
    assert!(!d.fx.paths.service_dir(instance).exists(), "services/{instance} is gone");
    assert!(!has_entry(d, instance), "no services.toml entry");
    let ports = Registry::load(&d.fx.paths).unwrap();
    let left: Vec<&String> = ports.entries.keys().filter(|k| k.starts_with(instance)).collect();
    assert!(left.is_empty(), "no port entries: {left:?}");
}

#[tokio::test]
async fn add_installs_initializes_and_runs() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    let port = usual(&d, "mysql");
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("brew install mysql@8.4")), "{lines:?}");
    assert!(
        d.fx.paths.service_data("mysql-8.4").join("ibdata1").exists(),
        "the data directory is initialized"
    );
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    assert!(served(port), "it runs on the usual port");
    assert_eq!(port_of(&d, "mysql-8.4"), Some((port, PortMode::Pinned)), "pinned, so nothing moves it");
    let entry = load_services(&d.fx.paths).unwrap().services["mysql-8.4"].clone();
    assert_eq!((entry.version.as_deref(), entry.formula.as_str()), (Some("8.4"), "mysql@8.4"));

    let rows = list(&mut c).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "mysql-8.4");
    assert_eq!(rows[0]["account"]["user"], "root");
    assert_eq!(rows[0]["state"], "running");
    assert_eq!(rows[0]["ports"], json!([{"name": "mysql-8.4", "port": port}]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn list_is_empty_without_instances() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    assert!(list(&mut c).await.is_empty());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn two_mysql_lines_run_side_by_side() {
    let d = prepared("").await;
    write_formulae(&d, &[deprecated(core("mysql@8.0", "8.0.46"), "2027-04-30"), core("mysql@8.4", "8.4.13")]);
    let usual = usual(&d, "mysql");
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "mysql", Some("8.0")).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("mysql@8.0") && l.contains("deprecated") && l.contains("2027-04-30")),
        "a deprecated formula names brew's disable date: {lines:?}"
    );
    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("deprecated")), "{lines:?}");

    let (old, new) = (port_of(&d, "mysql-8.0").unwrap(), port_of(&d, "mysql-8.4").unwrap());
    assert_eq!(old, (usual, PortMode::Pinned));
    assert_eq!(new.1, PortMode::Pinned);
    assert!(new.0 > usual, "the usual port is taken, so the next free one: {new:?}");
    assert!(reaches(&mut c, "mysql-8.0", "running").await.is_ok());
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    assert!(served(old.0) && served(new.0));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_respects_reserved_ports() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    // DBngin keeps MySQL's usual port, which here is a free port, never 3306.
    std::fs::create_dir_all(d.opts.app_dirs[0].join("DBngin.app")).unwrap();
    let usual = usual(&d, "mysql");
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    let (port, mode) = port_of(&d, "mysql-8.4").unwrap();
    assert!(port > usual, "the reserved usual port {usual} is skipped: got {port}");
    assert_eq!(mode, PortMode::Pinned);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_unknown_line_lists_lines() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.0", "8.0.46"), core("mysql@8.4", "8.4.13")]);
    let d = d.start().await;
    let mut c = d.client().await;

    let e = c.call("services.add", json!({"engine": "mysql", "version": "9.9"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceVersionUnsupported);
    assert!(e.message.contains("8.0, 8.4"), "{}", e.message);
    assert!(!brew_log(&d).contains("install"), "nothing was installed");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_disabled_formula_is_not_offered() {
    let d = prepared("").await;
    let mut old = core("postgresql@13", "13.9");
    old["disabled"] = json!(true);
    old["disable_date"] = json!("2026-11-13");
    write_formulae(&d, &[old, core("postgresql@17", "17.2")]);
    let d = d.start().await;
    let mut c = d.client().await;

    let e = c.call("services.add", json!({"engine": "postgres", "version": "13"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceVersionUnsupported);
    assert!(e.message.contains("it offers 17") && !e.message.contains("13, "), "{}", e.message);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_formula_brew_cannot_describe_is_its_own_error() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    // brew lists the formula but fails on `brew info mysql@8.4`.
    std::fs::write(d.fx.brew.path().join("fake-brew-fail"), "mysql@8.4").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let e = c.call("services.add", json!({"engine": "mysql", "version": "8.4"})).await.unwrap_err();
    assert_ne!(e.code, ErrorCode::ServiceVersionUnsupported, "{e:?}");
    assert!(e.message.contains("could not describe mysql@8.4"), "{}", e.message);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_checks_its_arguments() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    for params in [
        json!({"engine": "mariadb"}),
        json!({"engine": "redis", "version": "8"}),
        json!({"engine": "minio", "version": "1"}),
        json!({"engine": "mysql", "version": "8"}),
        json!({"engine": "mysql", "version": "latest"}),
        json!({"engine": "postgres", "version": "17.2"}),
        json!({"engine": "redis", "port": 0}),
    ] {
        let e = c.call("services.add", params.clone()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument, "{params}: {e:?}");
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_twice_is_service_exists() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    let e = c.call("services.add", json!({"engine": "mysql", "version": "8.4"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceExists);
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok(), "the instance is left alone");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn two_adds_at_once_write_one_entry() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    // The first job sits in `brew install`, so the second passes the handler's check too.
    std::fs::write(d.fx.brew.path().join("fake-brew-delay"), "800").unwrap();
    let port = usual(&d, "mysql");
    let d = d.start().await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["jobs"]})).await.unwrap();
    let params = json!({"engine": "mysql", "version": "8.4"});
    let first = c.call("services.add", params.clone()).await.unwrap()["job_id"].as_str().unwrap().to_string();
    let second = c.call("services.add", params).await.unwrap()["job_id"].as_str().unwrap().to_string();

    let mut finished = std::collections::BTreeMap::new();
    while finished.len() < 2 {
        let e = c
            .event_where(Duration::from_secs(20), |e| e["type"] == "job_finished")
            .await
            .expect("both jobs finish");
        finished.insert(e["job_id"].as_str().unwrap().to_string(), e["error"].clone());
    }
    assert_eq!(finished[&first], Value::Null, "the first add wins");
    assert_eq!(finished[&second]["code"], "SERVICE_EXISTS", "the second finds the entry: {finished:?}");
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok(), "the first instance is untouched");
    assert!(served(port));
    assert_eq!(port_of(&d, "mysql-8.4"), Some((port, PortMode::Pinned)), "and so are its ports");
    assert!(d.fx.paths.service_data("mysql-8.4").join("ibdata1").exists(), "and its data");
    assert_eq!(list(&mut c).await.len(), 1);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_minio_needs_trust() {
    let d = prepared("").await;
    write_formulae(&d, &[formula("minio", "minio/stable/minio", "minio/stable", "2025-09-06")]);
    let d = d.start().await;
    let mut c = d.client().await;

    let e = c.call("services.add", json!({"engine": "minio"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::FormulaUntrusted);
    assert_eq!(e.hint.as_deref(), Some("yard source trust minio/stable/minio"));
    assert!(!brew_log(&d).contains("install"), "brew was never asked to install");
    assert_gone(&d, "minio");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_minio_installs_from_its_tap_once_trusted() {
    let d = prepared("").await;
    write_formulae(&d, &[formula("minio", "minio/stable/minio", "minio/stable", "2025-09-06")]);
    let trusted = r#"{"taps":["minio/stable"],"formulae":[],"casks":[],"commands":[]}"#;
    std::fs::write(d.fx.brew.path().join("fake-trust.json"), trusted).unwrap();
    let (api, console) = (usual(&d, "minio"), usual(&d, "minio-console"));
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "minio", None).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("brew install minio/stable/minio")), "{lines:?}");
    assert_eq!(port_of(&d, "minio"), Some((api, PortMode::Pinned)));
    assert_eq!(port_of(&d, "minio-console"), Some((console, PortMode::Pinned)));
    assert!(served(api) && served(console));
    assert_eq!(load_services(&d.fx.paths).unwrap().services["minio"].formula, "minio/stable/minio");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_uses_an_installed_core_minio() {
    let d = prepared("").await;
    // homebrew/core's minio, already installed, deprecated and due to be disabled.
    d.fx.add_formula("minio");
    write_formulae(&d, &[deprecated(core("minio", "2025.9.7"), "2027-02-17")]);
    let (api, console) = (usual(&d, "minio"), usual(&d, "minio-console"));
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "minio", None).await;
    assert_eq!(error, None, "{lines:?}");
    let log = brew_log(&d);
    assert!(!log.contains("install"), "it is used as it is: {log}");
    assert!(!log.contains("trust"), "no trust check for a core formula: {log}");
    assert!(log.contains("info --json=v2 minio"), "brew said which tap the keg is from: {log}");
    assert!(lines.iter().any(|l| l.contains("2027-02-17")), "brew's disable date: {lines:?}");
    assert_eq!(load_services(&d.fx.paths).unwrap().services["minio"].formula, "minio");
    assert!(served(api) && served(console));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_with_a_port_pins_that_port() {
    let d = prepared("").await;
    write_formulae(&d, &[core("redis", "8.0.1")]);
    let port = free_port();
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = run_job(&mut c, "services.add", json!({"engine": "redis", "port": port})).await;
    assert_eq!(error, None, "{lines:?}");
    assert_eq!(port_of(&d, "redis"), Some((port, PortMode::Pinned)));
    assert!(served(port));
    assert!(d.fx.paths.service_data("redis").is_dir(), "reconcile made redis's data directory");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_busy_port_names_its_holder_and_hints_at_another_port() {
    let d = prepared("").await;
    write_formulae(&d, &[core("redis", "8.0.1")]);
    let held = support::held_listener();
    let port = held.local_addr().unwrap().port();
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = run_job(&mut c, "services.add", json!({"engine": "redis", "port": port})).await;
    let e = error.unwrap_or_else(|| panic!("the busy port refuses the add: {lines:?}"));
    assert_eq!(e.code, ErrorCode::PortInUse);
    assert!(e.message.contains(&port.to_string()) && e.message.contains("is held by"), "{}", e.message);
    let hint = e.hint.unwrap_or_default();
    assert!(hint.contains("--port"), "{hint}");
    assert!(!hint.contains("yard ports set") && !hint.contains("auto_fallback"), "{hint}");
    assert_gone(&d, "redis");
    drop(held);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_add_leaves_nothing_behind() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    std::fs::write(d.fx.brew.path().join("fake-init-delay"), "5000").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["jobs"]})).await.unwrap();
    let started = c.call("services.add", json!({"engine": "mysql", "version": "8.4"})).await.unwrap();
    let id = started["job_id"].as_str().unwrap().to_string();

    let initializing = c
        .event_where(Duration::from_secs(10), |e| {
            e["job_id"] == id.as_str() && e["line"].as_str().is_some_and(|l| l.contains("initializing"))
        })
        .await;
    assert!(initializing.is_some(), "the job says it is initializing");
    c.call("jobs.cancel", json!({"job_id": id})).await.unwrap();
    let done = c
        .event_where(Duration::from_secs(15), |e| e["job_id"] == id.as_str() && e["type"] == "job_finished")
        .await
        .expect("the job finishes");
    assert_eq!(done["error"]["code"], "JOB_CANCELLED");
    assert_gone(&d, "mysql-8.4");

    std::fs::remove_file(d.fx.brew.path().join("fake-init-delay")).unwrap();
    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "a second add succeeds: {lines:?}");
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_init_rolls_back() {
    let d = prepared("").await;
    write_formulae(&d, &[core("postgresql@17", "17.2")]);
    std::fs::write(d.fx.brew.path().join("fake-init-fail"), "").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "postgres", Some("17")).await;
    let e = error.unwrap_or_else(|| panic!("initdb failed, so the add fails: {lines:?}"));
    assert_eq!(e.code, ErrorCode::JobFailed);
    assert_gone(&d, "postgres-17");
    assert!(list(&mut c).await.is_empty());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_instance_that_fails_to_start_rolls_back() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    // The user's overrides file exists first, as for a user who prepared it; mysqld rejects it.
    let overrides = d.fx.paths.service_overrides("mysql-8.4");
    std::fs::create_dir_all(overrides.parent().unwrap()).unwrap();
    std::fs::write(&overrides, "bad\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    let e = error.unwrap_or_else(|| panic!("mysqld exits, so the add fails: {lines:?}"));
    assert_eq!(e.code, ErrorCode::JobFailed);
    assert!(e.message.contains("unknown variable") || e.to_string().contains("unknown variable"), "{e:?}");
    assert!(!has_entry(&d, "mysql-8.4"));
    assert_eq!(port_of(&d, "mysql-8.4"), None);
    assert!(!d.fx.paths.service_data("mysql-8.4").exists(), "the data this add created is gone");
    assert_eq!(std::fs::read_to_string(&overrides).unwrap(), "bad\n", "the user's file stays");
    assert!(!d.fx.paths.service_conf("mysql-8.4", "my.cnf").exists(), "what Yard generated goes");
    assert!(unit(&mut c, "mysql-8.4").await.is_none(), "and the unit is stopped and gone");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn remove_refuses_in_use() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");

    let e = c.call("services.remove", json!({"name": "mysql-8.4"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceInUse);
    assert!(e.message.contains("blog"), "{}", e.message);
    assert_eq!(e.data.as_ref().unwrap()["sites"], json!(["blog"]));
    let file = dir.join(".yard");
    assert_eq!(e.hint, Some(format!("edit {}", file.display())), "{e:?}");
    assert!(has_entry(&d, "mysql-8.4"), "nothing was removed");
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn remove_of_an_unknown_instance_is_not_found() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let e = c.call("services.remove", json!({"name": "mysql-8.4"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceNotFound);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn remove_deletes_data() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13"), core("redis", "8.0.1")]);
    let d = d.start().await;
    let mut c = d.client().await;
    for (engine, version) in [("mysql", Some("8.4")), ("redis", None)] {
        let (lines, error) = add(&mut c, engine, version).await;
        assert_eq!(error, None, "{lines:?}");
    }
    let port = port_of(&d, "mysql-8.4").unwrap().0;
    assert!(served(port));

    c.call("services.remove", json!({"name": "mysql-8.4"})).await.unwrap();
    assert_gone(&d, "mysql-8.4");
    assert!(unit(&mut c, "mysql-8.4").await.is_none(), "the unit is gone");
    assert!(!served(port), "mysqld stopped before its directory went");
    assert!(wait_until(|| list_names(&d).contains(&"redis".to_string()), Duration::from_secs(5)).await);
    assert_eq!(list(&mut c).await.len(), 1, "redis is untouched");
    assert!(reaches(&mut c, "redis", "running").await.is_ok());
    assert!(!brew_log(&d).contains("uninstall"), "the keg is the user's");
    d.shutdown().await.unwrap();
}

fn list_names(d: &Daemon) -> Vec<String> {
    load_services(&d.fx.paths).unwrap().services.keys().cloned().collect()
}

#[tokio::test]
async fn remove_keep_data_then_add_reuses() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    let data = d.fx.paths.service_data("mysql-8.4");
    std::fs::write(data.join("marker"), "kept").unwrap();

    c.call("services.remove", json!({"name": "mysql-8.4", "keep_data": true})).await.unwrap();
    assert!(!has_entry(&d, "mysql-8.4"));
    assert_eq!(port_of(&d, "mysql-8.4"), None);
    assert_eq!(std::fs::read_to_string(data.join("marker")).unwrap(), "kept", "the data stays");
    let kept: Vec<_> = std::fs::read_dir(d.fx.paths.service_dir("mysql-8.4"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kept, ["data"], "and nothing else does");

    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("reusing the data in")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("initializing")), "no second init: {lines:?}");
    assert_eq!(std::fs::read_to_string(data.join("marker")).unwrap(), "kept");
    assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_add_never_deletes_data_it_reused() {
    let d = prepared("").await;
    write_formulae(&d, &[core("mysql@8.4", "8.4.13")]);
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    assert_eq!(error, None, "{lines:?}");
    let data = d.fx.paths.service_data("mysql-8.4");
    std::fs::write(data.join("marker"), "kept").unwrap();
    c.call("services.remove", json!({"name": "mysql-8.4", "keep_data": true})).await.unwrap();

    // The add reuses the data, then mysqld refuses the user's overrides.
    let overrides = d.fx.paths.service_overrides("mysql-8.4");
    std::fs::write(&overrides, "bad\n").unwrap();
    let (lines, error) = add(&mut c, "mysql", Some("8.4")).await;
    let e = error.unwrap_or_else(|| panic!("mysqld exits, so the add fails: {lines:?}"));
    assert_eq!(e.code, ErrorCode::JobFailed);
    assert!(lines.iter().any(|l| l.contains("reusing the data in")), "{lines:?}");
    assert_eq!(std::fs::read_to_string(data.join("marker")).unwrap(), "kept", "the earlier data stays");
    assert!(data.join("ibdata1").exists());
    assert!(!has_entry(&d, "mysql-8.4"));
    assert_eq!(port_of(&d, "mysql-8.4"), None);
    assert_eq!(std::fs::read_to_string(&overrides).unwrap(), "bad\n", "the user's file stays");
    d.shutdown().await.unwrap();
}

// ---- start, stop, restart, logs, port ----

fn services_text(d: &Daemon) -> String {
    std::fs::read_to_string(d.fx.paths.services_toml()).unwrap()
}

/// Every method that takes an instance name refuses an unknown one the same way.
#[tokio::test]
async fn an_unknown_instance_is_not_found_by_every_method() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    for (method, params) in [
        ("services.start", json!({"name": "redis"})),
        ("services.stop", json!({"name": "redis"})),
        ("services.restart", json!({"name": "redis"})),
        ("services.logs", json!({"name": "redis"})),
        ("services.port", json!({"name": "redis", "port": free_port()})),
    ] {
        let e = c.call(method, params).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ServiceNotFound, "{method}: {e:?}");
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn stop_stops_the_unit_and_start_clears_stopped() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    assert!(served(port));

    let info = c.call("services.stop", json!({"name": "redis"})).await.unwrap();
    assert_eq!((info["state"].clone(), info["stopped_by_user"].clone()), (json!("stopped"), json!(true)));
    assert!(services_text(&d).contains("stopped = true"), "{}", services_text(&d));
    assert!(unit(&mut c, "redis").await.is_none(), "reconcile dropped the unit");
    assert!(wait_until(|| !served(port), Duration::from_secs(10)).await, "the port is free again");
    assert!(state_is(&mut c, "nginx", "running").await, "everything else runs");

    let info = c.call("services.start", json!({"name": "redis"})).await.unwrap();
    assert_eq!(info["stopped_by_user"], json!(false));
    assert_ne!(info["state"], json!("stopped"), "a unit that is starting is not reported stopped: {info}");
    // The user's file is clean again: the key is gone, not `stopped = false`.
    assert!(!services_text(&d).contains("stopped"), "{}", services_text(&d));
    reaches(&mut c, "redis", "running").await.expect("redis runs again");
    assert!(served(port));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn start_clears_stopped() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;
    reconcile(&mut c).await;
    assert!(unit(&mut c, "redis").await.is_none());

    c.call("services.start", json!({"name": "redis"})).await.unwrap();
    assert!(!services_text(&d).contains("stopped"), "{}", services_text(&d));
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    assert!(served(port));
    // Starting one that runs changes nothing.
    let info = c.call("services.start", json!({"name": "redis"})).await.unwrap();
    assert_eq!(info["stopped_by_user"], json!(false));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn stop_survives_restart_of_yardd() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let mut d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    c.call("services.stop", json!({"name": "redis"})).await.unwrap();
    drop(c);

    d.stop().await.unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    reconcile(&mut c).await;
    assert!(unit(&mut c, "redis").await.is_none(), "still stopped under a new yardd");
    assert!(!served(port));
    let list = list(&mut c).await;
    assert_eq!(
        (list[0]["state"].clone(), list[0]["stopped_by_user"].clone()),
        (json!("stopped"), json!(true))
    );
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_restarts_a_running_instance() {
    let (mysql, redis) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "mysql-8.4", "running").await.expect("mysql-8.4 runs");
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    let (mysql_pid, redis_pid) = (pid_of(&mut c, "mysql-8.4").await, pid_of(&mut c, "redis").await);

    let info = c.call("services.restart", json!({"name": "mysql-8.4"})).await.unwrap();
    assert_eq!(info["name"], json!("mysql-8.4"));
    assert!(
        wait_until(|| served(mysql), Duration::from_secs(10)).await
            && reaches(&mut c, "mysql-8.4", "running").await.is_ok()
    );
    assert_ne!(pid_of(&mut c, "mysql-8.4").await, mysql_pid, "the instance was restarted");
    assert_eq!(pid_of(&mut c, "redis").await, redis_pid, "another instance is left alone");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_of_a_failed_instance_answers_with_the_new_attempt() {
    let port = free_port();
    let d = prepared(&pin("mysql-8.4", port)).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_service("mysql-8.4", "mysql@8.4", false);
    let overrides = d.fx.paths.service_overrides("mysql-8.4");
    std::fs::create_dir_all(overrides.parent().unwrap()).unwrap();
    std::fs::write(&overrides, "[mysqld]\nbad\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "mysql-8.4", "failed").await.expect("the broken override fails it");

    // The user fixes the file, then restarts: the answer is about this restart, not the failure.
    std::fs::write(&overrides, "[mysqld]\n").unwrap();
    for _ in 0..5 {
        let info = c.call("services.restart", json!({"name": "mysql-8.4"})).await.unwrap();
        assert!(
            info["state"] == json!("starting") || info["state"] == json!("running"),
            "the restart's own state: {info}"
        );
        assert!(reaches(&mut c, "mysql-8.4", "running").await.is_ok());
    }
    assert!(served(port));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_held_port_is_the_reason_a_failed_instance_gives() {
    // Something else listens on the instance's pinned port before it starts: the start fails
    // before the process runs, so its log has nothing to say and the reason must.
    let held = held_listener();
    let port = held.local_addr().unwrap().port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "failed").await.expect("redis cannot have its port");

    let check = |info: &Value| {
        let reason = &info["state"]["failed"]["reason"];
        assert_eq!(reason["code"], json!("PORT_IN_USE"), "{info}");
        let message = reason["message"].as_str().unwrap_or_default();
        assert!(message.starts_with(&format!("port {port} is held by ")), "{message}");
        assert_eq!(reason["hint"], json!("yard service port redis <port>"), "{info}");
        assert_eq!(reason["data"]["port"], json!(port), "{info}");
    };
    check(&list(&mut c).await[0]);
    check(&c.call("services.start", json!({"name": "redis"})).await.unwrap());
    check(&c.call("services.restart", json!({"name": "redis"})).await.unwrap_or_else(|e| panic!("{e:?}")));
    drop(held);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_refuses_a_stopped_instance() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;

    let e = c.call("services.restart", json!({"name": "redis"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument, "{e:?}");
    assert!(e.message.contains("redis") && e.message.contains("stopped"), "{}", e.message);
    assert_eq!(e.hint.as_deref(), Some("yard service start redis"));
    assert!(services_text(&d).contains("stopped = true"), "restart never clears stopped");
    reconcile(&mut c).await;
    assert!(unit(&mut c, "redis").await.is_none(), "and starts nothing");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn port_moves_and_restarts() {
    let (old, new) = (free_port(), free_port());
    let d = prepared(&pin("redis", old)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    let pid = pid_of(&mut c, "redis").await;
    assert!(served(old));

    let info = c.call("services.port", json!({"name": "redis", "port": new})).await.unwrap();
    assert_eq!(info["ports"], json!([{"name": "redis", "port": new}]));
    assert_eq!(port_of(&d, "redis"), Some((new, PortMode::Pinned)));
    assert!(wait_until(|| served(new), Duration::from_secs(10)).await, "redis listens on the new port");
    assert!(wait_until(|| !served(old), Duration::from_secs(10)).await, "and no longer on the old one");
    assert_ne!(pid_of(&mut c, "redis").await, pid, "reconcile restarted it");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn port_to_its_own_port_changes_nothing() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    let pid = pid_of(&mut c, "redis").await;

    let info = c.call("services.port", json!({"name": "redis", "port": port})).await.unwrap();
    assert_eq!(info["ports"], json!([{"name": "redis", "port": port}]));
    assert_eq!(pid_of(&mut c, "redis").await, pid, "it keeps running where it is");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn port_moves_a_stopped_instance_and_it_stays_stopped() {
    let (old, new) = (free_port(), free_port());
    let d = prepared(&pin("redis", old)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;

    let info = c.call("services.port", json!({"name": "redis", "port": new})).await.unwrap();
    assert_eq!((info["state"].clone(), info["stopped_by_user"].clone()), (json!("stopped"), json!(true)));
    assert_eq!(port_of(&d, "redis"), Some((new, PortMode::Pinned)));
    reconcile(&mut c).await;
    assert!(unit(&mut c, "redis").await.is_none(), "it stays stopped");
    assert!(!served(new));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn port_moves_only_minios_api_port() {
    let (api, console, new) = (free_port(), free_port(), free_port());
    let d = prepared(&(pin("minio", api) + &pin("minio-console", console))).await;
    d.fx.add_formula("minio");
    d.fx.add_service("minio", "minio", true);
    let d = d.start().await;
    let mut c = d.client().await;

    let info = c.call("services.port", json!({"name": "minio", "port": new})).await.unwrap();
    assert_eq!(
        info["ports"],
        json!([{"name": "minio", "port": new}, {"name": "minio-console", "port": console}])
    );
    // The console is the other entry's: it is not a port this method moves onto.
    let e = c.call("services.port", json!({"name": "minio", "port": console})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PortInUse);
    assert!(e.message.contains("minio-console"), "{}", e.message);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn port_refuses_busy() {
    let port = free_port();
    let d = prepared(&pin("redis", port)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let held = support::held_listener();
    let busy = held.local_addr().unwrap().port();
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    let pid = pid_of(&mut c, "redis").await;

    let e = c.call("services.port", json!({"name": "redis", "port": busy})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PortInUse);
    assert!(e.message.contains(&busy.to_string()) && e.message.contains("is held by"), "{}", e.message);
    let hint = e.hint.unwrap_or_default();
    assert_eq!(hint, "pass a different port: yard service port redis <port>");
    assert!(!hint.contains("yard ports set") && !hint.contains("auto_fallback"), "{hint}");
    assert_eq!(port_of(&d, "redis"), Some((port, PortMode::Pinned)), "nothing moved");
    assert_eq!(pid_of(&mut c, "redis").await, pid, "and nothing restarted");
    drop(held);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn port_refuses_a_port_another_unit_has() {
    let (redis, mysql) = (free_port(), free_port());
    let d = prepared(&(pin("redis", redis) + &pin("mysql-8.4", mysql))).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;

    let e = c.call("services.port", json!({"name": "redis", "port": mysql})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PortInUse);
    assert!(e.message.contains("already registered to mysql-8.4"), "{}", e.message);
    assert_eq!(e.hint.as_deref(), Some("pass a different port: yard service port redis <port>"));
    assert_eq!(port_of(&d, "redis"), Some((redis, PortMode::Pinned)));
    let e = c.call("services.port", json!({"name": "redis", "port": 0})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    d.shutdown().await.unwrap();
}

/// A stopped instance has no unit, so nothing else writes its log.
fn write_log(d: &Daemon, instance: &str, text: &str) {
    std::fs::create_dir_all(d.fx.paths.logs_dir()).unwrap();
    std::fs::write(d.fx.paths.log(instance), text).unwrap();
}

#[tokio::test]
async fn logs_tail() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    write_log(&d, "redis", "one\ntwo\nthree\nfour\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let last = c.call("services.logs", json!({"name": "redis", "lines": 2})).await.unwrap();
    assert_eq!(last, json!(["three", "four"]));
    let all = c.call("services.logs", json!({"name": "redis"})).await.unwrap();
    assert_eq!(all, json!(["one", "two", "three", "four"]), "the default is 100 lines");
    let none = c.call("services.logs", json!({"name": "redis", "lines": 0})).await.unwrap();
    assert_eq!(none, json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn logs_default_to_the_last_hundred_lines_of_a_large_file() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    // Longer than the tail's read block, with no newline at the end and a line cut in two.
    let text: String = (1..=5000).map(|i| format!("line {i} {}\n", "x".repeat(40))).collect();
    write_log(&d, "redis", text.trim_end());
    let d = d.start().await;
    let mut c = d.client().await;

    let got = c.call("services.logs", json!({"name": "redis"})).await.unwrap();
    let got: Vec<&str> = got.as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
    assert_eq!(got.len(), 100);
    assert!(got[0].starts_with("line 4901 "), "{}", got[0]);
    assert!(got[99].starts_with("line 5000 "), "{}", got[99]);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn logs_of_a_missing_file_is_empty() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let d = d.start().await;
    let mut c = d.client().await;
    assert!(!d.fx.paths.log("redis").exists());

    let got = c.call("services.logs", json!({"name": "redis"})).await.unwrap();
    assert_eq!(got, json!([]));
    d.shutdown().await.unwrap();
}

// ---- fix round 1 ----

/// A remove that lands between the handler's check and the edit must not leave a stub
/// `[services.<i>] stopped = true`, which `services.toml` cannot parse.
#[tokio::test]
async fn a_stop_of_an_entry_that_vanished_writes_no_stub() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", false);
    let d = d.start().await;
    let mut c = d.client().await;
    reaches(&mut c, "redis", "running").await.expect("redis runs");
    // The entry goes behind the handler's back, as a `services.remove` would.
    std::fs::write(d.fx.paths.services_toml(), "# nothing here\n").unwrap();

    for stopped in [true, false] {
        let e = yard_daemon::services::set_stopped(&d.fx.paths, "redis", stopped).unwrap_err();
        assert_eq!(e.code, ErrorCode::ServiceNotFound, "{e:?}");
    }
    assert_eq!(services_text(&d), "# nothing here\n", "nothing was saved");
    assert!(load_services(&d.fx.paths).unwrap().services.is_empty(), "and the file still parses");
    let e = c.call("services.stop", json!({"name": "redis"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceNotFound);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn logs_asking_for_more_than_the_file_has_returns_every_line() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    write_log(&d, "redis", "one\ntwo\nthree\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let got = c.call("services.logs", json!({"name": "redis", "lines": 5000})).await.unwrap();
    assert_eq!(got, json!(["one", "two", "three"]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_very_large_lines_is_clamped_and_answers_promptly() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let text: String = (1..=30_000).map(|i| format!("line {i}\n")).collect();
    write_log(&d, "redis", &text);
    let d = d.start().await;
    let mut c = d.client().await;

    let started = Instant::now();
    let got = c.call("services.logs", json!({"name": "redis", "lines": usize::MAX})).await.unwrap();
    let got = got.as_array().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(got.len(), 10_000, "clamped to the most a call returns");
    assert_eq!(got[0], json!("line 20001"));
    assert_eq!(got[9_999], json!("line 30000"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_log_line_that_is_not_utf8_comes_back_lossy() {
    let d = prepared(&pin("redis", free_port())).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    std::fs::create_dir_all(d.fx.paths.logs_dir()).unwrap();
    std::fs::write(d.fx.paths.log("redis"), b"ok\nbad \xff\xfe bytes\nlast\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let got = c.call("services.logs", json!({"name": "redis", "lines": 3})).await.unwrap();
    assert_eq!(got, json!(["ok", "bad \u{fffd}\u{fffd} bytes", "last"]));
    d.shutdown().await.unwrap();
}

// ---- env.print and sites.info ----

async fn env(c: &mut Client, params: Value) -> Result<Vec<Value>, yard_proto::YardError> {
    Ok(c.call("env.print", params).await?.as_array().unwrap().clone())
}

fn redis_lines(port: u16) -> Value {
    json!(["REDIS_HOST=127.0.0.1", format!("REDIS_PORT={port}"), "REDIS_PASSWORD=null"])
}

fn db_lines(connection: &str, port: u16, user: &str) -> Value {
    json!([
        format!("DB_CONNECTION={connection}"),
        "DB_HOST=127.0.0.1",
        format!("DB_PORT={port}"),
        format!("DB_USERNAME={user}"),
        "DB_PASSWORD=",
    ])
}

#[tokio::test]
async fn env_for_project_lists_its_services() {
    let (mysql, redis) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", true);
    d.fx.add_service("redis", "redis", true);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nredis = true\n");
    let d = d.start().await;
    let mut c = d.client().await;

    // From anywhere inside the project, and by the site's name: the one block, on the real port.
    let inside = dir.join("app");
    std::fs::create_dir_all(&inside).unwrap();
    let want = json!([{"instance": "redis", "lines": redis_lines(redis), "note": null}]);
    let by_path = env(&mut c, json!({"path": inside.display().to_string()})).await.unwrap();
    assert_eq!(Value::Array(by_path), want);
    let by_site = env(&mut c, json!({"site": "blog"})).await.unwrap();
    assert_eq!(Value::Array(by_site), want);

    // `service` names one instance, whatever the site says.
    let one = env(&mut c, json!({"service": "mysql-8.4", "site": "blog"})).await.unwrap();
    let want = json!([{"instance": "mysql-8.4", "lines": db_lines("mysql", mysql, "root"), "note": null}]);
    assert_eq!(Value::Array(one), want);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_names_a_missing_instance() {
    let redis = free_port();
    let d = prepared(&pin("redis", redis)).await;
    d.fx.add_formula("redis");
    d.fx.add_service("redis", "redis", true);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = \"8.4\"\nredis = true\nminio = true\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let blocks = env(&mut c, json!({"site": "blog"})).await.unwrap();
    let want = json!([
        {"instance": "mysql-8.4", "lines": [], "note": "mysql-8.4 is not added: yard service add mysql --version 8.4"},
        {"instance": "redis", "lines": redis_lines(redis), "note": null},
        {"instance": "minio", "lines": [], "note": "minio is not added: yard service add minio"},
    ]);
    assert_eq!(Value::Array(blocks), want);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_true_picks_the_default_line_else_the_first() {
    let (a, b, c_port) = (free_port(), free_port(), free_port());
    let d = prepared(&(pin("mysql-8.0", a) + &pin("mysql-8.4", b) + &pin("mysql-9.0", c_port))).await;
    for formula in ["mysql@8.0", "mysql@8.4", "mysql@9.0"] {
        d.fx.add_formula(formula);
    }
    d.fx.add_service("mysql-8.0", "mysql@8.0", true);
    d.fx.add_service("mysql-9.0", "mysql@9.0", true);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = true\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let first = env(&mut c, json!({"site": "blog"})).await.unwrap();
    assert_eq!(first[0]["instance"], "mysql-8.0", "no default line yet, so the first by name");
    assert_eq!(first[0]["lines"], db_lines("mysql", a, "root"));

    d.fx.add_service("mysql-8.4", "mysql@8.4", true);
    let mut chosen = Value::Null;
    for _ in 0..150 {
        chosen = env(&mut c, json!({"site": "blog"})).await.unwrap()[0].clone();
        if chosen["instance"] == "mysql-8.4" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(chosen["instance"], "mysql-8.4", "the default line's instance wins once it exists");
    assert_eq!(chosen["lines"], db_lines("mysql", b, "root"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_without_project_lists_all() {
    let (redis, minio, console) = (free_port(), free_port(), free_port());
    let d = prepared(&(pin("redis", redis) + &pin("minio", minio) + &pin("minio-console", console))).await;
    d.fx.add_formula("redis");
    d.fx.add_formula("minio");
    d.fx.add_service("redis", "redis", true);
    d.fx.add_service("minio", "minio/stable/minio", true);
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;

    let outside = tempfile::tempdir().unwrap();
    let want = json!([
        {"instance": "minio", "lines": [
            "AWS_ACCESS_KEY_ID=minioadmin",
            "AWS_SECRET_ACCESS_KEY=minioadmin",
            "AWS_DEFAULT_REGION=us-east-1",
            format!("AWS_ENDPOINT=http://127.0.0.1:{minio}"),
            "AWS_USE_PATH_STYLE_ENDPOINT=true",
        ], "note": null},
        {"instance": "redis", "lines": redis_lines(redis), "note": null},
    ]);
    let outside_dir = outside.path().display().to_string();
    assert_eq!(Value::Array(env(&mut c, json!({"path": outside_dir})).await.unwrap()), want);
    assert_eq!(Value::Array(env(&mut c, json!({})).await.unwrap()), want);

    // A linked project with no `.yard` lists no services: there is nothing it asks for.
    assert_eq!(env(&mut c, json!({"site": "blog"})).await.unwrap(), Vec::<Value>::new());

    let e = env(&mut c, json!({"service": "mysql-8.4"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ServiceNotFound);
    let e = env(&mut c, json!({"site": "nope"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::SiteNotFound);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_goes_to_the_innermost_linked_project() {
    let (mysql, redis) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", true);
    d.fx.add_service("redis", "redis", true);
    let outer = d.project("outer");
    write_yard(&outer, "[services]\nredis = true\n");
    let inner = outer.join("packages/inner");
    std::fs::create_dir_all(&inner).unwrap();
    let mut file = yard_core::config::TomlFile::load(&d.fx.paths.sites_toml()).unwrap();
    file.set(&["sites", "inner", "path"], toml_edit::value(inner.display().to_string()));
    file.save().unwrap();
    write_yard(&inner, "[services]\nmysql = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let blocks = env(&mut c, json!({"path": inner.join("src").display().to_string()})).await.unwrap();
    assert_eq!(blocks.iter().map(|b| b["instance"].clone()).collect::<Vec<_>>(), [json!("mysql-8.4")]);
    let blocks = env(&mut c, json!({"path": outer.display().to_string()})).await.unwrap();
    assert_eq!(blocks.iter().map(|b| b["instance"].clone()).collect::<Vec<_>>(), [json!("redis")]);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_two_databases_note() {
    let (mysql, postgres) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("postgres-17", postgres))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("postgresql@17");
    d.fx.add_service("mysql-8.4", "mysql@8.4", true);
    d.fx.add_service("postgres-17", "postgresql@17", true);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = \"8.4\"\npostgres = \"17\"\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let blocks = env(&mut c, json!({"site": "blog"})).await.unwrap();
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let want = json!([
        {"instance": "mysql-8.4", "lines": db_lines("mysql", mysql, "root"), "note": null},
        {"instance": "postgres-17", "lines": db_lines("pgsql", postgres, &user),
         "note": "both set DB_*; the second block wins in a .env"},
    ]);
    assert_eq!(Value::Array(blocks), want);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn site_info_lists_its_services() {
    let (mysql, redis) = (free_port(), free_port());
    let d = prepared(&(pin("mysql-8.4", mysql) + &pin("redis", redis))).await;
    d.fx.add_formula("mysql@8.4");
    d.fx.add_formula("redis");
    d.fx.add_service("mysql-8.4", "mysql@8.4", true);
    d.fx.add_service("redis", "redis", true);
    let dir = d.project("blog");
    write_yard(&dir, "[services]\nmysql = \"8.4\"\nredis = true\nminio = true\n");
    let d = d.start().await;
    let mut c = d.client().await;

    let detail = c.call("sites.info", json!({"site": "blog"})).await.unwrap();
    let services = detail["services"].as_array().unwrap();
    assert_eq!(services.len(), 3, "{detail}");
    assert_eq!((services[0]["engine"].clone(), services[0]["want"].clone()), (json!("mysql"), json!("8.4")));
    assert_eq!(services[0]["service"]["name"], "mysql-8.4");
    assert_eq!(services[0]["service"]["state"], "stopped");
    assert_eq!(services[0]["service"]["ports"], json!([{"name": "mysql-8.4", "port": mysql}]));
    assert_eq!((services[1]["engine"].clone(), services[1]["want"].clone()), (json!("redis"), Value::Null));
    assert_eq!(services[1]["service"]["name"], "redis");
    assert_eq!(services[2]["engine"], "minio");
    assert_eq!(services[2]["service"], Value::Null, "a missing instance has no info");
    assert_eq!(detail["site"]["needs"], json!([{"kind": "service", "engine": "minio"}]));
    d.shutdown().await.unwrap();
}
