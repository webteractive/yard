//! Running the `yard` CLI (M6 design §5.8): an argument vector, `--json`, a fixed PATH, and the
//! CLI's own exit codes; never a shell, never a hint's text.

use crate::bridge::Bridge;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use yard_client::not_running;
use yard_proto::{ErrorCode, YardError};

/// PATH for the CLI and anything it starts: nothing a shell profile added.
const PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub struct CliRunner {
    /// The `yard` to run; a new choice in Settings or onboarding replaces it at once.
    yard: RefCell<Option<PathBuf>>,
    pub root: PathBuf,
    pub root_is_default: bool,
    bridge: Arc<Bridge>,
}

#[derive(Deserialize)]
struct Failure {
    error: YardError,
}

impl CliRunner {
    pub fn new(
        yard: Option<PathBuf>,
        root: PathBuf,
        root_is_default: bool,
        bridge: Arc<Bridge>,
    ) -> CliRunner {
        CliRunner { yard: RefCell::new(yard), root, root_is_default, bridge }
    }

    pub fn yard(&self) -> Option<PathBuf> {
        self.yard.borrow().clone()
    }

    pub fn set_yard(&self, yard: Option<PathBuf>) {
        *self.yard.borrow_mut() = yard;
    }

    /// `yard [--root <root>] <args> --json`: exit 0 decodes `R`, exit 1 is the CLI's
    /// `{"error": YardError}`, exit 3 is yardd not running, anything else names stderr's last line.
    pub fn run<R: DeserializeOwned + Send + 'static>(
        &self,
        args: &[&str],
    ) -> impl Future<Output = Result<R, YardError>> + use<R> {
        let yard = self.yard();
        let mut argv: Vec<String> = vec![];
        if !self.root_is_default {
            argv.extend(["--root".to_string(), self.root.display().to_string()]);
        }
        argv.extend(args.iter().map(|a| a.to_string()));
        argv.push("--json".to_string());
        let shown = args.join(" ");
        let task = yard.map(|yard| {
            self.bridge.spawn(async move {
                tokio::process::Command::new(&yard)
                    .args(&argv)
                    .env("PATH", PATH)
                    .stdin(Stdio::null())
                    .output()
                    .await
            })
        });
        async move {
            let Some(task) = task else {
                return Err(not_found());
            };
            let out = task
                .await
                .map_err(YardError::internal)?
                .map_err(|e| YardError::internal(format!("yard {shown} did not start: {e}")))?;
            outcome(&shown, out.status.code(), &out.stdout, &last_line(&out.stderr))
        }
    }

    /// `yard [--root <root>] --json <args>` with its stderr read line by line as it is written
    /// (a dump's or an import's `--progress-json` lines), and stopped by [`CliChild::interrupt`],
    /// which the CLI takes as Ctrl-C (exit 130: `JOB_CANCELLED`). `--json` goes before `args`, so
    /// it never lands after their `--`.
    pub fn stream(&self, args: &[String]) -> Streaming {
        let mut argv: Vec<String> = vec![];
        if !self.root_is_default {
            argv.extend(["--root".to_string(), self.root.display().to_string()]);
        }
        argv.push("--json".to_string());
        argv.extend(args.iter().cloned());
        let shown = args.join(" ");
        let child = Arc::new(CliChild::default());
        let (tx, lines) = tokio::sync::mpsc::unbounded_channel();
        let task = self.yard().map(|yard| {
            let child = child.clone();
            self.bridge.spawn(async move {
                let mut c = tokio::process::Command::new(&yard)
                    .args(&argv)
                    .env("PATH", PATH)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                child.started(c.id());
                let (mut stdout, stderr) = (c.stdout.take(), c.stderr.take());
                let read_err = async move {
                    let mut last = String::new();
                    let Some(stderr) = stderr else { return last };
                    let mut reader = tokio::io::BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        if !line.trim().is_empty() && !line.trim_start().starts_with('{') {
                            last = line.trim().to_string();
                        }
                        let _ = tx.send(line);
                    }
                    last
                };
                let read_out = async move {
                    let mut buf = vec![];
                    if let Some(out) = stdout.as_mut() {
                        let _ = out.read_to_end(&mut buf).await;
                    }
                    buf
                };
                let (last, out) = tokio::join!(read_err, read_out);
                let status = c.wait().await;
                child.ended();
                status.map(|s| (s.code(), out, last))
            })
        });
        let interrupted = child.clone();
        let done = Box::pin(async move {
            let Some(task) = task else {
                return Err(not_found());
            };
            let (code, out, last) = task
                .await
                .map_err(YardError::internal)?
                .map_err(|e| YardError::internal(format!("yard {shown} did not start: {e}")))?;
            // 130 is the CLI's own Ctrl-C exit; a SIGINT before it took Ctrl-C ends it on the signal.
            if code == Some(130) || (code.is_none() && interrupted.interrupted()) {
                return Err(YardError::new(ErrorCode::JobCancelled, format!("yard {shown} was cancelled")));
            }
            outcome(&shown, code, &out, &last)
        });
        Streaming { child, lines, done }
    }
}

/// A `yard` the app started with [`CliRunner::stream`].
#[derive(Debug, Default)]
pub struct CliChild {
    /// Its process id while it runs; 0 before it starts and after it ends.
    pid: AtomicI32,
    interrupted: AtomicBool,
}

impl CliChild {
    /// Sends it SIGINT, at once or as soon as it starts.
    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
        Self::sigint(self.pid.load(Ordering::SeqCst));
    }

    pub fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    fn started(&self, pid: Option<u32>) {
        let pid = pid.and_then(|p| i32::try_from(p).ok()).unwrap_or(0);
        self.pid.store(pid, Ordering::SeqCst);
        if self.interrupted() {
            Self::sigint(pid);
        }
    }

    fn ended(&self) {
        self.pid.store(0, Ordering::SeqCst);
    }

    fn sigint(pid: i32) {
        if pid > 0 {
            // SAFETY: kill(2) on a pid this app started and has not yet reaped; no memory involved.
            unsafe {
                libc::kill(pid, libc::SIGINT);
            }
        }
    }
}

/// A streamed run: its stderr lines as they come, a handle to stop it, and its end.
pub struct Streaming {
    pub child: Arc<CliChild>,
    pub lines: tokio::sync::mpsc::UnboundedReceiver<String>,
    pub done: std::pin::Pin<Box<dyn Future<Output = Result<serde_json::Value, YardError>>>>,
}

fn not_found() -> YardError {
    YardError::new(ErrorCode::InvalidArgument, "yard was not found").with_hint("set its path in Settings")
}

/// What an exit means: 0 decodes `R` from stdout, 1 is the CLI's `{"error": YardError}`, 3 is
/// yardd not running, anything else names stderr's last line.
fn outcome<R: DeserializeOwned>(
    shown: &str,
    code: Option<i32>,
    stdout: &[u8],
    last: &str,
) -> Result<R, YardError> {
    match code {
        Some(0) => serde_json::from_slice(stdout).map_err(|e| {
            YardError::new(
                ErrorCode::ProtocolMismatch,
                format!("yard {shown} printed an unexpected shape: {e}"),
            )
        }),
        Some(1) => match serde_json::from_slice::<Failure>(stdout) {
            Ok(f) => Err(f.error),
            Err(_) => Err(YardError::internal(format!("yard {shown} failed: {last}"))),
        },
        Some(3) => Err(not_running()),
        code => Err(YardError::internal(format!(
            "yard {shown} exited {}: {last}",
            code.map_or_else(|| "on a signal".to_string(), |c| c.to_string()),
        ))),
    }
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::os::unix::fs::PermissionsExt;
    use yard_proto::ErrorCode;

    /// A stand-in `yard` that writes its arguments beside itself and answers from `out` and `exit`.
    fn stub(dir: &std::path::Path, out: &str, exit: i32) -> PathBuf {
        let yard = dir.join("yard");
        let script = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$0.args\"\nprintf '%s' '{out}'\necho 'first' >&2\necho 'the last line' >&2\nexit {exit}\n"
        );
        std::fs::write(&yard, script).unwrap();
        std::fs::set_permissions(&yard, std::fs::Permissions::from_mode(0o755)).unwrap();
        yard
    }

    fn runner(yard: Option<PathBuf>, root_is_default: bool) -> CliRunner {
        CliRunner::new(yard, PathBuf::from("/tmp/yard dev"), root_is_default, Arc::new(Bridge::new()))
    }

    fn args(yard: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(yard.with_extension("args")).unwrap().lines().map(str::to_string).collect()
    }

    #[test]
    fn exit_zero_decodes() {
        let dir = tempfile::tempdir().unwrap();
        let r = runner(Some(stub(dir.path(), r#"{"name":"blog"}"#, 0)), true);
        let v: Value = futures::executor::block_on(r.run(&["sites"])).unwrap();
        assert_eq!(v["name"], "blog");
    }

    #[test]
    fn exit_one_is_the_yard_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = r#"{"error":{"code":"PORT_IN_USE","message":"port 80 is held","hint":"yard ports"}}"#;
        let r = runner(Some(stub(dir.path(), out, 1)), true);
        let e = futures::executor::block_on(r.run::<Value>(&["ports", "profile", "privileged"])).unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str(), e.hint.as_deref()),
            (ErrorCode::PortInUse, "port 80 is held", Some("yard ports"))
        );
    }

    #[test]
    fn exit_three_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let r = runner(Some(stub(dir.path(), "", 3)), true);
        let e = futures::executor::block_on(r.run::<Value>(&["status"])).unwrap_err();
        assert_eq!(e.code, ErrorCode::DaemonNotRunning);
    }

    #[test]
    fn other_exits_name_the_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let r = runner(Some(stub(dir.path(), "", 2)), true);
        let e = futures::executor::block_on(r.run::<Value>(&["bogus"])).unwrap_err();
        assert_eq!(e.message, "yard bogus exited 2: the last line");
    }

    #[test]
    fn non_default_root_is_passed_first() {
        let dir = tempfile::tempdir().unwrap();
        let yard = stub(dir.path(), "null", 0);
        let r = runner(Some(yard.clone()), false);
        let _: Value = futures::executor::block_on(r.run(&["start"])).unwrap();
        assert_eq!(args(&yard), ["--root", "/tmp/yard dev", "start", "--json"]);
    }

    #[test]
    fn json_flag_is_appended() {
        let dir = tempfile::tempdir().unwrap();
        let yard = stub(dir.path(), "null", 0);
        let _: Value = futures::executor::block_on(runner(Some(yard.clone()), true).run(&["stop"])).unwrap();
        assert_eq!(args(&yard), ["stop", "--json"]);
    }

    /// A stand-in `yard` running `body` after writing its arguments beside itself.
    fn script(dir: &std::path::Path, body: &str) -> PathBuf {
        let yard = dir.join("yard");
        let script =
            format!("#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$0.args\"\n{body}\n");
        std::fs::write(&yard, script).unwrap();
        std::fs::set_permissions(&yard, std::fs::Permissions::from_mode(0o755)).unwrap();
        yard
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// A dump's progress reaches the app line by line, and `--json` goes before the words after
    /// `--`, which belong to mysqldump (db spec §10).
    #[test]
    fn a_stream_passes_stderr_lines_and_decodes_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let yard = script(
            dir.path(),
            r#"echo '{"bytes":5,"total":10}' >&2
echo 'a note' >&2
printf '%s' '{"instance":"mysql-8.4","name":"shop","file":"/tmp/x.sql","bytes":10}'"#,
        );
        let r = runner(Some(yard.clone()), false);
        let mut run = r.stream(&strings(&["db", "dump", "shop", "/tmp/x.sql", "--", "--routines"]));
        let v = futures::executor::block_on(&mut run.done).unwrap();
        assert_eq!(v["bytes"], 10);
        let mut lines = vec![];
        while let Ok(l) = run.lines.try_recv() {
            lines.push(l);
        }
        assert_eq!(lines, [r#"{"bytes":5,"total":10}"#, "a note"]);
        assert_eq!(
            args(&yard),
            ["--root", "/tmp/yard dev", "--json", "db", "dump", "shop", "/tmp/x.sql", "--", "--routines"]
        );
    }

    /// Cancel is Ctrl-C to the CLI, which stops its tool and exits 130 (db spec §6.5).
    #[test]
    fn an_interrupted_stream_is_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let yard = script(
            dir.path(),
            r#"trap 'exit 130' INT
echo '{"bytes":1,"total":null}' >&2
while :; do sleep 0.05; done"#,
        );
        let r = runner(Some(yard), true);
        let mut run = r.stream(&strings(&["db", "dump", "shop", "/tmp/x.sql"]));
        let first = futures::executor::block_on(run.lines.recv());
        assert_eq!(first.as_deref(), Some(r#"{"bytes":1,"total":null}"#));
        run.child.interrupt();
        let e = futures::executor::block_on(&mut run.done).unwrap_err();
        assert_eq!(e.code, ErrorCode::JobCancelled, "{e:?}");
    }

    /// An interrupt asked for before the CLI has started stops it as soon as it starts.
    #[test]
    fn an_interrupt_before_the_start_still_stops_it() {
        let child = CliChild::default();
        child.interrupt();
        assert!(child.interrupted());
        let mut sleeper = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        child.started(Some(sleeper.id()));
        let status = sleeper.wait().unwrap();
        assert!(!status.success(), "SIGINT ended it");
    }

    /// A failure says what the CLI said, not a progress line.
    #[test]
    fn a_failed_stream_names_its_last_words() {
        let dir = tempfile::tempdir().unwrap();
        let yard = script(
            dir.path(),
            "echo 'mysqldump: Got error 1049' >&2\necho '{\"bytes\":0,\"total\":null}' >&2\nexit 2",
        );
        let r = runner(Some(yard), true);
        let run = r.stream(&strings(&["db", "dump", "nope", "/tmp/x.sql"]));
        let e = futures::executor::block_on(run.done).unwrap_err();
        assert_eq!(e.message, "yard db dump nope /tmp/x.sql exited 2: mysqldump: Got error 1049");
        let missing = runner(None, true).stream(&strings(&["db", "list"]));
        assert_eq!(futures::executor::block_on(missing.done).unwrap_err().message, "yard was not found");
    }

    #[test]
    fn missing_yard_says_where_to_set_it() {
        let e = futures::executor::block_on(runner(None, true).run::<Value>(&["start"])).unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str(), e.hint.as_deref()),
            (ErrorCode::InvalidArgument, "yard was not found", Some("set its path in Settings"))
        );
    }
}
