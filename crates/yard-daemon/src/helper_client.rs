//! The daemon's side of the helper socket: one connection and one request per call.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use yard_proto::helper::{self, HelperStatus, TestConfigResult};
use yard_proto::rpc::{Request, Response, encode_line};
use yard_proto::{ErrorCode, YardError};

const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const INSTALL_HINT: &str = "sudo yard helper install";

#[derive(Debug, Clone)]
pub struct HelperClient {
    socket: PathBuf,
}

impl HelperClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into() }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub async fn start_nginx(&self) -> Result<(), YardError> {
        self.call(helper::NGINX_START, Value::Null).await
    }

    pub async fn stop_nginx(&self) -> Result<(), YardError> {
        self.call(helper::NGINX_STOP, Value::Null).await
    }

    pub async fn reload_nginx(&self) -> Result<(), YardError> {
        self.call(helper::NGINX_RELOAD, Value::Null).await
    }

    pub async fn test_nginx(&self) -> Result<TestConfigResult, YardError> {
        self.call(helper::NGINX_TEST_CONFIG, Value::Null).await
    }

    pub async fn install_resolver(&self, tld: &str, port: u16) -> Result<(), YardError> {
        self.call(helper::RESOLVER_INSTALL, json!({ "tld": tld, "port": port })).await
    }

    pub async fn remove_resolver(&self, tld: &str) -> Result<(), YardError> {
        self.call(helper::RESOLVER_REMOVE, json!({ "tld": tld })).await
    }

    pub async fn status(&self) -> Result<HelperStatus, YardError> {
        self.call(helper::STATUS, Value::Null).await
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
        let stream = UnixStream::connect(&self.socket).await.map_err(|e| {
            YardError::new(
                ErrorCode::HelperNotInstalled,
                format!("the Yard helper is not running ({}: {e})", self.socket.display()),
            )
            .with_hint(INSTALL_HINT)
        })?;
        let (read, mut write) = stream.into_split();
        write.write_all(&encode_line(&Request::new(1, method, params))).await.map_err(YardError::internal)?;
        let mut line = String::new();
        let n = BufReader::new(read).read_line(&mut line).await.map_err(YardError::internal)?;
        if n == 0 {
            return Err(YardError::new(
                ErrorCode::HelperRefused,
                "the helper closed the connection; it serves only the user it was installed for",
            )
            .with_hint(INSTALL_HINT));
        }
        let response: Response = serde_json::from_str(&line)
            .map_err(|e| YardError::internal(format!("unreadable helper answer: {e}")))?;
        response.into_result()
    }
}
