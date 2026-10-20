//! How yardd runs brew's nginx: always with the same fixed flags, as the user. The
//! supervisor runs it in the foreground (`daemon off;` in nginx.conf) and reloads it
//! with SIGHUP.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_proto::{ErrorCode, YardError};

#[derive(Debug, Clone)]
pub struct NginxCommand {
    bin: PathBuf,
    prefix: PathBuf,
    conf: PathBuf,
    error_log: PathBuf,
    pid_file: PathBuf,
}

/// How long an orphan gets to finish its requests after SIGQUIT.
const ORPHAN_GRACE: Duration = Duration::from_secs(5);

impl NginxCommand {
    pub fn new(paths: &Paths, brew: &Brew) -> Self {
        Self {
            bin: brew.nginx_bin(),
            prefix: paths.nginx_dir(),
            conf: paths.nginx_conf(),
            error_log: paths.logs_dir().join("nginx-error.log"),
            pid_file: paths.nginx_pid(),
        }
    }

    pub fn program(&self) -> &Path {
        &self.bin
    }

    /// `-p <root>/nginx/ -c <root>/nginx/nginx.conf -e <root>/logs/nginx-error.log`.
    pub fn args(&self) -> Vec<OsString> {
        vec![
            "-p".into(),
            format!("{}/", self.prefix.display()).into(),
            "-c".into(),
            self.conf.clone().into(),
            "-e".into(),
            self.error_log.clone().into(),
        ]
    }

    /// Stops an nginx an earlier yardd left running when it was killed: the master in
    /// `run/nginx.pid`, if it is still alive, runs brew's nginx binary, and has been
    /// handed to launchd (parent 1). It would otherwise hold the port the new nginx needs.
    /// SIGQUIT lets it finish its requests; SIGTERM follows after `ORPHAN_GRACE`.
    /// Returns the pid it stopped.
    pub async fn stop_orphan(&self) -> Option<u32> {
        let pid: u32 = std::fs::read_to_string(&self.pid_file).ok()?.trim().parse().ok()?;
        let binary = std::fs::canonicalize(&self.bin).ok()?;
        let ours = crate::holder::pid_path(pid).is_some_and(|p| Path::new(&p) == binary);
        if !ours || crate::holder::parent_of(pid) != Some(1) {
            return None;
        }
        let raw = i32::try_from(pid).ok()?;
        // SAFETY: kill(2) on a pid just checked to be an orphaned brew nginx.
        unsafe { libc::kill(raw, libc::SIGQUIT) };
        let deadline = tokio::time::Instant::now() + ORPHAN_GRACE;
        // SAFETY: signal 0 only checks that the pid exists.
        while unsafe { libc::kill(raw, 0) } == 0 {
            if tokio::time::Instant::now() >= deadline {
                // SAFETY: as above.
                unsafe { libc::kill(raw, libc::SIGTERM) };
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Some(pid)
    }

    /// `nginx -t`: `Ok(Err(..))` when nginx rejects the config, `Err` when it cannot run.
    /// The test parses and opens files but binds nothing, so it needs no sockets.
    pub async fn test(&self) -> Result<Result<(), String>, YardError> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(self.args()).arg("-t").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        // SAFETY: the hook makes only async-signal-safe calls.
        unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
        crate::fds::settle_before_fork();
        let mut child = cmd.spawn().map_err(|e| {
            YardError::new(ErrorCode::FormulaMissing, format!("cannot run {}: {e}", self.bin.display()))
                .with_hint("brew install nginx")
        })?;
        let mut stderr = child.stderr.take();
        let status = child.wait().await.map_err(YardError::internal)?;
        let mut output = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = tokio::time::timeout(Duration::from_millis(500), pipe.read_to_end(&mut output)).await;
        }
        let output = String::from_utf8_lossy(&output).into_owned();
        Ok(if status.success() { Ok(()) } else { Err(output) })
    }
}
