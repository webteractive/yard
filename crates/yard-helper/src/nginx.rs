//! Running brew's nginx as root. Every invocation uses the same fixed flags; the only
//! thing read from the user-writable data root is the pid, and it is verified before
//! any signal (rule: verify before signalling).

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_proto::helper::TestConfigResult;
use yard_proto::{ErrorCode, YardError};

const STOP_GRACE: Duration = Duration::from_secs(5);

pub struct Nginx {
    bin: PathBuf,
    prefix: PathBuf,
    conf: PathBuf,
    error_log: PathBuf,
    pid_file: PathBuf,
}

impl Nginx {
    pub fn new(paths: &Paths, brew: &Brew) -> Self {
        Self {
            bin: brew.nginx_bin(),
            prefix: paths.nginx_dir(),
            conf: paths.nginx_conf(),
            error_log: paths.logs_dir().join("nginx-error.log"),
            pid_file: paths.nginx_pid(),
        }
    }

    /// `nginx -p <root>/nginx/ -c <root>/nginx/nginx.conf -e <root>/logs/nginx-error.log`.
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("-p")
            .arg(format!("{}/", self.prefix.display()))
            .arg("-c")
            .arg(&self.conf)
            .arg("-e")
            .arg(&self.error_log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        cmd
    }

    /// No-op when a verified nginx is already running; otherwise nginx daemonizes.
    pub async fn start(&self) -> Result<(), YardError> {
        if self.running_pid().is_some() {
            return Ok(());
        }
        let (status, stderr) = run(self.command(), &self.bin).await?;
        if status.success() {
            Ok(())
        } else {
            Err(YardError::new(ErrorCode::HelperRefused, format!("nginx did not start: {}", stderr.trim()))
                .with_hint(format!("see {}", self.error_log.display())))
        }
    }

    pub async fn test_config(&self) -> Result<TestConfigResult, YardError> {
        let mut cmd = self.command();
        cmd.arg("-t");
        let (status, output) = run(cmd, &self.bin).await?;
        Ok(TestConfigResult { ok: status.success(), output })
    }

    pub fn reload(&self) -> Result<(), YardError> {
        let pid = self.signallable_pid()?.ok_or_else(|| {
            YardError::new(ErrorCode::HelperRefused, "nginx is not running, so there is nothing to reload")
        })?;
        signal(pid, libc::SIGHUP)
    }

    /// SIGQUIT (graceful), then SIGTERM after five seconds. Already stopped is fine.
    pub async fn stop(&self) -> Result<(), YardError> {
        let Some(pid) = self.signallable_pid()? else { return Ok(()) };
        signal(pid, libc::SIGQUIT)?;
        let deadline = Instant::now() + STOP_GRACE;
        while alive(pid) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if alive(pid) && self.is_nginx(pid) {
            signal(pid, libc::SIGTERM)?;
        }
        Ok(())
    }

    /// The pid file's pid when it is alive and is this nginx binary.
    pub fn running_pid(&self) -> Option<u32> {
        self.read_pid().filter(|pid| alive(*pid) && self.is_nginx(*pid))
    }

    /// `None` when nothing is running; `HELPER_REFUSED` when the pid file names a live
    /// process that is not nginx, in which case nothing is signalled.
    fn signallable_pid(&self) -> Result<Option<u32>, YardError> {
        let Some(pid) = self.read_pid().filter(|pid| alive(*pid)) else { return Ok(None) };
        if self.is_nginx(pid) {
            return Ok(Some(pid));
        }
        let path =
            pid_path(pid).map_or_else(|| "an unknown executable".to_string(), |p| p.display().to_string());
        Err(YardError::new(
            ErrorCode::HelperRefused,
            format!(
                "{} names pid {pid} ({path}), which is not nginx; nothing was signalled",
                self.pid_file.display()
            ),
        )
        .with_hint(format!("delete {} if nginx is not running", self.pid_file.display())))
    }

    fn read_pid(&self) -> Option<u32> {
        std::fs::read_to_string(&self.pid_file).ok()?.trim().parse().ok().filter(|pid| *pid > 1)
    }

    fn is_nginx(&self, pid: u32) -> bool {
        match (pid_path(pid), std::fs::canonicalize(&self.bin)) {
            (Some(running), Ok(bin)) => running == bin,
            _ => false,
        }
    }
}

/// Waits for the process to exit, then collects whatever it wrote to stderr. A
/// daemonizing nginx can leave its child holding the pipe, so the read is bounded.
async fn run(mut cmd: Command, bin: &Path) -> Result<(ExitStatus, String), YardError> {
    let mut child = cmd.spawn().map_err(|e| {
        YardError::new(ErrorCode::HelperRefused, format!("cannot run {}: {e}", bin.display()))
            .with_hint("brew install nginx")
    })?;
    let mut stderr = child.stderr.take();
    let status = child.wait().await.map_err(YardError::internal)?;
    let mut output = Vec::new();
    if let Some(pipe) = stderr.as_mut() {
        let _ = tokio::time::timeout(Duration::from_millis(500), pipe.read_to_end(&mut output)).await;
    }
    Ok((status, String::from_utf8_lossy(&output).into_owned()))
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else { return false };
    // SAFETY: signal 0 only checks that the pid exists and may be signalled.
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

fn signal(pid: u32, sig: libc::c_int) -> Result<(), YardError> {
    let raw = i32::try_from(pid).map_err(YardError::internal)?;
    // SAFETY: plain kill(2) on a pid the caller verified is nginx.
    if unsafe { libc::kill(raw, sig) } == 0 {
        Ok(())
    } else {
        Err(YardError::internal(format!(
            "cannot signal nginx (pid {pid}): {}",
            std::io::Error::last_os_error()
        )))
    }
}

fn pid_path(pid: u32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the length passed, and proc_pidpath writes at most that.
    let n =
        unsafe { libc::proc_pidpath(i32::try_from(pid).ok()?, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok().map(PathBuf::from)
}
