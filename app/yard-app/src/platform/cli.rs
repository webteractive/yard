//! Running the `yard` CLI (M6 design §5.8): an argument vector, `--json`, a fixed PATH, and the
//! CLI's own exit codes; never a shell, never a hint's text.

use crate::bridge::Bridge;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use yard_client::not_running;
use yard_proto::{ErrorCode, YardError};

/// PATH for the CLI and anything it starts: nothing a shell profile added.
const PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub struct CliRunner {
    pub yard: Option<PathBuf>,
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
        CliRunner { yard, root, root_is_default, bridge }
    }

    /// `yard [--root <root>] <args> --json`: exit 0 decodes `R`, exit 1 is the CLI's
    /// `{"error": YardError}`, exit 3 is yardd not running, anything else names stderr's last line.
    pub fn run<R: DeserializeOwned + Send + 'static>(
        &self,
        args: &[&str],
    ) -> impl Future<Output = Result<R, YardError>> + use<R> {
        let yard = self.yard.clone();
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
                return Err(YardError::new(ErrorCode::InvalidArgument, "yard was not found")
                    .with_hint("set its path in Settings"));
            };
            let out = task
                .await
                .map_err(YardError::internal)?
                .map_err(|e| YardError::internal(format!("yard {shown} did not start: {e}")))?;
            match out.status.code() {
                Some(0) => serde_json::from_slice(&out.stdout).map_err(|e| {
                    YardError::new(
                        ErrorCode::ProtocolMismatch,
                        format!("yard {shown} printed an unexpected shape: {e}"),
                    )
                }),
                Some(1) => match serde_json::from_slice::<Failure>(&out.stdout) {
                    Ok(f) => Err(f.error),
                    Err(_) => {
                        Err(YardError::internal(format!("yard {shown} failed: {}", last_line(&out.stderr))))
                    }
                },
                Some(3) => Err(not_running()),
                code => Err(YardError::internal(format!(
                    "yard {shown} exited {}: {}",
                    code.map_or_else(|| "on a signal".to_string(), |c| c.to_string()),
                    last_line(&out.stderr)
                ))),
            }
        }
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

    #[test]
    fn missing_yard_says_where_to_set_it() {
        let e = futures::executor::block_on(runner(None, true).run::<Value>(&["start"])).unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str(), e.hint.as_deref()),
            (ErrorCode::InvalidArgument, "yard was not found", Some("set its path in Settings"))
        );
    }
}
