//! One unit's lifecycle: pre-start port check, start, health, backoff, failed.

use super::{Health, SupervisorConfig, Table, UnitKind, UnitSpec, lock};
use crate::events::EventBus;
use crate::helper_client::HelperClient;
use crate::logsink::LogSink;
use crate::{holder, probe};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use yard_core::supervise::{Decision, RestartPolicy};
use yard_proto::{Event, Holder, HolderKind, UnitState, YardError};

const HEALTH_TIMEOUT: Duration = Duration::from_millis(200);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

pub enum Control {
    Restart,
    Stop(oneshot::Sender<()>),
}

/// Why one run of the unit ended. The process is already gone when this is returned.
enum Ended {
    Failure(String),
    Control(Control),
}

pub struct Runner {
    spec: UnitSpec,
    cfg: SupervisorConfig,
    helper: HelperClient,
    bus: EventBus,
    sink: Arc<Mutex<LogSink>>,
    table: Table,
    control: mpsc::UnboundedReceiver<Control>,
    policy: RestartPolicy,
}

impl Runner {
    pub fn new(
        spec: UnitSpec,
        cfg: SupervisorConfig,
        helper: HelperClient,
        bus: EventBus,
        sink: LogSink,
        table: Table,
        control: mpsc::UnboundedReceiver<Control>,
    ) -> Self {
        let policy = RestartPolicy::new(cfg.policy);
        Self { spec, cfg, helper, bus, sink: Arc::new(Mutex::new(sink)), table, control, policy }
    }

    pub async fn run(mut self) {
        let ack = loop {
            if let Some((port, proto)) = self.spec.port
                && !probe::is_free(port, proto)
            {
                // Retrying cannot free a foreign port, so this skips the backoff.
                let holder = holder::find_holder(port);
                self.update(|u| u.last_error = Some(port_in_use(port, holder.as_ref())));
                self.bus.emit(Event::PortConflict { unit: self.spec.name.clone(), port, holder });
                self.enter_failed();
                match self.next_control().await {
                    Control::Restart => self.policy.reset(),
                    Control::Stop(ack) => break ack,
                }
                continue;
            }
            match self.attempt().await {
                Ended::Control(Control::Restart) => self.policy.reset(),
                Ended::Control(Control::Stop(ack)) => break ack,
                Ended::Failure(reason) => {
                    self.update(|u| u.last_error = Some(reason));
                    match self.policy.on_failure(Instant::now()) {
                        Decision::RestartAfter(delay) => {
                            self.update(|u| u.restarts += 1);
                            self.set_state(UnitState::Backoff { retry_in_ms: delay.as_millis() as u64 });
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                control = self.next_control() => match control {
                                    Control::Restart => self.policy.reset(),
                                    Control::Stop(ack) => break ack,
                                },
                            }
                        }
                        Decision::Fail => {
                            self.enter_failed();
                            match self.next_control().await {
                                Control::Restart => self.policy.reset(),
                                Control::Stop(ack) => break ack,
                            }
                        }
                    }
                }
            }
        };
        self.set_state(UnitState::Stopped);
        let _ = ack.send(());
    }

    /// One start, then supervision until the unit fails or is told to stop or restart.
    async fn attempt(&mut self) -> Ended {
        self.set_state(UnitState::Starting);
        let mut process = match self.start().await {
            Ok(p) => p,
            Err(reason) => return Ended::Failure(reason),
        };
        self.update(|u| u.pid = process.pid());

        let interval = self.cfg.health_interval;
        let deadline = tokio::time::Instant::now() + self.cfg.start_grace;
        while !self.healthy().await {
            if tokio::time::Instant::now() >= deadline {
                process.stop(&self.helper, self.cfg.stop_timeout).await;
                let grace = self.cfg.start_grace.as_secs_f32();
                return Ended::Failure(format!("not healthy within {grace} s of starting"));
            }
            tokio::select! {
                status = process.wait() => return Ended::Failure(status),
                control = self.next_control() => {
                    process.stop(&self.helper, self.cfg.stop_timeout).await;
                    return Ended::Control(control);
                }
                () = tokio::time::sleep(interval) => {}
            }
        }
        self.set_state(UnitState::Running);

        let mut misses = 0;
        loop {
            tokio::select! {
                status = process.wait() => return Ended::Failure(status),
                control = self.next_control() => {
                    process.stop(&self.helper, self.cfg.stop_timeout).await;
                    return Ended::Control(control);
                }
                () = tokio::time::sleep(interval) => {
                    misses = if self.healthy().await { 0 } else { misses + 1 };
                    if misses >= 2 {
                        process.stop(&self.helper, self.cfg.stop_timeout).await;
                        return Ended::Failure("failed two health checks in a row".into());
                    }
                }
            }
        }
    }

    async fn start(&self) -> Result<Process, String> {
        match &self.spec.kind {
            UnitKind::Nginx => {
                self.helper.start_nginx().await.map_err(|e| describe(&e))?;
                let pid = self.helper.status().await.ok().and_then(|s| s.nginx_pid);
                Ok(Process::Nginx { pid })
            }
            UnitKind::Process { program, args, env } => {
                let mut cmd = Command::new(program);
                cmd.args(args)
                    .envs(env.iter().map(|(k, v)| (k, v)))
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                // SAFETY: the hook makes only async-signal-safe calls.
                unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
                let mut child = cmd.spawn().map_err(|e| format!("cannot run {}: {e}", program.display()))?;
                let mut readers = vec![];
                if let Some(out) = child.stdout.take() {
                    readers.push(forward(out, self.sink.clone()));
                }
                if let Some(err) = child.stderr.take() {
                    readers.push(forward(err, self.sink.clone()));
                }
                Ok(Process::Child { child, readers })
            }
        }
    }

    async fn healthy(&self) -> bool {
        match &self.spec.health {
            Health::Tcp(port) => {
                let connect = TcpStream::connect(("127.0.0.1", *port));
                matches!(tokio::time::timeout(HEALTH_TIMEOUT, connect).await, Ok(Ok(_)))
            }
            Health::Unix(path) => {
                matches!(tokio::time::timeout(HEALTH_TIMEOUT, UnixStream::connect(path)).await, Ok(Ok(_)))
            }
        }
    }

    /// The supervisor dropping its sender means it is gone: treat that as a stop.
    async fn next_control(&mut self) -> Control {
        match self.control.recv().await {
            Some(control) => control,
            None => Control::Stop(oneshot::channel().0),
        }
    }

    fn enter_failed(&mut self) {
        let last_lines = lock(&self.sink).tail();
        self.set_state(UnitState::Failed { last_lines });
    }

    /// Records the state and emits `UnitStateChanged` when it differs. Only a running
    /// or starting unit has a pid.
    fn set_state(&self, state: UnitState) {
        let changed = {
            let mut table = lock(&self.table);
            let Some(info) = table.get_mut(&self.spec.name) else { return };
            if !matches!(state, UnitState::Starting | UnitState::Running) {
                info.pid = None;
            }
            let changed = info.state != state;
            info.state = state.clone();
            changed
        };
        if changed {
            self.bus.emit(Event::UnitStateChanged { unit: self.spec.name.clone(), state });
        }
    }

    fn update(&self, change: impl FnOnce(&mut yard_proto::UnitInfo)) {
        if let Some(info) = lock(&self.table).get_mut(&self.spec.name) {
            change(info);
        }
    }
}

enum Process {
    Child { child: Child, readers: Vec<JoinHandle<()>> },
    Nginx { pid: Option<u32> },
}

impl Process {
    fn pid(&self) -> Option<u32> {
        match self {
            Process::Child { child, .. } => child.id(),
            Process::Nginx { pid } => *pid,
        }
    }

    /// Resolves when a child exits, after its output is in the log. nginx is daemonized,
    /// so its exit shows only as failed health checks.
    async fn wait(&mut self) -> String {
        match self {
            Process::Child { child, readers } => {
                let status = child.wait().await;
                drain(readers).await;
                match status {
                    Ok(status) => format!("exited unexpectedly ({status})"),
                    Err(e) => format!("exited; wait failed: {e}"),
                }
            }
            Process::Nginx { .. } => std::future::pending().await,
        }
    }

    /// SIGTERM, then SIGKILL after `timeout`. nginx stops through the helper.
    async fn stop(&mut self, helper: &HelperClient, timeout: Duration) {
        match self {
            Process::Child { child, readers } => {
                if let Some(pid) = child.id() {
                    let _ = signal(pid, libc::SIGTERM);
                }
                if tokio::time::timeout(timeout, child.wait()).await.is_err() {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
                drain(readers).await;
            }
            Process::Nginx { .. } => {
                if let Err(e) = helper.stop_nginx().await {
                    eprintln!("yardd: stopping nginx: {}", describe(&e));
                }
            }
        }
    }
}

/// Copies a child's output into the log line by line; invalid UTF-8 is replaced.
fn forward(pipe: impl AsyncRead + Unpin + Send + 'static, sink: Arc<Mutex<LogSink>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        while matches!(reader.read_until(b'\n', &mut buf).await, Ok(n) if n > 0) {
            let line = String::from_utf8_lossy(&buf);
            lock(&sink).write_line(line.trim_end_matches(['\n', '\r']));
            buf.clear();
        }
    })
}

/// A grandchild can keep a pipe open after the child exits, so this is bounded.
async fn drain(readers: &mut Vec<JoinHandle<()>>) {
    for reader in readers.drain(..) {
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, reader).await;
    }
}

pub(super) fn signal(pid: u32, sig: libc::c_int) -> Result<(), YardError> {
    let raw = i32::try_from(pid).map_err(YardError::internal)?;
    // SAFETY: kill(2) on a pid this supervisor spawned.
    if unsafe { libc::kill(raw, sig) } == 0 {
        Ok(())
    } else {
        Err(YardError::internal(format!("cannot signal pid {pid}: {}", std::io::Error::last_os_error())))
    }
}

fn describe(e: &YardError) -> String {
    match &e.hint {
        Some(hint) => format!("{}: {} ({hint})", code_name(e), e.message),
        None => format!("{}: {}", code_name(e), e.message),
    }
}

fn code_name(e: &YardError) -> String {
    serde_json::to_value(e.code).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn port_in_use(port: u16, holder: Option<&Holder>) -> String {
    let by = match holder {
        Some(h) => format!("{} (pid {}, {})", h.name, h.pid, kind_label(h.kind)),
        None => "a process not visible to this user".into(),
    };
    format!("PORT_IN_USE: port {port} is held by {by}")
}

fn kind_label(kind: HolderKind) -> &'static str {
    match kind {
        HolderKind::Herd => "Herd",
        HolderKind::BrewServices => "brew services",
        HolderKind::Docker => "Docker",
        HolderKind::Yard => "Yard",
        HolderKind::Unknown => "unknown",
    }
}
