//! The daemon's side of the helper socket: one connection and one request per call.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use yard_proto::helper::{self, BindParams, BindResult, HelperStatus};
use yard_proto::rpc::{Request, Response, encode_line};
use yard_proto::{ErrorCode, YardError};

const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const INSTALL_HINT: &str = "sudo yard helper install";

#[derive(Debug, Clone)]
pub struct HelperClient {
    socket: PathBuf,
    /// This yardd's data root and the helper's LaunchDaemon plist, when the helper may only be
    /// used by the yardd of the root it was installed for (`serving`).
    serving: Option<(PathBuf, PathBuf)>,
}

impl HelperClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into(), serving: None }
    }

    /// Asks the helper to change the system (the resolver, 80 and 443) only while it was
    /// installed for `root`, as `plist` records. The helper takes these verbs from any process of
    /// its user, so without this a yardd on a scratch root would point the Mac's `.yard` names at
    /// its own dnsmasq.
    pub fn serving(mut self, root: &Path, plist: &Path) -> Self {
        self.serving = Some((root.to_path_buf(), plist.to_path_buf()));
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The data root the installed helper serves, when it is not this yardd's. `None` when it is
    /// this one's, or no helper plist can be read (the call then fails as it always has).
    pub fn other_root(&self) -> Option<PathBuf> {
        let (root, plist) = self.serving.as_ref()?;
        let theirs = yard_core::render::launchd::helper_root(&std::fs::read(plist).ok()?)?;
        let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        };
        (!same(root, &theirs)).then_some(theirs)
    }

    /// Refuses a verb that changes the system when the helper serves another root.
    fn ours(&self) -> Result<(), YardError> {
        match (self.other_root(), &self.serving) {
            (Some(theirs), Some((root, _))) => Err(YardError::new(
                ErrorCode::HelperRefused,
                format!(
                    "the helper serves {}, not {}, so this yardd leaves the resolver and ports 80 and 443 to that root's yardd",
                    theirs.display(),
                    root.display()
                ),
            )
            .with_hint(format!("sudo yard --root {} helper install", root.display()))),
            _ => Ok(()),
        }
    }

    pub async fn install_resolver(&self, tld: &str, port: u16) -> Result<(), YardError> {
        self.ours()?;
        self.call(helper::RESOLVER_INSTALL, json!({ "tld": tld, "port": port })).await
    }

    pub async fn remove_resolver(&self, tld: &str) -> Result<(), YardError> {
        self.ours()?;
        self.call(helper::RESOLVER_REMOVE, json!({ "tld": tld })).await
    }

    pub async fn status(&self) -> Result<HelperStatus, YardError> {
        self.call(helper::STATUS, Value::Null).await
    }

    /// `ports.bind`: listening sockets on 127.0.0.1 and ::1 for a privileged port, in
    /// that order. The answer carries them, so this reads it with `recvmsg` on a
    /// blocking socket that times out like `call` does.
    pub async fn bind_port(&self, port: u16) -> Result<Vec<OwnedFd>, YardError> {
        self.ours()?;
        let socket = self.socket.clone();
        let exchange = move || -> Result<Vec<OwnedFd>, YardError> {
            let mut stream =
                std::os::unix::net::UnixStream::connect(&socket).map_err(|e| not_running(&socket, e))?;
            stream.set_read_timeout(Some(CALL_TIMEOUT)).map_err(YardError::internal)?;
            stream.set_write_timeout(Some(CALL_TIMEOUT)).map_err(YardError::internal)?;
            let request = Request::new(1, helper::PORTS_BIND, json!(BindParams { port }));
            stream.write_all(&encode_line(&request)).map_err(YardError::internal)?;
            let (line, fds) = crate::fdpass::recv_line_with_fds(stream.as_raw_fd()).map_err(|e| {
                YardError::internal(format!("the helper did not answer {}: {e}", helper::PORTS_BIND))
            })?;
            if line.is_empty() {
                return Err(closed());
            }
            let response: Response = serde_json::from_str(&line)
                .map_err(|e| YardError::internal(format!("unreadable helper answer: {e}")))?;
            let answer = response.into_result().map_err(|e| match e.code {
                ErrorCode::MethodNotFound => YardError::new(
                    ErrorCode::HelperRefused,
                    "the installed helper predates ports.bind, so it cannot open 80 and 443",
                )
                .with_hint(INSTALL_HINT),
                _ => e,
            })?;
            let result: BindResult = serde_json::from_value(answer)
                .map_err(|e| YardError::internal(format!("unexpected helper answer to ports.bind: {e}")))?;
            if fds.len() != result.addresses.len() {
                return Err(YardError::internal(format!(
                    "the helper bound {} but handed over {} sockets",
                    result.addresses.join(" and "),
                    fds.len()
                )));
            }
            Ok(fds)
        };
        tokio::task::spawn_blocking(exchange).await.map_err(YardError::internal)?
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, YardError> {
        let answer =
            tokio::time::timeout(CALL_TIMEOUT, self.exchange(method, params)).await.unwrap_or_else(|_| {
                Err(YardError::internal(format!("the helper did not answer {method} within 10 s"))
                    .with_hint("sudo launchctl kickstart -k system/dev.more.yard.helper"))
            })?;
        serde_json::from_value(answer)
            .map_err(|e| YardError::internal(format!("unexpected helper answer to {method}: {e}")))
    }

    async fn exchange(&self, method: &str, params: Value) -> Result<Value, YardError> {
        let stream = UnixStream::connect(&self.socket).await.map_err(|e| not_running(&self.socket, e))?;
        let (read, mut write) = stream.into_split();
        write.write_all(&encode_line(&Request::new(1, method, params))).await.map_err(YardError::internal)?;
        let mut line = String::new();
        let n = BufReader::new(read).read_line(&mut line).await.map_err(YardError::internal)?;
        if n == 0 {
            return Err(closed());
        }
        let response: Response = serde_json::from_str(&line)
            .map_err(|e| YardError::internal(format!("unreadable helper answer: {e}")))?;
        response.into_result()
    }
}

fn not_running(socket: &Path, e: std::io::Error) -> YardError {
    YardError::new(
        ErrorCode::HelperNotInstalled,
        format!("the Yard helper is not running ({}: {e})", socket.display()),
    )
    .with_hint(INSTALL_HINT)
}

fn closed() -> YardError {
    YardError::new(
        ErrorCode::HelperRefused,
        "the helper closed the connection; it serves only the user it was installed for",
    )
    .with_hint(INSTALL_HINT)
}
