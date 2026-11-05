//! Talking to `yardd` one request at a time: `hello` first, then a call, then the next. The CLI's
//! connection; Yard.app pipelines through `Connection` instead.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use yard_proto::methods::{HELLO, HelloParams, HelloResult, SUBSCRIBE, SubscribeParams};
use yard_proto::rpc::{Notification, Request, Response, encode_line};
use yard_proto::{ErrorCode, Event, PROTOCOL_VERSION, Topic, YardError};

/// A mutation reconciles before it answers, and reconcile may wait on the helper.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    /// Events that arrived while a call waited for its answer.
    events: VecDeque<Event>,
    pub hello: HelloResult,
}

impl Client {
    /// Connects to `socket` and says `hello` as `name` (`yard 0.1.0`). Nothing answering is
    /// `DAEMON_NOT_RUNNING`, `yardd is not running`, hint `yard start`.
    pub async fn connect(socket: &Path, name: &str) -> Result<Client, YardError> {
        let stream = UnixStream::connect(socket).await.map_err(|_| not_running())?;
        let (read, write) = stream.into_split();
        let placeholder =
            HelloResult { protocol: String::new(), daemon_version: String::new(), commit: String::new() };
        let mut client = Client {
            lines: BufReader::new(read).lines(),
            write,
            next_id: 1,
            events: VecDeque::new(),
            hello: placeholder,
        };
        let params = HelloParams { protocol: PROTOCOL_VERSION.into(), client: name.to_string() };
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
            // An event can arrive before the answer that started it; keep it for next_event.
            if let Ok(note) = serde_json::from_str::<Notification>(&line) {
                self.events.push_back(note.params);
                continue;
            }
            let Ok(response) = serde_json::from_str::<Response>(&line) else { continue };
            if response.id != id {
                continue;
            }
            let value = response.into_result().map_err(|e| self.stale(e, method))?;
            return serde_json::from_value(value).map_err(|e| {
                YardError::new(
                    ErrorCode::ProtocolMismatch,
                    format!("yardd answered {method} in an unexpected shape: {e}"),
                )
                .with_hint("upgrade yard and yardd together")
            });
        }
    }

    /// A method an older yardd does not know yet means it was not restarted after an
    /// upgrade, so `METHOD_NOT_FOUND` names the restart.
    fn stale(&self, e: YardError, method: &str) -> YardError {
        let protocol = &self.hello.protocol;
        if e.code != ErrorCode::MethodNotFound
            || protocol_version(protocol).is_none_or(|v| Some(v) >= protocol_version(PROTOCOL_VERSION))
        {
            return e;
        }
        YardError::new(
            ErrorCode::MethodNotFound,
            format!("yardd speaks protocol {protocol} and has no {method}"),
        )
        .with_hint("yard stop && yard start")
    }

    pub async fn subscribe(&mut self, topics: &[Topic]) -> Result<(), YardError> {
        self.call(SUBSCRIBE, SubscribeParams { topics: topics.to_vec() }).await
    }

    /// The next pushed event. No timeout: a brew install can take many minutes.
    pub async fn next_event(&mut self) -> Result<Event, YardError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        let lost = |e: std::io::Error| YardError::internal(format!("lost the connection to yardd: {e}"));
        loop {
            let line = self.lines.next_line().await.map_err(lost)?.ok_or_else(|| {
                YardError::internal("yardd closed the connection").with_hint("yard doctor checks why")
            })?;
            if let Ok(note) = serde_json::from_str::<Notification>(&line) {
                return Ok(note.params);
            }
        }
    }
}

/// What a client says when nothing answers on yardd's socket.
pub fn not_running() -> YardError {
    YardError::new(ErrorCode::DaemonNotRunning, "yardd is not running").with_hint("yard start")
}

/// A `major.minor` protocol version.
pub fn protocol_version(protocol: &str) -> Option<(u32, u32)> {
    let (major, minor) = protocol.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}
