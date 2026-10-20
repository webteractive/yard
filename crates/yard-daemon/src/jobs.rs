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
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use yard_core::shim::child_path;
use yard_proto::{ErrorCode, Event, JobStarted, YardError};

/// Lines kept for a failed command's `data.tail`.
const TAIL: usize = 20;

type Work = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct Jobs {
    bus: EventBus,
    next: AtomicU64,
    queue: mpsc::UnboundedSender<Work>,
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
        Jobs { bus, next: AtomicU64::new(1), queue }
    }

    /// Queues `work` and returns its id at once.
    pub fn start<F, Fut>(&self, kind: &str, work: F) -> JobStarted
    where
        F: FnOnce(JobCtx) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), YardError>> + Send + 'static,
    {
        let job_id = format!("{kind}-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let ctx = JobCtx { job_id: job_id.clone(), bus: self.bus.clone() };
        let bus = self.bus.clone();
        let id = job_id.clone();
        let job: Work = Box::pin(async move {
            let error = work(ctx).await.err();
            bus.emit(Event::JobFinished { job_id: id, error });
        });
        if self.queue.send(job).is_err() {
            // The worker is gone only when the runtime is shutting down.
            let error = Some(YardError::internal("yardd is shutting down"));
            self.bus.emit(Event::JobFinished { job_id: job_id.clone(), error });
        }
        JobStarted { job_id }
    }
}

/// What a job's work uses to report and to run commands.
#[derive(Clone)]
pub struct JobCtx {
    job_id: String,
    bus: EventBus,
}

impl JobCtx {
    pub fn id(&self) -> &str {
        &self.job_id
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
        let (shown, ended, lines) = self.exec(program, args, env, true).await?;
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
        let (_, success, lines) = self.exec(program, args, env, false).await?;
        Ok((success.is_ok(), lines))
    }

    /// The command as shown, `Err(how it ended)` unless it exited 0, and its output lines.
    async fn exec(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
        stream: bool,
    ) -> Result<(String, Result<(), String>, Vec<String>), YardError> {
        if !program.is_absolute() {
            return Err(YardError::internal(format!("{} is not an absolute path", program.display())));
        }
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
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

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let readers = [
            child.stdout.take().map(|o| tokio::spawn(forward(o, tx.clone()))),
            child.stderr.take().map(|e| tokio::spawn(forward(e, tx.clone()))),
        ];
        drop(tx);
        let mut lines = vec![];
        while let Some(line) = rx.recv().await {
            if stream {
                self.line(line.clone());
            }
            lines.push(line);
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
        Ok((shown, ended, lines))
    }
}

async fn forward(pipe: impl AsyncRead + Unpin, tx: mpsc::UnboundedSender<String>) {
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send(line).is_err() {
            return;
        }
    }
}
