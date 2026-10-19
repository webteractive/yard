//! The daemon end to end over its socket: handshake, handlers, watchers and port scan.

mod support;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::{Fixture, free_port, spawn_helper, wait_until};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use yard_core::Paths;
use yard_core::supervise::PolicyConfig;
use yard_daemon::daemon::{DaemonOptions, run};
use yard_daemon::supervisor::SupervisorConfig;
use yard_proto::rpc::{Request, encode_line};
use yard_proto::{ErrorCode, PROTOCOL_VERSION, YardError};

struct Daemon {
    fx: Fixture,
    _resolver: TempDir,
    parked: TempDir,
    opts: DaemonOptions,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), YardError>>>,
}

fn fast() -> SupervisorConfig {
    SupervisorConfig {
        policy: PolicyConfig {
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_millis(200),
            failure_window: Duration::from_secs(60),
            max_failures: 5,
        },
        health_interval: Duration::from_millis(20),
        start_grace: Duration::from_secs(2),
        stop_timeout: Duration::from_millis(500),
        log_max_bytes: 1024 * 1024,
        log_keep: 2,
    }
}

fn options(paths: Paths, brew: &Path, helper_socket: PathBuf) -> DaemonOptions {
    DaemonOptions {
        paths,
        brew_prefix: brew.to_path_buf(),
        helper_socket,
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
    let opts = options(fx.paths.clone(), fx.brew.path(), helper.socket().to_path_buf());
    Daemon { fx, _resolver: resolver, parked, opts, stop: None, task: None }
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

    let _held = std::net::TcpListener::bind(("127.0.0.1", p)).unwrap();
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
