//! A session with yardd (M6 design §5.2, §5.6): one connection for events, one for calls, a
//! one-shot connection per slow call and one for logs, reconnecting with a backoff. Each
//! successful connect is a new epoch, so a client can tell a job id yardd reused after a restart
//! from the job it was following.

use crate::api::Method;
use crate::client::{not_running, protocol_version};
use crate::connection::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use yard_proto::methods::HelloResult;
use yard_proto::{ErrorCode, Event, PROTOCOL_VERSION, Topic, YardError};

/// What to subscribe to on a yardd speaking `daemon`: an older yardd refuses a topic it does not
/// know, so `tunnels` waits for 2.10 and `state` for 2.11.
pub fn topics_for(daemon: (u32, u32)) -> Vec<Topic> {
    let mut topics = vec![Topic::Units, Topic::Sites, Topic::Ports, Topic::Jobs];
    if daemon >= (2, 10) {
        topics.push(Topic::Tunnels);
    }
    if daemon >= (2, 11) {
        topics.push(Topic::State);
    }
    topics
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub epoch: u64,
    pub hello: HelloResult,
    /// yardd speaks an older protocol than this build: it was not restarted after an upgrade.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected(SessionInfo),
    Disconnected(YardError),
    /// yardd refused the protocol; the session waits for `reconnect_now`.
    Incompatible(YardError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    State(ConnectionState),
    Event(Event),
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub socket: PathBuf,
    /// What `hello` says the client is: `Yard.app 0.1.0 (<commit>)`.
    pub client: String,
    /// The waits between attempts; the last one repeats forever.
    pub backoff: Vec<Duration>,
    pub timeout: Duration,
}

impl SessionConfig {
    /// Design §5.6's backoff (0.25 s, 0.5 s, 1 s, 2 s, then every 5 s) and a 60 s call timeout.
    pub fn new(socket: PathBuf, client: String) -> SessionConfig {
        let backoff = [250, 500, 1000, 2000, 5000].map(Duration::from_millis).to_vec();
        SessionConfig { socket, client, backoff, timeout: Duration::from_secs(60) }
    }
}

struct Inner {
    config: SessionConfig,
    calls: Mutex<Option<Connection>>,
    wake: Notify,
    following: AtomicBool,
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

/// A connection subscribed to `logs` alone; dropping it closes it.
pub struct LogStream {
    conn: Connection,
    events: mpsc::UnboundedReceiver<Event>,
}

impl LogStream {
    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        self.conn.close();
    }
}

impl Session {
    /// Starts connecting on the current tokio runtime and returns the updates as they happen.
    pub fn start(config: SessionConfig) -> (Session, mpsc::UnboundedReceiver<Update>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            config,
            calls: Mutex::new(None),
            wake: Notify::new(),
            following: AtomicBool::new(false),
        });
        tokio::spawn(run(inner.clone(), tx));
        (Session { inner }, rx)
    }

    /// A call on the calls connection, or for a slow method on a connection of its own.
    /// `DAEMON_NOT_RUNNING` while disconnected.
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        m: Method<P, R>,
        params: P,
    ) -> Result<R, YardError> {
        if m.slow {
            let (conn, _events) = self.open().await?;
            let result = conn.call(m, params).await;
            conn.close();
            return result;
        }
        let conn = lock(&self.inner.calls).clone().ok_or_else(not_running)?;
        conn.call(m, params).await
    }

    /// Drops the backoff (or an incompatible wait) and connects now.
    pub fn reconnect_now(&self) {
        self.inner.wake.notify_one();
    }

    /// Whether job events reach this session, so a job's progress and end can be followed.
    pub fn can_follow_jobs(&self) -> bool {
        self.inner.following.load(Ordering::SeqCst)
    }

    /// A connection subscribed to `logs` only, so a busy log never delays other events.
    pub async fn logs(&self) -> Result<LogStream, YardError> {
        let (conn, events) = self.open().await?;
        if let Err(e) = conn.subscribe(&[Topic::Logs]).await {
            conn.close();
            return Err(e);
        }
        Ok(LogStream { conn, events })
    }

    async fn open(&self) -> Result<(Connection, mpsc::UnboundedReceiver<Event>), YardError> {
        let c = &self.inner.config;
        Connection::open(&c.socket, &c.client, c.timeout).await
    }
}

async fn run(inner: Arc<Inner>, tx: mpsc::UnboundedSender<Update>) {
    let send = |s: ConnectionState| tx.send(Update::State(s)).is_ok();
    if !send(ConnectionState::Connecting) {
        return;
    }
    let mut epoch = 0;
    let mut attempt = 0;
    loop {
        match connect(&inner.config).await {
            Ok((events, mut event_rx, calls)) => {
                epoch += 1;
                attempt = 0;
                let hello = events.hello().clone();
                let stale = protocol_version(&hello.protocol) < protocol_version(PROTOCOL_VERSION);
                *lock(&inner.calls) = Some(calls.clone());
                inner.following.store(true, Ordering::SeqCst);
                if !send(ConnectionState::Connected(SessionInfo { epoch, hello, stale })) {
                    return;
                }
                let why = loop {
                    tokio::select! {
                        event = event_rx.recv() => match event {
                            Some(e) => {
                                if tx.send(Update::Event(e)).is_err() {
                                    return;
                                }
                            }
                            None => break events.closed().await,
                        },
                        why = calls.closed() => break why,
                        () = inner.wake.notified() => break YardError::internal("reconnecting to yardd"),
                    }
                };
                inner.following.store(false, Ordering::SeqCst);
                lock(&inner.calls).take();
                events.close();
                calls.close();
                if !send(ConnectionState::Disconnected(why)) {
                    return;
                }
            }
            Err(e) if e.code == ErrorCode::ProtocolMismatch => {
                if !send(ConnectionState::Incompatible(e)) {
                    return;
                }
                inner.wake.notified().await;
                continue;
            }
            Err(e) => {
                if !send(ConnectionState::Disconnected(e)) {
                    return;
                }
            }
        }
        let backoff = &inner.config.backoff;
        let wait = backoff.get(attempt).or(backoff.last()).copied().unwrap_or(Duration::from_secs(5));
        attempt += 1;
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = inner.wake.notified() => {}
        }
        if tx.is_closed() {
            return;
        }
    }
}

/// The events connection, subscribed, then the calls connection: nothing is fetched before the
/// events that would make it stale can arrive.
async fn connect(
    config: &SessionConfig,
) -> Result<(Connection, mpsc::UnboundedReceiver<Event>, Connection), YardError> {
    let (events, event_rx) = Connection::open(&config.socket, &config.client, config.timeout).await?;
    let daemon = protocol_version(&events.hello().protocol).unwrap_or((0, 0));
    if let Err(e) = events.subscribe(&topics_for(daemon)).await {
        events.close();
        return Err(e);
    }
    match Connection::open(&config.socket, &config.client, config.timeout).await {
        Ok((calls, _)) => Ok((events, event_rx, calls)),
        Err(e) => {
            events.close();
            Err(e)
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
