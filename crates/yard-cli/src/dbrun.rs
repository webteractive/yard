//! Running a dump or an import that `db.command` answered (db spec §6.5, §6.6): the tool,
//! gzip, the statement filter and the file, with progress and Ctrl-C. The CLI runs it, not
//! yardd, so the file is opened by the user's own process.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use yard_core::db::filter::{Filter, Switch, Verb};
use yard_core::db::flags::DumpFormat;
use yard_core::present::bytes_text as human;
use yard_proto::{DbCommand, ErrorCode, ImportKind, YardError};

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
    // Its own process group: a terminal's Ctrl-C reaches only this process, whose handler
    // stops the tool and reports a cancel rather than the tool's death.
    c.stderr(Stdio::piped()).kill_on_drop(true).process_group(0);
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
                    .process_group(0)
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
        // A fresh file, never through a symlink someone left at the name; the dump's data is
        // the user's alone.
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(partial)
            .await
            .map_err(|e| {
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

/// How an import ended when it was not cancelled.
pub struct Imported {
    pub bytes: u64,
    pub skipped: usize,
}

/// Feeds `file` to the tool through `filter` (db spec §6.6). A directory goes to the tool as
/// the path the command already names. A switching statement closes the tool's input before
/// it is sent, and a tool that fails leaves its last lines.
pub async fn import(
    cmd: &DbCommand,
    file: &Path,
    kind: ImportKind,
    gzip: bool,
    mut filter: Filter,
    label: &str,
    progress: Progress,
) -> Result<Ran<Imported>, YardError> {
    let name = program_name(cmd);
    if kind == ImportKind::Directory {
        let mut child = spawn(tool(cmd, &[]).stdin(Stdio::null()).stdout(Stdio::null()), &name)?;
        let stderr = collect(child.stderr.take());
        return tokio::select! {
            status = child.wait() => {
                let status = status.map_err(|e| YardError::internal(format!("{name}: {e}")))?;
                check(&name, status.success(), stderr.await)?;
                Ok(Ran::Done(Imported { bytes: dir_size(file), skipped: 0 }))
            }
            _ = tokio::signal::ctrl_c() => {
                stop(&mut child).await;
                Ok(Ran::Cancelled)
            }
        };
    }
    let mut child = spawn(tool(cmd, &[]).stdin(Stdio::piped()).stdout(Stdio::null()), &name)?;
    let stderr = collect(child.stderr.take());
    let mut stdin = child.stdin.take().ok_or_else(|| YardError::internal("no stdin"))?;
    let mut gunzip = None;
    let (reader, total): (Box<dyn AsyncRead + Unpin + Send>, Option<u64>) = if gzip {
        let mut g = spawn(
            Command::new(GZIP)
                .process_group(0)
                .arg("-dc")
                .arg(file)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true),
            "gzip",
        )?;
        let out = g.stdout.take().ok_or_else(|| YardError::internal("no gzip stdout"))?;
        gunzip = Some(g);
        // gzip reads the file itself, so progress counts what it unpacked, with no total.
        (Box::new(out), None)
    } else {
        let f = tokio::fs::File::open(file).await.map_err(|e| {
            YardError::new(ErrorCode::InvalidArgument, format!("cannot read {}: {e}", file.display()))
        })?;
        let size = f.metadata().await.ok().map(|m| m.len());
        (Box::new(f), size)
    };
    let feed = async {
        let mut reader = reader;
        let mut buf = vec![0u8; CHUNK];
        let mut bytes = 0u64;
        let mut shown = Instant::now();
        let mut switched: Option<Switch> = None;
        loop {
            let n = reader.read(&mut buf).await.map_err(|e| YardError::internal(format!("reading: {e}")))?;
            let (chunk, end) = if n == 0 { (filter.finish(), true) } else { (filter.push(&buf[..n]), false) };
            match chunk {
                Ok(out) => {
                    // The tool may have stopped reading because it failed; its status says why.
                    if stdin.write_all(&out).await.is_err() {
                        break;
                    }
                }
                Err(s) => {
                    switched = Some(s);
                    break;
                }
            }
            if end {
                break;
            }
            bytes += n as u64;
            if shown.elapsed() >= TICK {
                progress.show(label, bytes, total);
                shown = Instant::now();
            }
        }
        progress.show(label, bytes, total);
        drop(stdin);
        // Waited for inside the select too: once Ctrl-C has a handler it no longer kills us.
        let status = child.wait().await.map_err(|e| YardError::internal(format!("{name}: {e}")))?;
        Ok::<_, YardError>((bytes, switched, status))
    };
    tokio::select! {
        fed = feed => {
            progress.done();
            let (bytes, switched, status) = fed?;
            if let Some(s) = switched {
                if let Some(mut g) = gunzip {
                    stop(&mut g).await;
                }
                return Err(switch_error(&s, label));
            }
            // gzip read the whole file: a truncated or corrupt one fails here, even when the
            // tool took the prefix it got as a complete dump.
            if let Some(mut g) = gunzip {
                let gz_err = collect(g.stderr.take());
                let status = g.wait().await.map_err(|e| YardError::internal(format!("gzip: {e}")))?;
                check("gzip", status.success(), gz_err.await).map_err(|e| {
                    let what = format!("{}; what ran before stays in {label} (yard db shell {label} to look)", e.message);
                    YardError { message: what, ..e }
                })?;
            }
            check(&name, status.success(), stderr.await).map_err(|e| {
                let what = format!("{}; what ran before the error stays in {label} (yard db shell {label} to look)", e.message);
                YardError { message: what, ..e }
            })?;
            Ok(Ran::Done(Imported { bytes, skipped: filter.skipped() }))
        }
        _ = tokio::signal::ctrl_c() => {
            progress.done();
            stop(&mut child).await;
            if let Some(g) = gunzip.as_mut() {
                stop(g).await;
            }
            Ok(Ran::Cancelled)
        }
    }
}

fn switch_error(s: &Switch, label: &str) -> YardError {
    match &s.database {
        // Dropping a database is refused even when it is the import's own.
        Some(db) if s.verb == Verb::Drop => YardError::new(
            ErrorCode::DbDumpSwitchesDatabase,
            format!("the dump drops database {db} ({}); nothing from there on ran", s.statement),
        )
        .with_hint("dump again without --add-drop-database (mysqldump) or --clean (pg_dump)")
        .with_data(serde_json::json!({ "database": db })),
        Some(db) => YardError::new(
            ErrorCode::DbDumpSwitchesDatabase,
            format!("the dump switches to database {db} ({}); nothing from there on ran", s.statement),
        )
        .with_hint(format!(
            "yard db import {db} <file>, or dump again without --databases (mysqldump) or -C (pg_dump)"
        ))
        .with_data(serde_json::json!({ "database": db })),
        None => YardError::new(
            ErrorCode::DbDumpSwitchesDatabase,
            format!("the dump runs a client command ({}), which an import into {label} does not; nothing from there on ran", s.statement),
        )
        .with_hint("remove that line from the dump"),
    }
}

/// The first bytes of `file` as the tool will see them (unpacked when gzipped), and whether
/// it is gzipped. A missing, unreadable or empty file is refused before yardd is asked.
pub async fn head(file: &Path) -> Result<(Vec<u8>, bool), YardError> {
    let refused = |why: String| YardError::new(ErrorCode::InvalidArgument, why);
    let meta =
        std::fs::metadata(file).map_err(|e| refused(format!("cannot read {}: {e}", file.display())))?;
    if meta.is_dir() {
        return Ok((vec![], false));
    }
    if meta.len() == 0 {
        return Err(refused(format!("{} is empty", file.display())));
    }
    let mut f = tokio::fs::File::open(file)
        .await
        .map_err(|e| refused(format!("cannot read {}: {e}", file.display())))?;
    let mut first = vec![0u8; 512];
    let n = f.read(&mut first).await.map_err(|e| refused(format!("cannot read {}: {e}", file.display())))?;
    first.truncate(n);
    if !yard_core::db::detect::is_gzip(&first) {
        return Ok((first, false));
    }
    let mut g = spawn(
        Command::new(GZIP)
            .process_group(0)
            .arg("-dc")
            .arg(file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true),
        "gzip",
    )?;
    let mut out = g.stdout.take().ok_or_else(|| YardError::internal("no gzip stdout"))?;
    let mut unpacked = Vec::new();
    let mut buf = [0u8; 512];
    while unpacked.len() < 512 {
        let n = out.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        unpacked.extend_from_slice(&buf[..n]);
    }
    stop(&mut g).await;
    if unpacked.is_empty() {
        return Err(refused(format!("{} is gzipped but holds nothing gzip can unpack", file.display())));
    }
    unpacked.truncate(512);
    Ok((unpacked, true))
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

/// Removes a file, a symlink (never what it points at) or a directory.
pub fn remove(path: &Path) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir()) {
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
