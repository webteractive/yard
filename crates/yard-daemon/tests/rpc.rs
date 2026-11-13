//! The daemon end to end over its socket: handshake, handlers, watchers and port scan.

mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::daemon::{Client, prepared, prepared_with, site_names};
use support::{free_port, wait_until};
use yard_core::Paths;
use yard_daemon::daemon::run;
use yard_proto::{CheckStatus, DoctorReport, ErrorCode, PROTOCOL_VERSION};

#[tokio::test]
async fn hello_then_status() {
    let d = prepared("").await.start().await;
    let mut c = Client::connect(&d.fx.paths.daemon_socket()).await;
    let hello = c.call("hello", json!({"protocol": PROTOCOL_VERSION, "client": "test"})).await.unwrap();
    assert_eq!(hello["protocol"], PROTOCOL_VERSION);
    assert!(hello["daemon_version"].is_string() && hello["commit"].is_string());

    let status = c.call("status", Value::Null).await.unwrap();
    assert_eq!(status["tld"], "yard");
    assert_eq!(status["profile"], "standard");
    assert!(status["units"].as_array().unwrap().iter().any(|u| u["name"] == "nginx"), "{status}");
    assert_eq!(status["version"]["daemon"], env!("CARGO_PKG_VERSION"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn doctor_reports_the_running_daemon() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let report: DoctorReport = serde_json::from_value(c.call("doctor", Value::Null).await.unwrap()).unwrap();
    let status = |id: &str| report.checks.iter().find(|c| c.id == id).map(|c| c.status);
    assert_eq!(status("daemon"), Some(CheckStatus::Ok), "{report:#?}");
    assert_eq!(status("resolver"), Some(CheckStatus::Ok), "reads the daemon's resolver dir: {report:#?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn protocol_major_mismatch_is_refused() {
    let d = prepared("").await.start().await;
    let mut c = Client::connect(&d.fx.paths.daemon_socket()).await;
    let err = c.call("hello", json!({"protocol": "3.0", "client": "test"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ProtocolMismatch);
    assert!(err.hint.is_some_and(|h| h.starts_with("upgrade yard and yardd together (yardd ")));
    assert!(c.closed().await, "the connection closes");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_before_hello_is_invalid_request() {
    let d = prepared("").await.start().await;
    let mut c = Client::connect(&d.fx.paths.daemon_socket()).await;
    let err = c.call("status", Value::Null).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRequest);
    c.call("hello", json!({"protocol": PROTOCOL_VERSION, "client": "test"})).await.unwrap();
    c.call("status", Value::Null).await.unwrap();
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn link_and_unlink_round_trip() {
    let d = prepared("").await;
    std::fs::write(d.fx.paths.sites_toml(), "# keep me\n").unwrap();
    let d = d.start().await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("public")).unwrap();
    let mut c = d.client().await;

    let site = c
        .call("sites.link", json!({"name": "proj", "path": project.path().display().to_string()}))
        .await
        .unwrap();
    assert_eq!(site["name"], "proj");
    assert!(site_names(&c.call("sites.list", Value::Null).await.unwrap()).contains(&"proj".to_string()));
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(toml.contains("# keep me\n") && toml.contains("[sites.proj]"), "{toml}");

    c.call("sites.unlink", json!({"name": "proj"})).await.unwrap();
    assert!(!site_names(&c.call("sites.list", Value::Null).await.unwrap()).contains(&"proj".to_string()));
    let err = c.call("sites.unlink", json!({"name": "proj"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::SiteNotFound);
    let err = c.call("sites.link", json!({"name": "nope", "path": "/does/not/exist"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    d.shutdown().await.unwrap();
}

/// A link that starts a PHP version's pool answers only once the pool listens, so the
/// site's first request reaches PHP instead of a 502 from nginx (found on a Mac where no
/// other site kept that pool running).
#[tokio::test]
async fn link_answers_once_a_pool_it_started_listens() {
    let d = prepared("").await;
    std::fs::write(d.fx.brew.path().join("fake-fpm-delay"), "500").unwrap();
    let d = d.start().await;
    let socket = d.fx.paths.fpm_socket(yard_core::PhpVersion::DEFAULT);
    assert!(!socket.exists(), "no site uses PHP yet, so no pool should listen");
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("public")).unwrap();
    let mut c = d.client().await;

    c.call("sites.link", json!({"name": "proj", "path": project.path().display().to_string()}))
        .await
        .unwrap();
    assert!(socket.exists(), "sites.link answered before {} listened", socket.display());
    d.shutdown().await.unwrap();
}

/// A client that shows a list refetches it after a reconcile (M6 design §7.2); a broken file
/// arrives as the event's `config_error`.
#[tokio::test]
async fn reconciled_follows_a_hand_edit() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["state"]})).await.unwrap();

    d.project("scratch");
    let done = c.event_where(Duration::from_secs(5), |e| e["type"] == "reconciled").await;
    let done = done.expect("a reconciled event after the edit");
    assert_eq!(done["config_error"], Value::Null, "{done}");

    std::fs::write(d.fx.paths.sites_toml(), "[sites").unwrap();
    let broken = c
        .event_where(Duration::from_secs(5), |e| e["type"] == "reconciled" && !e["config_error"].is_null())
        .await
        .expect("a reconciled event naming the broken file");
    assert_eq!(broken["config_error"]["code"], "CONFIG_INVALID", "{broken}");
    d.shutdown().await.unwrap();
}

/// Yard.app's session against a real yardd: it connects, calls, and hears a hand edit as
/// `reconciled` (M6 Task 6).
#[tokio::test]
async fn a_session_follows_a_real_yardd() {
    use yard_client::session::{ConnectionState, Session, SessionConfig, Update};
    let d = prepared("").await.start().await;
    let config = SessionConfig::new(d.fx.paths.daemon_socket(), "Yard.app test".into());
    let (session, mut updates) = Session::start(config);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, updates.recv()).await.expect("connected within 5 s") {
            Some(Update::State(ConnectionState::Connected(info))) => {
                assert_eq!(info.epoch, 1);
                assert!(!info.stale, "{info:?}");
                break;
            }
            Some(Update::State(ConnectionState::Connecting)) | Some(Update::Event(_)) => continue,
            other => panic!("{other:?}"),
        }
    }
    assert!(session.call(yard_client::api::SITES_LIST, ()).await.unwrap().is_empty());
    d.project("scratch");
    let reconciled = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(u) = updates.recv().await {
            if matches!(u, Update::Event(yard_proto::Event::Reconciled { .. })) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(reconciled, Ok(true));
    d.shutdown().await.unwrap();
}

/// When yardd goes, the session says so (found 2026-11-06: Yard.app kept showing a yardd that
/// had exited).
#[tokio::test]
async fn a_session_notices_yardd_going() {
    use yard_client::session::{ConnectionState, Session, SessionConfig, Update};
    let d = prepared("").await.start().await;
    let config = SessionConfig::new(d.fx.paths.daemon_socket(), "Yard.app test".into());
    let (_session, mut updates) = Session::start(config);
    let state = |u: Option<Update>| match u {
        Some(Update::State(s)) => Some(s),
        _ => None,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !matches!(
        state(tokio::time::timeout_at(deadline, updates.recv()).await.unwrap()),
        Some(ConnectionState::Connected(_))
    ) {}
    d.shutdown().await.unwrap();
    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(ConnectionState::Disconnected(e)) = state(updates.recv().await) {
                return e;
            }
        }
    })
    .await;
    assert!(gone.is_ok(), "no Disconnected within 5 s of yardd shutting down");
}

#[tokio::test]
async fn linking_in_sites_toml_emits_site_added() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["sites"]})).await.unwrap();

    d.project("fresh");
    let added = c
        .event_where(Duration::from_secs(5), |e| e["type"] == "site_added" && e["site"]["name"] == "fresh")
        .await;
    assert!(added.is_some(), "SiteAdded for the new link");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn status_reports_a_failed_reconcile() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    std::fs::write(d.fx.paths.nginx_override("blog"), "yard-test-broken;\n").unwrap();

    d.project("shop");
    assert_eq!(config_error_becomes(&mut c, json!("NGINX_CONFIG_INVALID")).await, "NGINX_CONFIG_INVALID");
    d.shutdown().await.unwrap();
}

/// `config_error.code` from `status`, polled until it is `want` (or 5 s pass).
async fn config_error_becomes(c: &mut Client, want: Value) -> Value {
    let mut code = Value::Null;
    for _ in 0..50 {
        code = c.call("status", Value::Null).await.unwrap()["config_error"]["code"].clone();
        if code == want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    code
}

#[tokio::test]
async fn override_edit_is_tested_and_applied() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    let over = d.fx.paths.nginx_override("blog");
    let pid = nginx_pid(&mut c).await;

    std::fs::write(&over, "yard-test-broken;\n").unwrap();
    assert_eq!(config_error_becomes(&mut c, json!("NGINX_CONFIG_INVALID")).await, "NGINX_CONFIG_INVALID");
    // Nothing was rendered, so nothing was put back: nginx simply kept its config.
    let message = c.call("status", Value::Null).await.unwrap()["config_error"]["message"].clone();
    let message = message.as_str().unwrap_or_default();
    assert!(message.contains("nginx keeps serving the config it had"), "{message}");
    assert!(!message.contains("back in place"), "{message}");
    std::fs::write(&over, "client_max_body_size 2G;\n").unwrap();
    assert_eq!(config_error_becomes(&mut c, Value::Null).await, Value::Null);
    assert_eq!(nginx_pid(&mut c).await, pid, "the override was reloaded, not restarted");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn tunnel_override_edit_is_tested() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    nginx_pid(&mut c).await;
    let over = d.fx.paths.tunnel_override("blog");
    std::fs::write(&over, "yard-test-broken;\n").unwrap();
    assert_eq!(config_error_becomes(&mut c, json!("NGINX_CONFIG_INVALID")).await, "NGINX_CONFIG_INVALID");
    std::fs::write(&over, "client_max_body_size 2G;\n").unwrap();
    assert_eq!(config_error_becomes(&mut c, Value::Null).await, Value::Null);
    d.shutdown().await.unwrap();
}

/// The nginx unit's pid from `status`, once it is running.
async fn nginx_pid(c: &mut Client) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let units = c.call("status", Value::Null).await.unwrap()["units"].clone();
        let nginx = units.as_array().unwrap().iter().find(|u| u["name"] == "nginx").cloned().unwrap();
        if nginx["state"] == "running" {
            return nginx["pid"].clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "nginx never ran: {nginx}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn restart_nginx_refuses_when_the_config_cannot_be_tested() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    let pid = nginx_pid(&mut c).await;
    // A running nginx keeps its binary; a new one, or `nginx -t`, cannot start.
    std::fs::remove_file(yard_core::brew::Brew::new(d.fx.brew.path()).nginx_bin()).unwrap();

    let e = c.call("units.restart", json!({"unit": "nginx"})).await;
    assert!(e.is_err(), "restart reported success: {e:?}");
    assert_eq!(nginx_pid(&mut c).await, pid, "nginx was left alone");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn profile_privileged_refuses_without_a_helper() {
    let mut d = prepared("").await;
    d.opts.helper_socket = "/tmp/yard-test-missing/helper.sock".into();
    let d = d.start().await;
    let mut c = d.client().await;
    let before = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();

    // Only the helper can open 80/443, so switching without one would serve nothing.
    let e = c.call("ports.profile", json!({"profile": "privileged"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::HelperNotInstalled, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some("sudo yard helper install"));
    assert_eq!(std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap(), before, "nothing changed");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn privileged_sockets_are_rebound_after_a_yardd_restart() {
    let (http, https) = (free_port(), free_port());
    let d = prepared_with("", &[http, https]).await;
    let ports = format!("profile = \"privileged\"\n{}", support::ports_toml(http, https, free_port()));
    std::fs::write(d.fx.paths.ports_toml(), ports).unwrap();
    d.project("blog");
    let paths = d.fx.paths.clone();
    let starts = |n: usize| {
        let paths = paths.clone();
        move || support::inherited_starts(&paths) == n
    };
    let mut d = d.start().await;
    nginx_pid(&mut d.client().await).await;
    assert!(wait_until(starts(1), Duration::from_secs(5)).await, "the first yardd's nginx inherited");
    assert!(support::served(http), "the first yardd serves {http}");

    d.stop().await.unwrap();
    let d = d.start().await;
    nginx_pid(&mut d.client().await).await;
    assert!(wait_until(starts(2), Duration::from_secs(5)).await, "the second yardd's nginx inherited");
    assert!(support::served(http), "the second yardd got {http} from the helper again");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_nginx_refuses_a_broken_config() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    std::fs::write(d.fx.paths.nginx_override("blog"), "yard-test-broken;\n").unwrap();

    let e = c.call("units.restart", json!({"unit": "nginx"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NginxConfigInvalid);
    let units = c.call("status", Value::Null).await.unwrap()["units"].clone();
    let nginx = units.as_array().unwrap().iter().find(|u| u["name"] == "nginx").unwrap().clone();
    assert_eq!(nginx["state"], "running", "nginx keeps serving: {nginx}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_port_change_nginx_rejects_is_not_kept() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    nginx_pid(&mut c).await;
    let before = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    // nginx rejects every config while this override is broken.
    std::fs::write(d.fx.paths.nginx_override("blog"), "yard-test-broken;\n").unwrap();

    let e = c.call("ports.set", json!({"unit": "nginx-http", "port": free_port()})).await.unwrap_err();

    assert_eq!(e.code, ErrorCode::NginxConfigInvalid, "{e:?}");
    let after = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    assert_eq!(after, before, "a change nginx refused is not left in ports.toml to fail every later one");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ports_check_reads_ports_toml_from_disk() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let file = d.fx.paths.ports_toml();
    let text = std::fs::read_to_string(&file).unwrap();
    let dns = text.lines().find(|l| l.starts_with("dnsmasq")).unwrap().replace("dnsmasq", "redis");
    std::fs::write(&file, format!("{text}{dns}\n")).unwrap();

    // `ports check` is about the file as it is now, not the last registry that parsed.
    let e = c.call("ports.check", Value::Null).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfigInvalid, "{e:?}");
    assert!(e.message.contains("ports.toml"), "{}", e.message);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn docroot_created_after_the_project_is_picked_up() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let blog = d.project("blog");
    let docroot = |sites: &Value| {
        sites
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "blog")
            .map(|s| s["docroot"].as_str().unwrap().to_string())
    };
    let mut seen = None;
    // Up to 15 s: FSEvents can lag several seconds while the whole suite runs.
    for _ in 0..150 {
        seen = docroot(&c.call("sites.list", Value::Null).await.unwrap());
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen.as_deref().is_some_and(|d| !d.ends_with("/public")), "blog is served from its root first");

    // `laravel new` creates public/ seconds after the project directory.
    std::fs::create_dir(blog.join("public")).unwrap();
    for _ in 0..150 {
        seen = docroot(&c.call("sites.list", Value::Null).await.unwrap());
        if seen.as_deref().is_some_and(|d| d.ends_with("/public")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen.as_deref().is_some_and(|d| d.ends_with("/public")), "{seen:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn hand_edit_of_sites_toml_is_picked_up() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;

    std::fs::write(d.fx.paths.sites_toml(), "[sites.api]\nproxy = \"http://127.0.0.1:3000\"\n").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let names = site_names(&c.call("sites.list", Value::Null).await.unwrap());
        if names.contains(&"api".to_string()) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "api never appeared: {names:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn foreign_listener_on_idle_port_emits_conflict() {
    let p = free_port();
    let d = prepared(&format!("redis = {{ port = {p}, mode = \"preferred\", preferred = {p} }}\n"))
        .await
        .start()
        .await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["ports"]})).await.unwrap();

    // The daemon's scan binds the idle port for a moment to probe it; retry past that.
    let mut held = None;
    for _ in 0..100 {
        if let Ok(l) = std::net::TcpListener::bind(("127.0.0.1", p)) {
            held = Some(l);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let _held = held.expect("the port is ours between scans");
    let conflict = c
        .event_where(Duration::from_secs(5), |e| e["type"] == "port_conflict" && e["unit"] == "redis")
        .await
        .expect("PortConflict for redis");
    assert_eq!(conflict["port"], p);
    assert_eq!(conflict["holder"]["pid"], std::process::id());
    let again = c.event_where(Duration::from_millis(400), |e| e["type"] == "port_conflict").await;
    assert!(again.is_none(), "one event per transition: {again:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn second_daemon_refuses_and_stale_socket_is_replaced() {
    let d = prepared("").await.start().await;
    let err = run(d.opts.clone(), async {}).await.unwrap_err();
    assert!(err.message.contains("already running"), "{err:?}");
    d.shutdown().await.unwrap();

    let d = prepared("").await;
    let socket = d.fx.paths.daemon_socket();
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists(), "a stale socket file is left behind");
    let d = d.start().await;
    d.client().await;
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn overlong_socket_path_is_reported() {
    let d = prepared("").await;
    let long = d.fx.root.path().join("x".repeat(110));
    let mut opts = d.opts.clone();
    opts.paths = Paths::new(&long);
    let err = run(opts, async {}).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains(&long.join("yardd.sock").display().to_string()), "{err:?}");
}

#[tokio::test]
async fn the_data_root_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let d = prepared("").await;
    let root = d.fx.paths.root().to_path_buf();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = d.start().await;
    let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "{mode:o}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_removes_socket_and_stops_units() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let http = {
        let ports = c.call("ports.list", Value::Null).await.unwrap();
        ports.as_array().unwrap().iter().find(|p| p["unit"] == "nginx-http").unwrap()["port"]
            .as_u64()
            .unwrap() as u16
    };
    assert!(wait_until(|| yard_daemon::probe::is_listening(http), Duration::from_secs(5)).await);

    let socket = d.fx.paths.daemon_socket();
    d.shutdown().await.unwrap();
    assert!(!socket.exists());
    assert!(!yard_daemon::probe::is_listening(http), "nginx stopped");
}

#[tokio::test]
async fn park_method_is_gone() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    for method in ["sites.park", "sites.unpark"] {
        let err = c.call(method, json!({"path": "/tmp"})).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::MethodNotFound, "{method}");
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn secure_then_unsecure_round_trips_sites_toml() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let before = std::fs::read(d.fx.paths.sites_toml()).unwrap();
    let mut c = d.client().await;

    let site = c.call("sites.secure", json!({"site": "blog"})).await.unwrap();
    assert_eq!(site["secure"], true);
    assert!(site["url"].as_str().unwrap().starts_with("https://blog.yard:"), "{site}");
    assert!(d.fx.paths.cert("blog").exists());

    let site = c.call("sites.unsecure", json!({"site": "blog"})).await.unwrap();
    assert_eq!(site["secure"], false);
    assert!(site["url"].as_str().unwrap().starts_with("http://"), "{site}");
    assert_eq!(std::fs::read(d.fx.paths.sites_toml()).unwrap(), before, "sites.toml is as it was");
    assert!(!d.fx.paths.cert("blog").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsecure_overrides_secure_by_default() {
    let d = prepared("").await;
    d.project("blog");
    std::fs::write(d.fx.paths.yard_toml(), "secure_by_default = true\n").unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let site = c.call("sites.unsecure", json!({"site": "blog"})).await.unwrap();
    assert_eq!(site["secure"], false);
    assert!(site["url"].as_str().unwrap().starts_with("http://"), "{site}");
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(toml.contains("secure = false"), "{toml}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn secure_works_for_a_proxy() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let url = format!("http://127.0.0.1:{}", free_port());
    c.call("sites.proxy", json!({"name": "api", "url": url})).await.unwrap();

    let site = c.call("sites.secure", json!({"site": "api"})).await.unwrap();
    assert_eq!(site["secure"], true);
    assert!(d.fx.paths.cert("api").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn secure_an_unknown_site_is_site_not_found() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    for method in ["sites.secure", "sites.unsecure"] {
        let err = c.call(method, json!({"site": "nope"})).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::SiteNotFound, "{method}");
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ca_info_reports_trust_from_security() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;

    let info = c.call("ca.info", Value::Null).await.unwrap();
    assert_eq!(info["trusted"], false);
    let ca = d.fx.paths.ca_cert();
    assert_eq!(info["cert"], ca.display().to_string());
    assert_eq!(info["sha256"].as_str().unwrap().len(), 64, "{info}");

    std::fs::write(d.fx.security_dir().join("fake-trusted"), "").unwrap();
    let info = c.call("ca.info", Value::Null).await.unwrap();
    assert_eq!(info["trusted"], true);
    let log = std::fs::read_to_string(d.fx.security_dir().join("security.log")).unwrap();
    assert!(log.contains(&format!("verify-cert -c {} -l -L -q -p basic", ca.display())), "{log}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsecure_reads_secure_by_default_from_yard_toml_not_the_last_good_state() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    // A ports.toml that does not parse keeps the last good state, from before the
    // secure_by_default that follows.
    std::fs::write(d.fx.paths.ports_toml(), "[ports\n").unwrap();
    std::fs::write(d.fx.paths.yard_toml(), "secure_by_default = true\n").unwrap();

    let err = c.call("sites.unsecure", json!({"site": "blog"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid, "{err:?}");
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(toml.contains("secure = false"), "unsecure opts out of the default it will meet: {toml}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn commands_are_refused_with_a_project_file() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    let dir = d.project("ee");
    std::fs::write(dir.join(".yard"), "").unwrap();
    // A managed file beside it is hidden by the project's own: still refused, never edited.
    let managed = d.fx.paths.managed_project_file("ee");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, format!("project = {:?}\n", dir.display().to_string())).unwrap();
    let managed_before = std::fs::read(&managed).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let before = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    let file = dir.join(".yard").display().to_string();
    for (method, params, change) in [
        ("sites.isolate", json!({"site": "ee", "php": "7.1"}), "php = \"7.1\""),
        ("sites.isolate", json!({"site": "ee"}), "remove php"),
        ("sites.secure", json!({"site": "ee"}), "secure = true"),
        ("sites.unsecure", json!({"site": "ee"}), "secure = false"),
        ("sites.docroot", json!({"site": "ee", "path": "web"}), "docroot = \"web\""),
    ] {
        let e = c.call(method, params).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::SiteHasProjectFile, "{method}");
        assert_eq!(e.message, format!("ee is configured by {file}"), "{method}");
        assert_eq!(e.hint, Some(format!("edit {file}: {change}")), "{method}");
    }
    assert_eq!(std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap(), before);
    assert_eq!(std::fs::read(&managed).unwrap(), managed_before);
    d.shutdown().await.unwrap();
}
