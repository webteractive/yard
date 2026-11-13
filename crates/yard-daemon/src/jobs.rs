//! Long operations (brew installs) as jobs (spec §6.2): the method answers with a
//! `job_id` at once, and the output streams as `job_progress` events until one
//! `job_finished`. Jobs run one at a time, in the order they were started; brew holds a
//! global lock anyway.

use crate::events::EventBus;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use yard_core::shim::child_path;
use yard_proto::{ErrorCode, Event, JobStarted, YardError};

/// Lines kept for a failed command's `data.tail`.
const TAIL: usize = 20;

type Work = Pin<Box<dyn Future<Output = ()> + Send>>;
pub struct Jobs {
    bus: EventBus,
    next: AtomicU64,
    queue: mpsc::UnboundedSender<Work>,
    /// Jobs queued or running, oldest first.
    pending: Arc<Mutex<Vec<Pending>>>,
}

/// A queued or running job: the switch that cancels it, and whether its work has begun. One
/// cancelled before it begins leaves the queue at once; the worker then skips it.
struct Pending {
    id: String,
    cancel: watch::Sender<bool>,
    started: bool,
}

/// How long a cancelled command gets to exit after SIGTERM before its group is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

fn cancelled(job_id: &str) -> YardError {
    YardError::new(ErrorCode::JobCancelled, format!("{job_id} was cancelled"))
}

impl Jobs {
    /// Spawns the worker, so it needs a tokio runtime.
    pub fn new(bus: EventBus) -> Jobs {
        let (queue, mut work) = mpsc::unbounded_channel::<Work>();
        tokio::spawn(async move {
            while let Some(job) = work.recv().await {
                job.await;
            }
        });
        Jobs { bus, next: AtomicU64::new(1), queue, pending: Arc::default() }
    }

    /// Queues `work` and returns its id at once. A job that has to wait says for which.
    pub fn start<F, Fut>(&self, kind: &str, work: F) -> JobStarted
    where
        F: FnOnce(JobCtx) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), YardError>> + Send + 'static,
    {
        let job_id = format!("{kind}-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (cancel, cancelled_rx) = watch::channel(false);
        let result: Arc<Mutex<Option<serde_json::Value>>> = Arc::default();
        let ctx = JobCtx {
            job_id: job_id.clone(),
            bus: self.bus.clone(),
            cancel: cancelled_rx,
            result: result.clone(),
        };
        let ahead = {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            let ahead = pending.last().map(|p| p.id.clone());
            pending.push(Pending { id: job_id.clone(), cancel, started: false });
            ahead
        };
        if let Some(ahead) = ahead {
            ctx.line(format!("waiting for {ahead}"));
        }
        let (bus, pending) = (self.bus.clone(), self.pending.clone());
        let id = job_id.clone();
        let job: Work = Box::pin(async move {
            // Cancelled while it waited: it already finished, and never starts.
            {
                let mut queued = pending.lock().unwrap_or_else(|p| p.into_inner());
                match queued.iter_mut().find(|p| p.id == id) {
                    Some(p) => p.started = true,
                    None => return,
                }
            }
            let error = work(ctx).await.err();
            pending.lock().unwrap_or_else(|p| p.into_inner()).retain(|p| p.id != id);
            let result = match error {
                None => result.lock().unwrap_or_else(|p| p.into_inner()).take(),
                Some(_) => None,
            };
            bus.emit(Event::JobFinished { job_id: id, error, result });
        });
        if self.queue.send(job).is_err() {
            // The worker is gone only when the runtime is shutting down.
            self.pending.lock().unwrap_or_else(|p| p.into_inner()).retain(|p| p.id != job_id);
            let error = Some(YardError::internal("yardd is shutting down"));
            self.bus.emit(Event::JobFinished { job_id: job_id.clone(), error, result: None });
        }
        JobStarted { job_id }
    }

    /// Cancels a queued or running job. A queued one finishes `JOB_CANCELLED` at once, without
    /// waiting for the jobs ahead of it, and never runs; a running one has its command's process
    /// group stopped, and finishes `JOB_CANCELLED` once its work has cleaned up after itself.
    pub fn cancel(&self, job_id: &str) -> Result<(), YardError> {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        let Some(at) = pending.iter().position(|p| p.id == job_id) else {
            return Err(YardError::new(ErrorCode::JobNotFound, format!("no queued or running job {job_id}")));
        };
        pending[at].cancel.send_replace(true);
        if !pending[at].started {
            pending.remove(at);
            drop(pending);
            let error = Some(cancelled(job_id));
            self.bus.emit(Event::JobFinished { job_id: job_id.to_string(), error, result: None });
        }
        Ok(())
    }
}

/// What a job's work uses to report and to run commands.
#[derive(Clone)]
pub struct JobCtx {
    job_id: String,
    bus: EventBus,
    cancel: watch::Receiver<bool>,
    /// What the job made, sent with `job_finished` if it succeeds.
    result: Arc<Mutex<Option<serde_json::Value>>>,
}

impl JobCtx {
    pub fn id(&self) -> &str {
        &self.job_id
    }

    /// `JOB_CANCELLED` once a cancel was requested: a job waiting on something other than a
    /// command checks this between polls.
    pub fn check_cancelled(&self) -> Result<(), YardError> {
        if *self.cancel.borrow() { Err(cancelled(&self.job_id)) } else { Ok(()) }
    }

    /// What a successful job hands back in `JobFinished.result`; the last value set wins.
    /// A failed job sends none.
    pub fn set_result(&self, value: serde_json::Value) {
        *self.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(value);
    }

    pub fn line(&self, text: impl Into<String>) {
        self.bus.emit(Event::JobProgress { job_id: self.job_id.clone(), line: text.into() });
    }

    /// Runs `program`, which must be absolute so nothing is found through PATH (Herd's
    /// `php` comes first on it), streaming its stdout and stderr as progress. `PATH` is
    /// `env`'s when given, else the inherited one without Herd's directories.
    pub async fn run(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
    ) -> Result<(), YardError> {
        let (shown, ended, Output { lines, .. }) = self.exec(program, args, env, true).await?;
        let Err(ended) = ended else { return Ok(()) };
        let tail: Vec<&String> = lines.iter().skip(lines.len().saturating_sub(TAIL)).collect();
        Err(YardError::new(ErrorCode::JobFailed, format!("{shown} exited with {ended}"))
            .with_data(serde_json::json!({ "tail": tail })))
    }

    /// Runs `program` as `run` does, without streaming, and returns its stdout and stderr
    /// lines (interleaved as they arrived) and whether it exited 0.
    pub async fn capture(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
    ) -> Result<(bool, Vec<String>), YardError> {
        let (_, success, Output { lines, .. }) = self.exec(program, args, env, false).await?;
        Ok((success.is_ok(), lines))
    }

    /// As [`capture`](Self::capture), with stdout's lines apart too, for output to parse that
    /// stderr's log lines must not mix into.
    pub async fn capture_stdout(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
    ) -> Result<(bool, Output), YardError> {
        let (_, success, output) = self.exec(program, args, env, false).await?;
        Ok((success.is_ok(), output))
    }

    /// The command as shown, `Err(how it ended)` unless it exited 0, and its output.
    async fn exec(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
        stream: bool,
    ) -> Result<(String, Result<(), String>, Output), YardError> {
        if !program.is_absolute() {
            return Err(YardError::internal(format!("{} is not an absolute path", program.display())));
        }
        let mut cancel = self.cancel.clone();
        if *cancel.borrow_and_update() {
            return Err(cancelled(&self.job_id));
        }
        let mut cmd = Command::new(program);
        // Its own process group, so a cancel stops what it started too (brew's curl, make).
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        cmd.env("PATH", child_path(None, &inherited));
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
        // SAFETY: the hook makes only async-signal-safe calls.
        unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
        crate::fds::settle_before_fork();
        let name = program
            .file_name()
            .map_or_else(|| program.display().to_string(), |n| n.to_string_lossy().into_owned());
        let shown = format!("{name} {}", args.join(" "));
        let mut child = cmd
            .spawn()
            .map_err(|e| YardError::new(ErrorCode::JobFailed, format!("cannot run {shown}: {e}")))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<(bool, String)>();
        let readers = [
            child.stdout.take().map(|o| tokio::spawn(forward(o, true, tx.clone()))),
            child.stderr.take().map(|e| tokio::spawn(forward(e, false, tx.clone()))),
        ];
        drop(tx);
        let mut output = Output::default();
        let was_cancelled = loop {
            tokio::select! {
                line = rx.recv() => match line {
                    Some((from_stdout, line)) => {
                        if stream {
                            self.line(line.clone());
                        }
                        if from_stdout {
                            output.stdout.push(line.clone());
                        }
                        output.lines.push(line);
                    }
                    None => break false,
                },
                () = requested(&mut cancel) => break true,
            }
        };
        if was_cancelled {
            stop_group(&mut child).await;
            for reader in readers.into_iter().flatten() {
                reader.abort();
            }
            return Err(cancelled(&self.job_id));
        }
        for reader in readers.into_iter().flatten() {
            let _ = reader.await;
        }
        let status = child.wait().await.map_err(YardError::internal)?;
        let ended = if status.success() {
            Ok(())
        } else {
            Err(status.code().map_or_else(|| "a signal".to_string(), |c| c.to_string()))
        };
        Ok((shown, ended, output))
    }
}

/// What a command printed: every line as it arrived, and stdout's alone.
#[derive(Debug, Default)]
pub struct Output {
    pub lines: Vec<String>,
    pub stdout: Vec<String>,
}

/// Resolves once a cancel is requested; never, if the job can no longer be cancelled.
async fn requested(cancel: &mut watch::Receiver<bool>) {
    while !*cancel.borrow_and_update() {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// SIGTERM to the child's process group, then SIGKILL if it outlives the grace period.
async fn stop_group(child: &mut tokio::process::Child) {
    let Some(pid) = child.id() else { return };
    let group = pid as libc::pid_t;
    // SAFETY: signalling a process group we created has no memory-safety preconditions.
    unsafe { libc::killpg(group, libc::SIGTERM) };
    if tokio::time::timeout(CANCEL_GRACE, child.wait()).await.is_err() {
        // SAFETY: as above.
        unsafe { libc::killpg(group, libc::SIGKILL) };
        let _ = child.wait().await;
    }
}

async fn forward(pipe: impl AsyncRead + Unpin, stdout: bool, tx: mpsc::UnboundedSender<(bool, String)>) {
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stdout, line)).is_err() {
            return;
        }
    }
}
