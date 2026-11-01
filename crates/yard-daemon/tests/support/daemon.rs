//! A real `yardd` in-process on a temp data root, and a raw JSON-RPC client for it.

use super::{Fixture, fast, free_port, spawn_helper, wait_until};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use yard_core::Paths;
use yard_core::config::TomlFile;
use yard_daemon::daemon::{DaemonOptions, run};
use yard_daemon::sleep::SleepConfig;
use yard_proto::rpc::{Request, encode_line};
use yard_proto::{PROTOCOL_VERSION, YardError};

pub struct Daemon {
    pub fx: Fixture,
    _resolver: TempDir,
    pub projects: TempDir,
    pub opts: DaemonOptions,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), YardError>>>,
}

/// How long a test daemon may take to finish its first reconcile.
const STARTUP: Duration = Duration::from_secs(60);

pub fn options(paths: Paths, brew: &Path, helper_socket: PathBuf, resolver_dir: &Path) -> DaemonOptions {
    let no_apps = paths.root().join("no-applications");
    let home = paths.root().join("home");
    DaemonOptions {
        paths,
        brew_prefix: brew.to_path_buf(),
        helper_socket,
        resolver_dir: resolver_dir.to_path_buf(),
        supervisor: fast(),
        debounce: Duration::from_millis(50),
        port_scan_interval: Duration::from_millis(100),
        shim: None,
        security: PathBuf::from("/nonexistent/security"),
        // Every engine's usual port comes from the OS, so no test binds 3306, 5432, 6379,
        // 9000 or 9001.
        usual_ports: Some(
            ["mysql", "postgres", "redis", "minio", "minio-console"]
                .into_iter()
                .map(|key| (key.to_string(), free_port()))
                .collect(),
        ),
        // No DBngin.app or Postgres.app here, whatever this machine has installed.
        app_dirs: vec![no_apps],
        // A sample every 50 ms and a millisecond for each second of a `sleep_after`, so
        // `"60s"` lasts 60 ms.
        sleep: SleepConfig { sample_interval: Duration::from_millis(50), second: Duration::from_millis(1) },
        // Never this machine's home: its emailulator and LaunchAgent are not a test's.
        home,
    }
}

/// A configured data root and helper; `extra_ports` lines are appended to `[ports]`.
pub async fn prepared(extra_ports: &str) -> Daemon {
    prepared_with(extra_ports, &[]).await
}

/// As `prepared`, with a helper that may bind `privileged_ports` (standing in for 80/443).
pub async fn prepared_with(extra_ports: &str, privileged_ports: &[u16]) -> Daemon {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), privileged_ports).await;
    let projects = tempfile::tempdir().unwrap();
    let ports = format!(
        "[ports]\nnginx-http = {{ port = {}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n{extra_ports}",
        free_port(),
        free_port(),
        free_port()
    );
    std::fs::write(fx.paths.ports_toml(), ports).unwrap();
    let mut opts = options(fx.paths.clone(), fx.brew.path(), helper.socket().to_path_buf(), resolver.path());
    opts.security = fx.security.clone();
    Daemon { fx, _resolver: resolver, projects, opts, stop: None, task: None }
}

impl Daemon {
    /// A project directory linked as `name`, as `yard link` writes it to `sites.toml`.
    pub fn project(&self, name: &str) -> PathBuf {
        let dir = self.projects.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut file = TomlFile::load(&self.fx.paths.sites_toml()).unwrap();
        file.set(&["sites", name, "path"], toml_edit::value(dir.display().to_string()));
        file.save().unwrap();
        dir
    }

    pub async fn start(mut self) -> Self {
        let (stop, stopped) = oneshot::channel::<()>();
        let opts = self.opts.clone();
        self.task = Some(tokio::spawn(run(opts, async {
            let _ = stopped.await;
        })));
        self.stop = Some(stop);
        let socket = self.fx.paths.daemon_socket();
        let up = || std::os::unix::net::UnixStream::connect(&socket).is_ok();
        assert!(wait_until(up, Duration::from_secs(5)).await, "the daemon listens on its socket");
        // yardd binds before its first reconcile and answers only once it watches its
        // files. Wait for that here, so the per-call timeout measures the call alone.
        let mut c = Client::connect(&socket).await;
        let hello = json!({"protocol": PROTOCOL_VERSION, "client": "test"});
        let answered = tokio::time::timeout(STARTUP, c.call_within("hello", hello, STARTUP * 2)).await;
        let Ok(answer) = answered else {
            panic!("the daemon did not answer hello within {STARTUP:?}{}", crowded_exe_dir());
        };
        answer.unwrap();
        self
    }

    pub async fn client(&self) -> Client {
        let mut c = Client::connect(&self.fx.paths.daemon_socket()).await;
        c.call("hello", json!({"protocol": PROTOCOL_VERSION, "client": "test"})).await.unwrap();
        c
    }

    pub async fn shutdown(mut self) -> Result<(), YardError> {
        self.stop().await
    }

    /// Stops the daemon and keeps its data root, so `start` can run another on it.
    pub async fn stop(&mut self) -> Result<(), YardError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.task.take() {
            Some(task) => task.await.unwrap(),
            None => Ok(()),
        }
    }
}

pub struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    events: Vec<Value>,
}

impl Client {
    pub async fn connect(socket: &Path) -> Self {
        let (read, write) = UnixStream::connect(socket).await.unwrap().into_split();
        Self { lines: BufReader::new(read).lines(), write, next_id: 1, events: vec![] }
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, YardError> {
        self.call_within(method, params, Duration::from_secs(10)).await
    }

    pub async fn call_within(
        &mut self,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Result<Value, YardError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write.write_all(&encode_line(&Request::new(id, method, params))).await.unwrap();
        loop {
            let line = tokio::time::timeout(wait, self.lines.next_line())
                .await
                .unwrap_or_else(|_| panic!("an answer to {method} within {wait:?}"))
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
    pub async fn event(&mut self, within: Duration) -> Option<Value> {
        if !self.events.is_empty() {
            return Some(self.events.remove(0)["params"].clone());
        }
        let line = tokio::time::timeout(within, self.lines.next_line()).await.ok()?.ok()??;
        let msg: Value = serde_json::from_str(&line).unwrap();
        Some(msg["params"].clone())
    }

    pub async fn event_where(&mut self, within: Duration, matches: impl Fn(&Value) -> bool) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let event = self.event(left).await?;
            if matches(&event) {
                return Some(event);
            }
        }
    }

    pub async fn closed(&mut self) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_secs(2), self.lines.next_line()).await,
            Ok(Ok(None)) | Ok(Err(_))
        )
    }
}

pub fn site_names(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap().to_string()).collect()
}

/// Calls a job method after subscribing to `jobs`, then waits for its `job_finished`.
/// Returns the progress lines and the job's error.
#[allow(dead_code)]
pub async fn run_job(c: &mut Client, method: &str, params: Value) -> (Vec<String>, Option<YardError>) {
    c.call("subscribe", json!({"topics": ["jobs"]})).await.unwrap();
    let started = c.call(method, params).await.unwrap();
    let id = started["job_id"].as_str().expect("a job_id").to_string();
    let mut lines = vec![];
    loop {
        let event = c.event(Duration::from_secs(10)).await.expect("the job finished");
        if event["job_id"] != id.as_str() {
            continue;
        }
        match event["type"].as_str() {
            Some("job_progress") => lines.push(event["line"].as_str().unwrap_or_default().to_string()),
            Some("job_finished") => {
                let error = serde_json::from_value(event["error"].clone()).unwrap();
                return (lines, error);
            }
            _ => {}
        }
    }
}

/// Starting an FSEvents stream makes fseventsd list the directory of the executable, one
/// stream at a time across the machine. On macOS `target/debug/deps` keeps every build's
/// object files (`split-debuginfo = "unpacked"`), and once it holds a few hundred thousand
/// each daemon takes seconds to start and the last of a test file's daemons times out.
fn crowded_exe_dir() -> String {
    const CROWDED: usize = 50_000;
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) else {
        return String::new();
    };
    let files = std::fs::read_dir(&dir).map(|entries| entries.count()).unwrap_or(0);
    if files < CROWDED {
        return String::new();
    }
    format!(
        "\n  {} holds {files} files, which makes every FSEvents watch start slowly;\n  \
         `cargo clean --profile dev` empties it",
        dir.display()
    )
}
