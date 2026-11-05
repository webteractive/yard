//! What the app's state needs from yardd, as a trait: `SessionDaemon` is the real one, through the
//! bridge's tokio runtime; tests use `testing::FakeDaemon` on GPUI's executor.

use crate::bridge::Bridge;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use yard_client::api::Method;
use yard_client::session::{Session, SessionConfig, Update};
use yard_proto::{ErrorCode, Event, YardError};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Daemon: Send + Sync + 'static {
    fn call_value(
        &self,
        name: &'static str,
        slow: bool,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, YardError>>;
    /// The session's updates; taken once, by `AppState`.
    fn take_updates(&self) -> UnboundedReceiver<Update>;
    fn reconnect_now(&self);
    fn can_follow_jobs(&self) -> bool;
    /// Log lines on a connection of their own; dropping the receiver closes it.
    fn logs(&self) -> BoxFuture<'static, Result<UnboundedReceiver<Event>, YardError>>;
}

/// A typed call through any `Daemon`.
pub async fn call<P: Serialize, R: DeserializeOwned>(
    d: &dyn Daemon,
    m: Method<P, R>,
    params: P,
) -> Result<R, YardError> {
    let params = serde_json::to_value(params).map_err(YardError::internal)?;
    let value = d.call_value(m.name, m.slow, params).await?;
    serde_json::from_value(value).map_err(|e| {
        YardError::new(
            ErrorCode::ProtocolMismatch,
            format!("yardd answered {} in an unexpected shape: {e}", m.name),
        )
        .with_hint("upgrade yard and yardd together")
    })
}

pub struct SessionDaemon {
    session: Session,
    bridge: Arc<Bridge>,
    updates: Mutex<Option<UnboundedReceiver<Update>>>,
}

impl SessionDaemon {
    /// Starts the session on the bridge's runtime.
    pub fn start(config: SessionConfig, bridge: Arc<Bridge>) -> Arc<SessionDaemon> {
        let _guard = bridge.runtime.enter();
        let (session, updates) = Session::start(config);
        Arc::new(SessionDaemon { session, bridge, updates: Mutex::new(Some(updates)) })
    }
}

impl Daemon for SessionDaemon {
    fn call_value(
        &self,
        name: &'static str,
        slow: bool,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, YardError>> {
        let session = self.session.clone();
        let method =
            if slow { Method::<Value, Value>::slow(name) } else { Method::<Value, Value>::quick(name) };
        let task = self.bridge.spawn(async move { session.call(method, params).await });
        Box::pin(async move {
            task.await
                .unwrap_or_else(|e| Err(YardError::internal(format!("the call to {name} stopped: {e}"))))
        })
    }

    fn take_updates(&self) -> UnboundedReceiver<Update> {
        self.updates.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| unbounded_channel().1)
    }

    fn reconnect_now(&self) {
        self.session.reconnect_now();
    }

    fn can_follow_jobs(&self) -> bool {
        self.session.can_follow_jobs()
    }

    fn logs(&self) -> BoxFuture<'static, Result<UnboundedReceiver<Event>, YardError>> {
        let session = self.session.clone();
        let bridge = self.bridge.clone();
        Box::pin(async move {
            let opened = bridge.spawn(async move { session.logs().await });
            let mut stream =
                opened.await.map_err(|e| YardError::internal(format!("logs stopped: {e}")))??;
            let (tx, rx) = unbounded_channel();
            // The stream lives as long as someone reads it.
            bridge.spawn(async move {
                loop {
                    tokio::select! {
                        event = stream.recv() => match event {
                            Some(e) => {
                                if tx.send(e).is_err() {
                                    break;
                                }
                            }
                            None => break,
                        },
                        () = tx.closed() => break,
                    }
                }
            });
            Ok(rx)
        })
    }
}
