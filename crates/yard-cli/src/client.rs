//! Talking to `yardd`: `hello` first, then one request at a time over its socket.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use yard_core::Paths;
use yard_proto::methods::{HELLO, HelloParams, HelloResult};
use yard_proto::rpc::{Request, Response, encode_line};
use yard_proto::{ErrorCode, PROTOCOL_VERSION, YardError};

/// A mutation reconciles before it answers, and reconcile may wait on the helper.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum CliError {
    /// Nothing answers on the socket: exit 3, `yardd is not running → yard start`.
    DaemonNotRunning,
    Yard(YardError),
}

impl From<YardError> for CliError {
    fn from(e: YardError) -> Self {
        CliError::Yard(e)
    }
}

pub struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    pub hello: HelloResult,
}

impl Client {
    pub async fn connect(paths: &Paths) -> Result<Client, CliError> {
        let stream =
            UnixStream::connect(paths.daemon_socket()).await.map_err(|_| CliError::DaemonNotRunning)?;
        let (read, write) = stream.into_split();
        let placeholder =
            HelloResult { protocol: String::new(), daemon_version: String::new(), commit: String::new() };
        let mut client =
            Client { lines: BufReader::new(read).lines(), write, next_id: 1, hello: placeholder };
        let params = HelloParams {
            protocol: PROTOCOL_VERSION.into(),
            client: format!("yard {}", env!("CARGO_PKG_VERSION")),
        };
        client.hello = client.call(HELLO, params).await?;
        Ok(client)
    }

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<R, YardError> {
        let id = self.next_id;
        self.next_id += 1;
        let params = serde_json::to_value(params).map_err(YardError::internal)?;
        tokio::time::timeout(CALL_TIMEOUT, self.exchange(id, method, params)).await.unwrap_or_else(|_| {
            Err(YardError::internal(format!("yardd did not answer {method} within 60 s"))
                .with_hint("yard doctor checks what is stuck"))
        })
    }

    async fn exchange<R: DeserializeOwned>(
        &mut self,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<R, YardError> {
        let lost = |e: std::io::Error| YardError::internal(format!("lost the connection to yardd: {e}"));
        self.write.write_all(&encode_line(&Request::new(id, method, params))).await.map_err(lost)?;
        loop {
            let line = self.lines.next_line().await.map_err(lost)?.ok_or_else(|| {
                YardError::internal("yardd closed the connection").with_hint("yard doctor checks why")
            })?;
            // Notifications carry no id; this client never subscribes, but skip them anyway.
            let Ok(response) = serde_json::from_str::<Response>(&line) else { continue };
            if response.id != id {
                continue;
            }
            let value = response.into_result()?;
            return serde_json::from_value(value).map_err(|e| {
                YardError::new(
                    ErrorCode::ProtocolMismatch,
                    format!("yardd answered {method} in an unexpected shape: {e}"),
                )
                .with_hint("upgrade yard and yardd together")
            });
        }
    }
}
