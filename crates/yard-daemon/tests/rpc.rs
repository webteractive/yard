//! The daemon end to end over its socket: handshake, handlers, watchers and port scan.

mod support;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::{Fixture, fast, free_port, spawn_helper, wait_until};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use yard_core::Paths;
use yard_daemon::daemon::{DaemonOptions, run};
use yard_proto::rpc::{Request, encode_line};
use yard_proto::{CheckStatus, DoctorReport, ErrorCode, PROTOCOL_VERSION, YardError};

struct Daemon {
    fx: Fixture,
    resolver: TempDir,
    parked: TempDir,
    opts: DaemonOptions,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), YardError>>>,
}

fn options(paths: Paths, brew: &Path, helper_socket: PathBuf, resolver_dir: &Path) -> DaemonOptions {
    DaemonOptions {
        paths,
        brew_prefix: brew.to_path_buf(),
        helper_socket,
        resolver_dir: resolver_dir.to_path_buf(),
        supervisor: fast(),
        debounce: Duration::from_millis(50),
        port_scan_interval: Duration::from_millis(100),
    }
}

/// A configured data root and helper; `extra_ports` lines are appended to `[ports]`.
async fn prepared(extra_ports: &str) -> Daemon {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path()).await;
    let parked = tempfile::tempdir().unwrap();
    std::fs::write(fx.paths.yard_toml(), format!("parked = [{:?}]\n", parked.path().display().to_string()))
        .unwrap();
    let ports = format!(
        "[ports]\nnginx-http = {{ port = {}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n{extra_ports}",
        free_port(),
        free_port(),
        free_port()
    );
    std::fs::write(fx.paths.ports_toml(), ports).unwrap();
    let opts = options(fx.paths.clone(), fx.brew.path(), helper.socket().to_path_buf(), resolver.path());
    Daemon { fx, resolver, parked, opts, stop: None, task: None }
}

impl Daemon {
    async fn start(mut self) -> Self {
        let (stop, stopped) = oneshot::channel::<()>();
        let opts = self.opts.clone();
        self.task = Some(tokio::spawn(run(opts, async {
            let _ = stopped.await;
        })));
        self.stop = Some(stop);
        let socket = self.fx.paths.daemon_socket();
        let up = || std::os::unix::net::UnixStream::connect(&socket).is_ok();
        assert!(wait_until(up, Duration::from_secs(5)).await, "the daemon answers on its socket");
        self
    }

    async fn client(&self) -> Client {
        let mut c = Client::connect(&self.fx.paths.daemon_socket()).await;
        c.call("hello", json!({"protocol": PROTOCOL_VERSION, "client": "test"})).await.unwrap();
        c
    }

    async fn shutdown(mut self) -> Result<(), YardError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.task.take() {
            Some(task) => task.await.unwrap(),
            None => Ok(()),
        }
    }
}

struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    events: Vec<Value>,
}

impl Client {
    async fn connect(socket: &Path) -> Self {
        let (read, write) = UnixStream::connect(socket).await.unwrap().into_split();
        Self { lines: BufReader::new(read).lines(), write, next_id: 1, events: vec![] }
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, YardError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write.write_all(&encode_line(&Request::new(id, method, params))).await.unwrap();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
                .await
                .expect("an answer within 10 s")
                .unwrap()
                .expect("connection open");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg.get("id").and_then(Value::as_u64) == Some(id) {
                return match msg.get("error") {
                    Some(e) => Err(serde_json::from_value(e["data"].clone()).unwrap()),
                    None => Ok(msg["result"].clone()),
                };
            }
            self.events.push(msg);
        }
    }

    /// The next notification's `params`, or `None` after `within`.
    async fn event(&mut self, within: Duration) -> Option<Value> {
        if !self.events.is_empty() {
            return Some(self.events.remove(0)["params"].clone());
        }
        let line = tokio::time::timeout(within, self.lines.next_line()).await.ok()?.ok()??;
        let msg: Value = serde_json::from_str(&line).unwrap();
        Some(msg["params"].clone())
    }

    async fn event_where(&mut self, within: Duration, matches: impl Fn(&Value) -> bool) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let event = self.event(left).await?;
            if matches(&event) {
                return Some(event);
            }
        }
    }

    async fn closed(&mut self) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_secs(2), self.lines.next_line()).await,
            Ok(Ok(None)) | Ok(Err(_))
        )
    }
}

fn site_names(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn hello_then_status() {
    let d = prepared("").await.start().await;
    let mut c = Client::connect(&d.fx.paths.daemon_socket()).await;
    let hello = c.call("hello", json!({"protocol": PROTOCOL_VERSION, "client": "test"})).await.unwrap();
    assert_eq!(hello["protocol"], PROTOCOL_VERSION);
    assert!(hello["daemon_version"].is_string() && hello["commit"].is_string());

    let status = c.call("status", Value::Null).await.unwrap();
    assert_eq!(status["tld"], "yard");
    assert_eq!(status["profile"], "trial");
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
    let err = c.call("hello", json!({"protocol": "2.0", "client": "test"})).await.unwrap_err();
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

#[tokio::test]
async fn parked_mkdir_emits_site_added() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    c.call("subscribe", json!({"topics": ["sites"]})).await.unwrap();

    std::fs::create_dir(d.parked.path().join("fresh")).unwrap();
    let added = c
        .event_where(Duration::from_secs(5), |e| e["type"] == "site_added" && e["site"]["name"] == "fresh")
        .await;
    assert!(added.is_some(), "SiteAdded for the new directory");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn status_reports_a_failed_reconcile() {
    let d = prepared("").await;
    std::fs::create_dir_all(d.parked.path().join("blog")).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    std::fs::write(d.fx.paths.nginx_override("blog"), "yard-test-broken;\n").unwrap();

    std::fs::create_dir(d.parked.path().join("shop")).unwrap();
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
    std::fs::create_dir_all(d.parked.path().join("blog")).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let over = d.fx.paths.nginx_override("blog");

    std::fs::write(&over, "yard-test-broken;\n").unwrap();
    assert_eq!(config_error_becomes(&mut c, json!("NGINX_CONFIG_INVALID")).await, "NGINX_CONFIG_INVALID");
    // Nothing was rendered, so nothing was put back: nginx simply kept its config.
    let message = c.call("status", Value::Null).await.unwrap()["config_error"]["message"].clone();
    let message = message.as_str().unwrap_or_default();
    assert!(message.contains("nginx keeps serving the config it had"), "{message}");
    assert!(!message.contains("back in place"), "{message}");
    std::fs::write(&over, "client_max_body_size 2G;\n").unwrap();
    assert_eq!(config_error_becomes(&mut c, Value::Null).await, Value::Null);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_nginx_refuses_a_broken_config() {
    let d = prepared("").await;
    std::fs::create_dir_all(d.parked.path().join("blog")).unwrap();
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
    let blog = d.parked.path().join("blog");
    std::fs::create_dir(&blog).unwrap();
    let docroot = |sites: &Value| {
        sites
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "blog")
            .map(|s| s["docroot"].as_str().unwrap().to_string())
    };
    let mut seen = None;
    for _ in 0..50 {
        seen = docroot(&c.call("sites.list", Value::Null).await.unwrap());
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen.as_deref().is_some_and(|d| !d.ends_with("/public")), "blog is served from its root first");

    // `laravel new` creates public/ seconds after the project directory.
    std::fs::create_dir(blog.join("public")).unwrap();
    for _ in 0..50 {
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
async fn profile_switch_refuses_a_foreign_resolver() {
    let d = prepared("").await.start().await;
    // Herd's file. The check comes before any probe of 80/443, which tests never bind.
    std::fs::write(d.resolver.path().join("test"), "nameserver 127.0.0.1\n").unwrap();
    let before = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    let mut c = d.client().await;

    let e = c.call("ports.profile", json!({"profile": "default"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ResolverForeign, "{e:?}");
    assert!(e.message.contains("test"), "{}", e.message);
    assert_eq!(std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap(), before);
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
