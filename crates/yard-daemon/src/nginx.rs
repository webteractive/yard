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
}

impl NginxCommand {
    pub fn new(paths: &Paths, brew: &Brew) -> Self {
        Self {
            bin: brew.nginx_bin(),
            prefix: paths.nginx_dir(),
            conf: paths.nginx_conf(),
            error_log: paths.logs_dir().join("nginx-error.log"),
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

    /// `nginx -t`: `Ok(Err(..))` when nginx rejects the config, `Err` when it cannot run.
    /// The test parses and opens files but binds nothing, so it needs no sockets.
    pub async fn test(&self) -> Result<Result<(), String>, YardError> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(self.args()).arg("-t").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        // SAFETY: the hook makes only async-signal-safe calls.
        unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
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
