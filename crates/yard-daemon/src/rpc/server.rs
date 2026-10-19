//! Connections: `hello` first, requests answered in order, and pushed events once the
//! client subscribes.

use super::handlers;
use crate::daemon::{Context, VERSION};
use serde_json::Value;
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use yard_proto::methods::{self, HelloParams, HelloResult, SubscribeParams};
use yard_proto::rpc::{Notification, Request, Response, encode_line};
use yard_proto::{ErrorCode, Event, PROTOCOL_VERSION, Topic, YardError};

pub async fn serve(listener: UnixListener, ctx: Arc<Context>, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    while connections.try_join_next().is_some() {}
                    connections.spawn(connection(ctx.clone(), stream));
                }
                Err(e) => tracing::warn!("accept failed: {e}"),
            },
        }
    }
    connections.abort_all();
}

async fn connection(ctx: Arc<Context>, stream: UnixStream) {
    let (read, write) = stream.into_split();
    let (out, queue) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(write_lines(write, queue));
    let mut lines = BufReader::new(read).lines();
    let mut greeted = false;
    let mut subscription: Option<JoinHandle<()>> = None;

    while let Ok(Some(line)) = lines.next_line().await {
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(r) => r,
            Err(e) => {
                let error = YardError::new(ErrorCode::InvalidRequest, format!("not a JSON-RPC request: {e}"));
                let _ = out.send(encode_line(&Response::err(0, error)));
                continue;
            }
        };
        let Some(id) = request.id else { continue };
        let mut close = false;
        let result = match request.method.as_str() {
            methods::HELLO => {
                let answer = hello(request.params);
                close = answer.as_ref().is_err_and(|e| e.code == ErrorCode::ProtocolMismatch);
                greeted = answer.is_ok();
                answer
            }
            _ if !greeted => {
                Err(YardError::new(ErrorCode::InvalidRequest, "the first request must be hello")
                    .with_hint("send hello { protocol, client } first"))
            }
            methods::SUBSCRIBE => match serde_json::from_value::<SubscribeParams>(request.params) {
                Ok(p) => {
                    if let Some(old) = subscription.take() {
                        old.abort();
                    }
                    let topics: HashSet<Topic> = p.topics.into_iter().collect();
                    subscription = Some(tokio::spawn(forward(ctx.bus.subscribe(), topics, out.clone())));
                    Ok(Value::Null)
                }
                Err(e) => Err(YardError::new(ErrorCode::InvalidArgument, format!("bad parameters: {e}"))),
            },
            method => handlers::handle(&ctx, method, request.params).await,
        };
        let response = match result {
            Ok(value) => Response::ok(id, value),
            Err(e) => Response::err(id, e),
        };
        if out.send(encode_line(&response)).is_err() || close {
            break;
        }
    }
    if let Some(s) = subscription {
        s.abort();
    }
    drop(out);
    let _ = writer.await;
}

fn hello(params: Value) -> Result<Value, YardError> {
    let p: HelloParams = serde_json::from_value(params)
        .map_err(|e| YardError::new(ErrorCode::InvalidArgument, format!("bad hello: {e}")))?;
    let major = |v: &str| v.split('.').next().unwrap_or_default().to_string();
    if major(&p.protocol) != major(PROTOCOL_VERSION) {
        return Err(YardError::new(
            ErrorCode::ProtocolMismatch,
            format!("{} speaks protocol {}, yardd speaks {PROTOCOL_VERSION}", p.client, p.protocol),
        )
        .with_hint(format!("upgrade yard and yardd together (yardd {VERSION})")));
    }
    let result = HelloResult {
        protocol: PROTOCOL_VERSION.into(),
        daemon_version: VERSION.into(),
        commit: crate::daemon::COMMIT.into(),
    };
    serde_json::to_value(result).map_err(YardError::internal)
}

async fn forward(
    mut events: broadcast::Receiver<Event>,
    topics: HashSet<Topic>,
    out: mpsc::UnboundedSender<Vec<u8>>,
) {
    loop {
        match events.recv().await {
            Ok(event) if topics.contains(&event.topic()) => {
                if out.send(encode_line(&Notification::event(event))).is_err() {
                    return;
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!("a subscriber fell behind and skipped {missed} events");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn write_lines(mut write: OwnedWriteHalf, mut queue: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(line) = queue.recv().await {
        if write.write_all(&line).await.is_err() {
            return;
        }
    }
}
