//! Running a dump or an import that `db.command` answered (db spec §6.5, §6.6): the tool,
//! gzip, the statement filter and the file, with progress and Ctrl-C. The CLI runs it, not
//! yardd, so the file is opened by the user's own process.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use yard_core::db::flags::DumpFormat;
use yard_proto::{DbCommand, ErrorCode, YardError};

pub const GZIP: &str = "/usr/bin/gzip";
const CHUNK: usize = 1 << 16;
const TICK: Duration = Duration::from_millis(250);

/// How progress is shown on stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// One line rewritten in place.
    Tty,
    /// `{"bytes":N,"total":M}` lines, for Yard.app.
    Json,
    Off,
}

impl Progress {
    pub fn pick(json_lines: bool) -> Progress {
        if json_lines {
            Progress::Json
        } else if std::io::stderr().is_terminal() {
            Progress::Tty
        } else {
            Progress::Off
        }
    }

    fn show(self, label: &str, bytes: u64, total: Option<u64>) {
        match self {
            Progress::Tty => match total {
                Some(t) => eprint!("\r{label}: {} of {}   ", human(bytes), human(t)),
                None => eprint!("\r{label}: {} written   ", human(bytes)),
            },
            Progress::Json => eprintln!("{}", serde_json::json!({ "bytes": bytes, "total": total })),
            Progress::Off => {}
        }
    }

    fn done(self) {
        if self == Progress::Tty {
            eprintln!();
        }
    }
}

/// How a run ended when it did not fail.
pub enum Ran<T> {
    Done(T),
    Cancelled,
}

/// `<name>-<stamp>` with the extension the format and `--compress` give (db spec §6.5);
/// a directory for `-Fd`.
pub fn default_dump_name(name: &str, format: DumpFormat, compress: bool, stamp: &str) -> String {
    let ext = match (format, compress) {
        (DumpFormat::Plain, false) => ".sql",
        (DumpFormat::Plain, true) => ".sql.gz",
        (DumpFormat::Custom, _) => ".dump",
        (DumpFormat::Tar, _) => ".tar",
        (DumpFormat::Directory, _) => "",
    };
    format!("{name}-{stamp}{ext}")
}

/// The local time as `YYYY-MM-DD-HHMM`.
pub fn local_stamp() -> String {
    // SAFETY: localtime_r writes into the tm we own; time(NULL) has no side effects.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

pub fn partial_of(out: &Path) -> PathBuf {
    PathBuf::from(format!("{}.partial", out.display()))
}

/// The tool as `cmd` says, with its stderr kept for the failure message.
fn tool(cmd: &DbCommand, extra: &[String]) -> Command {
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args).args(extra);
    for var in &cmd.env_remove {
        c.env_remove(var);
    }
    c.envs(cmd.env.iter().map(|(k, v)| (k, v)));
    c.stderr(Stdio::piped()).kill_on_drop(true);
    c
}

fn program_name(cmd: &DbCommand) -> String {
    Path::new(&cmd.program).file_name().map_or(cmd.program.clone(), |n| n.to_string_lossy().into_owned())
}

/// Runs the dump into `<out>.partial` and renames it onto `out` when the tool (and gzip)
/// exit 0. A failure or Ctrl-C removes the partial file.
pub async fn dump(
    cmd: &DbCommand,
    out: &Path,
    compress: bool,
    directory: bool,
    label: &str,
    progress: Progress,
) -> Result<Ran<u64>, YardError> {
    let partial = partial_of(out);
    remove(&partial);
    let result = run_dump(cmd, &partial, compress, directory, label, progress).await;
    progress.done();
    match result {
        Ok(Ran::Done(bytes)) => {
            remove(out);
            std::fs::rename(&partial, out).map_err(|e| {
                remove(&partial);
                YardError::new(ErrorCode::Internal, format!("cannot move the dump to {}: {e}", out.display()))
            })?;
            Ok(Ran::Done(bytes))
        }
        other => {
            remove(&partial);
            other
        }
    }
}

async fn run_dump(
    cmd: &DbCommand,
    partial: &Path,
    compress: bool,
    directory: bool,
    label: &str,
    progress: Progress,
) -> Result<Ran<u64>, YardError> {
    let name = program_name(cmd);
    if directory {
        let mut child = spawn(
            tool(cmd, &["-f".into(), partial.display().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null()),
            &name,
        )?;
        let stderr = collect(child.stderr.take());
        tokio::select! {
            status = child.wait() => {
                let status = status.map_err(|e| YardError::internal(format!("{name}: {e}")))?;
                check(&name, status.success(), stderr.await)?;
                Ok(Ran::Done(dir_size(partial)))
            }
            _ = tokio::signal::ctrl_c() => {
                stop(&mut child).await;
                Ok(Ran::Cancelled)
            }
        }
    } else {
        let mut child = spawn(tool(cmd, &[]).stdin(Stdio::null()).stdout(Stdio::piped()), &name)?;
        let stderr = collect(child.stderr.take());
        let stdout = child.stdout.take().ok_or_else(|| YardError::internal("no stdout"))?;
        let mut gzip = None;
        let reader: Box<dyn AsyncRead + Unpin + Send> = if compress {
            let feed: Stdio = stdout.try_into().map_err(|e| YardError::internal(format!("gzip: {e}")))?;
            let mut g = spawn(
                Command::new(GZIP)
                    .arg("-c")
                    .stdin(feed)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true),
                "gzip",
            )?;
            let out = g.stdout.take().ok_or_else(|| YardError::internal("no gzip stdout"))?;
            gzip = Some(g);
            Box::new(out)
        } else {
            Box::new(stdout)
        };
        let mut file = tokio::fs::File::create(partial).await.map_err(|e| {
            YardError::new(ErrorCode::InvalidArgument, format!("cannot write {}: {e}", partial.display()))
        })?;
        let copy = pump(reader, &mut file, label, None, progress);
        tokio::select! {
            bytes = copy => {
                let bytes = bytes?;
                file.flush().await.map_err(|e| YardError::internal(format!("cannot write the dump: {e}")))?;
                let status = child.wait().await.map_err(|e| YardError::internal(format!("{name}: {e}")))?;
                check(&name, status.success(), stderr.await)?;
                if let Some(mut g) = gzip {
                    let gz_err = collect(g.stderr.take());
                    let status = g.wait().await.map_err(|e| YardError::internal(format!("gzip: {e}")))?;
                    check("gzip", status.success(), gz_err.await)?;
                }
                Ok(Ran::Done(bytes))
            }
            _ = tokio::signal::ctrl_c() => {
                stop(&mut child).await;
                if let Some(g) = gzip.as_mut() {
                    stop(g).await;
                }
                Ok(Ran::Cancelled)
            }
        }
    }
}

/// Copies `reader` to `writer`, showing progress; the byte count.
pub async fn pump(
    mut reader: impl AsyncRead + Unpin,
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    label: &str,
    total: Option<u64>,
    progress: Progress,
) -> Result<u64, YardError> {
    let mut buf = vec![0u8; CHUNK];
    let mut bytes = 0u64;
    let mut shown = Instant::now();
    loop {
        let n = reader.read(&mut buf).await.map_err(|e| YardError::internal(format!("reading: {e}")))?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await.map_err(|e| YardError::internal(format!("writing: {e}")))?;
        bytes += n as u64;
        if shown.elapsed() >= TICK {
            progress.show(label, bytes, total);
            shown = Instant::now();
        }
    }
    progress.show(label, bytes, total);
    Ok(bytes)
}

pub fn spawn(cmd: &mut Command, name: &str) -> Result<Child, YardError> {
    cmd.spawn().map_err(|e| YardError::new(ErrorCode::DbToolFailed, format!("cannot run {name}: {e}")))
}

/// Reads a child's stderr to its end in the background.
pub fn collect(stderr: Option<tokio::process::ChildStderr>) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        let mut text = String::new();
        if let Some(mut s) = stderr {
            let _ = s.read_to_string(&mut text).await;
        }
        text
    })
}

/// The tool's failure, its last 20 lines printed and kept in `data.tail`.
pub fn check(
    name: &str,
    success: bool,
    stderr: Result<String, tokio::task::JoinError>,
) -> Result<(), YardError> {
    if success {
        return Ok(());
    }
    let text = stderr.unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: Vec<String> = lines[lines.len().saturating_sub(20)..].iter().map(|l| l.to_string()).collect();
    for line in &tail {
        eprintln!("  {line}");
    }
    Err(YardError::new(ErrorCode::DbToolFailed, format!("{name} failed"))
        .with_data(serde_json::json!({ "tail": tail })))
}

pub async fn stop(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

pub fn remove(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

pub fn dir_size(path: &Path) -> u64 {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.flatten().map(|e| dir_size(&e.path())).sum(),
        Err(_) => std::fs::metadata(path).map_or(0, |m| m.len()),
    }
}

/// `3.0 MB`, `80.0 KB`, `512 bytes`.
pub fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / (1u64 << 10) as f64),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names() {
        let s = "2026-11-15-1430";
        assert_eq!(default_dump_name("shop", DumpFormat::Plain, false, s), "shop-2026-11-15-1430.sql");
        assert_eq!(default_dump_name("shop", DumpFormat::Plain, true, s), "shop-2026-11-15-1430.sql.gz");
        assert_eq!(default_dump_name("app", DumpFormat::Custom, false, s), "app-2026-11-15-1430.dump");
        assert_eq!(default_dump_name("app", DumpFormat::Tar, false, s), "app-2026-11-15-1430.tar");
        assert_eq!(default_dump_name("app", DumpFormat::Directory, false, s), "app-2026-11-15-1430");
        assert_eq!(local_stamp().len(), 15);
    }
}
