//! One unit's lifecycle: pre-start port check, start, health, backoff, failed.

use super::{CONFLICT, Health, SupervisorConfig, Table, UnitSpec, lock};
use crate::events::EventBus;
use crate::logsink::LogSink;
use crate::privileged::PrivilegedSockets;
use crate::{describe, holder, probe};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use yard_core::supervise::{Decision, RestartPolicy};
use yard_proto::{ErrorCode, Event, Holder, UnitState, YardError};

const HEALTH_TIMEOUT: Duration = Duration::from_millis(200);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

pub enum Control {
    /// Forget past failures and start again. The sender, when there is one, hears once the
    /// runner has taken the restart and left the state it was in (`Starting`), so a caller
    /// that reads the state then reads this restart's, never the failure it replaces.
    Restart(Option<oneshot::Sender<()>>),
    Stop(oneshot::Sender<()>),
}

/// Why one run of the unit ended. The process is already gone when this is returned.
enum Ended {
    Failure(String),
    /// A failure retrying cannot fix (no helper installed): straight to `Failed`.
    Fatal(String),
    /// The helper found the port held by someone else; handled like the pre-start probe.
    Conflict(u16),
    Control(Control),
}

pub struct Runner {
    spec: UnitSpec,
    cfg: SupervisorConfig,
    bus: EventBus,
    sockets: Arc<PrivilegedSockets>,
    sink: Arc<Mutex<LogSink>>,
    table: Table,
    control: mpsc::UnboundedReceiver<Control>,
    policy: RestartPolicy,
}

impl Runner {
    pub fn new(
        spec: UnitSpec,
        cfg: SupervisorConfig,
        bus: EventBus,
        sockets: Arc<PrivilegedSockets>,
        sink: LogSink,
        table: Table,
        control: mpsc::UnboundedReceiver<Control>,
    ) -> Self {
        let policy = RestartPolicy::new(cfg.policy);
        Self { spec, cfg, bus, sockets, sink: Arc::new(Mutex::new(sink)), table, control, policy }
    }

    pub async fn run(mut self) {
        let ack = loop {
            if let Some(port) = self.busy_port() {
                match self.conflict(port).await {
                    Control::Restart(_) => self.policy.reset(),
                    Control::Stop(ack) => break ack,
                }
                continue;
            }
            match self.attempt().await {
                Ended::Control(Control::Restart(_)) => self.policy.reset(),
                Ended::Control(Control::Stop(ack)) => break ack,
                Ended::Conflict(port) => match self.conflict(port).await {
                    Control::Restart(_) => self.policy.reset(),
                    Control::Stop(ack) => break ack,
                },
                Ended::Fatal(reason) => {
                    self.update(|u| u.last_error = Some(reason));
                    self.enter_failed();
                    match self.next_control().await {
                        Control::Restart(_) => self.policy.reset(),
                        Control::Stop(ack) => break ack,
                    }
                }
                Ended::Failure(reason) => {
                    self.update(|u| u.last_error = Some(reason));
                    match self.policy.on_failure(Instant::now()) {
                        Decision::RestartAfter(delay) => {
                            self.update(|u| u.restarts += 1);
                            self.set_state(UnitState::Backoff { retry_in_ms: delay.as_millis() as u64 });
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                control = self.next_control() => match control {
                                    Control::Restart(_) => self.policy.reset(),
                                    Control::Stop(ack) => break ack,
                                },
                            }
                        }
                        Decision::Fail => {
                            self.enter_failed();
                            match self.next_control().await {
                                Control::Restart(_) => self.policy.reset(),
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

    /// The first of the unit's ports that someone holds. Every port is probed, since a
    /// unit with two (MinIO) cannot start when either is taken.
    fn busy_port(&self) -> Option<u16> {
        self.spec.ports.iter().find(|(port, proto)| !probe::is_free(*port, *proto)).map(|(port, _)| *port)
    }

    /// One start, then supervision until the unit fails or is told to stop or restart.
    /// The pid file lives exactly as long as the process.
    async fn attempt(&mut self) -> Ended {
        self.set_state(UnitState::Starting);
        let mut process = match self.start().await {
            Ok(p) => p,
            Err(ended) => return ended,
        };
        self.update(|u| u.pid = process.pid());
        self.write_pid_file(process.pid());
        let ended = self.supervise(&mut process).await;
        self.remove_pid_file();
        ended
    }

    fn write_pid_file(&self, pid: Option<u32>) {
        let (Some(path), Some(pid)) = (&self.spec.pid_file, pid) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, format!("{pid}\n")) {
            tracing::warn!(
                "cannot write {}: {e}; a killed yardd could not find {}",
                path.display(),
                self.spec.name
            );
        }
    }

    fn remove_pid_file(&self) {
        if let Some(path) = &self.spec.pid_file {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Supervision of a started process until it fails or is told to stop or restart.
    async fn supervise(&mut self, process: &mut Process) -> Ended {
        let stop_timeout = self.spec.stop_timeout.unwrap_or(self.cfg.stop_timeout);

        let interval = self.cfg.health_interval;
        let start_poll = self.cfg.start_poll;
        let grace = self.spec.start_timeout.unwrap_or(self.cfg.start_grace);
        let deadline = tokio::time::Instant::now() + grace;
        // Until the first health check passes nothing is in flight, and the unit may not
        // handle its graceful signal yet, so it gets SIGTERM.
        while !self.healthy().await {
            if tokio::time::Instant::now() >= deadline {
                process.stop(libc::SIGTERM, stop_timeout).await;
                let grace = grace.as_secs_f32();
                return Ended::Failure(format!("not healthy within {grace} s of starting"));
            }
            tokio::select! {
                status = process.wait() => return Ended::Failure(status),
                control = self.next_control() => {
                    process.stop(libc::SIGTERM, stop_timeout).await;
                    return Ended::Control(control);
                }
                () = tokio::time::sleep(start_poll) => {}
            }
        }
        self.set_state(UnitState::Running);

        let mut misses = 0;
        loop {
            tokio::select! {
                status = process.wait() => return Ended::Failure(status),
                control = self.next_control() => {
                    process.stop(self.spec.stop, stop_timeout).await;
                    return Ended::Control(control);
                }
                () = tokio::time::sleep(interval) => {
                    misses = if self.still_healthy().await { 0 } else { misses + 1 };
                    if misses >= 2 {
                        process.stop(self.spec.stop, stop_timeout).await;
                        return Ended::Failure("failed two health checks in a row".into());
                    }
                }
            }
        }
    }

    /// Records a port someone else holds and waits in `Failed`: retrying cannot free it,
    /// so this skips the backoff.
    async fn conflict(&mut self, port: u16) -> Control {
        let holder = holder::find_holder(port);
        self.update(|u| u.last_error = Some(port_in_use(port, holder.as_ref())));
        self.bus.emit(Event::PortConflict { unit: self.spec.name.clone(), port, holder });
        self.enter_failed();
        self.next_control().await
    }

    /// The sockets the unit inherits, from the helper; none for most units.
    async fn inherited(&self) -> Result<Vec<OwnedFd>, Ended> {
        if self.spec.inherit.is_empty() {
            return Ok(vec![]);
        }
        self.sockets.get(&self.spec.inherit).await.map_err(|e| match e.code {
            // Retrying cannot install a helper or change which ports it opens.
            ErrorCode::HelperNotInstalled | ErrorCode::HelperRefused | ErrorCode::InvalidArgument => {
                Ended::Fatal(describe(&e))
            }
            ErrorCode::PortInUse => {
                let port =
                    e.data.as_ref().and_then(|d| d["port"].as_u64()).and_then(|p| u16::try_from(p).ok());
                Ended::Conflict(port.unwrap_or(self.spec.inherit[0]))
            }
            _ => Ended::Failure(describe(&e)),
        })
    }

    async fn start(&self) -> Result<Process, Ended> {
        let inherited = self.inherited().await?;
        let UnitSpec { program, args, env, .. } = &self.spec;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let keep: Vec<RawFd> = inherited.iter().map(AsRawFd::as_raw_fd).collect();
        if !keep.is_empty() {
            // nginx reads `NGINX="fd;fd;"` and serves on those sockets instead of binding.
            cmd.env("NGINX", keep.iter().map(|fd| format!("{fd};")).collect::<String>());
        }
        // SAFETY: the hook makes only async-signal-safe calls: fcntl on descriptors this
        // process owns, from a Vec allocated before the fork.
        unsafe {
            cmd.pre_exec(move || {
                crate::fds::cloexec_inherited()?;
                for fd in &keep {
                    libc::fcntl(*fd, libc::F_SETFD, 0);
                }
                Ok(())
            })
        };
        crate::fds::settle_before_fork();
        let mut child =
            cmd.spawn().map_err(|e| Ended::Failure(format!("cannot run {}: {e}", program.display())))?;
        let mut readers = vec![];
        if let Some(out) = child.stdout.take() {
            readers.push(forward(out, self.sink.clone()));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(forward(err, self.sink.clone()));
        }
        Ok(Process { child, readers })
    }

    /// The check for a unit already running. A Unix socket is only looked at, never connected
    /// to: php-fpm's `ondemand` pool forks a worker per connection and reaps one only after it
    /// sits idle, so a probe every `health_interval` kept every worker it ever forked alive. A
    /// pool that dies is seen by `process.wait()`; one whose socket is gone fails here.
    async fn still_healthy(&self) -> bool {
        match &self.spec.health {
            Health::Unix(path) => {
                use std::os::unix::fs::FileTypeExt;
                tokio::fs::symlink_metadata(path).await.is_ok_and(|m| m.file_type().is_socket())
            }
            Health::Tcp(_) => self.healthy().await,
        }
    }

    async fn healthy(&self) -> bool {
        match &self.spec.health {
            Health::Tcp(port) => {
                let connect = TcpStream::connect(("127.0.0.1", *port));
                let Ok(Ok(stream)) = tokio::time::timeout(HEALTH_TIMEOUT, connect).await else {
                    return false;
                };
                // The connection it leaves in TIME_WAIT is yardd's, not a client of the unit.
                if let Ok(local) = stream.local_addr() {
                    crate::conns::own_connection(local.port());
                }
                true
            }
            Health::Unix(path) => {
                matches!(tokio::time::timeout(HEALTH_TIMEOUT, UnixStream::connect(path)).await, Ok(Ok(_)))
            }
        }
    }

    /// The supervisor dropping its sender means it is gone: treat that as a stop. A restart
    /// is under way from the moment it is taken, so the unit is `Starting` at once (a running
    /// one keeps its pid while it stops), and whoever asked for it hears so.
    async fn next_control(&mut self) -> Control {
        match self.control.recv().await {
            Some(Control::Restart(ack)) => {
                self.set_state(UnitState::Starting);
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
                Control::Restart(None)
            }
            Some(control) => control,
            None => Control::Stop(oneshot::channel().0),
        }
    }

    /// A failed unit stays down until `yard restart`, so it gives back any privileged
    /// ports: a connection to 80 is refused rather than left waiting in yardd's backlog,
    /// and something else may take the port. The next start asks the helper again.
    fn enter_failed(&mut self) {
        self.sockets.release(&self.spec.inherit);
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

struct Process {
    child: Child,
    readers: Vec<JoinHandle<()>>,
}

impl Process {
    fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Resolves when the child exits, after its output is in the log.
    async fn wait(&mut self) -> String {
        let status = self.child.wait().await;
        drain(&mut self.readers).await;
        match status {
            Ok(status) => format!("exited unexpectedly ({status})"),
            Err(e) => format!("exited; wait failed: {e}"),
        }
    }

    /// `sig`, then SIGKILL after `timeout`.
    async fn stop(&mut self, sig: libc::c_int, timeout: Duration) {
        if let Some(pid) = self.child.id() {
            let _ = signal(pid, sig);
        }
        if tokio::time::timeout(timeout, self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
        drain(&mut self.readers).await;
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

fn port_in_use(port: u16, holder: Option<&Holder>) -> String {
    format!("{CONFLICT}port {port} is held by {}", holder::label(holder))
}
