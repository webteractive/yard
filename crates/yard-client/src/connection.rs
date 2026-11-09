//! A pipelined connection to yardd (M6 design §5.2): many calls in flight at once, matched to
//! their answers by id, with events delivered on a channel as they arrive. Yard.app keeps a few
//! of these; the CLI's one-call-at-a-time `Client` stays as it is.

use crate::api::Method;
use crate::client::{not_running, stale};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use yard_proto::methods::{HELLO, HelloParams, HelloResult, SUBSCRIBE, SubscribeParams};
use yard_proto::rpc::{Notification, Request, Response, encode_line};
use yard_proto::{ErrorCode, Event, PROTOCOL_VERSION, Topic, YardError};

/// The longest line yardd may send: a large site list is a few hundred KB.
pub const MAX_LINE: usize = 16 << 20;

/// `sun_path` on macOS is 104 bytes, its terminating NUL included.
const SOCKET_PATH_MAX: usize = 103;

type Answer = Result<Value, YardError>;

struct Inner {
    write: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
    /// `None` once the connection is closed.
    pending: Mutex<Option<HashMap<u64, oneshot::Sender<Answer>>>>,
    next_id: AtomicU64,
    timeout: Duration,
    hello: OnceLock<HelloResult>,
    closed: watch::Sender<Option<YardError>>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").field("hello", &self.inner.hello.get()).finish_non_exhaustive()
    }
}

impl Connection {
    /// Connects to `socket`, says `hello` as `client`, and returns the connection with the
    /// channel its events arrive on. `timeout` bounds each call.
    pub async fn open(
        socket: &Path,
        client: &str,
        timeout: Duration,
    ) -> Result<(Connection, mpsc::UnboundedReceiver<Event>), YardError> {
        if socket.as_os_str().len() > SOCKET_PATH_MAX {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "{} is longer than a unix socket path may be ({SOCKET_PATH_MAX} bytes)",
                    socket.display()
                ),
            ));
        }
        let stream = UnixStream::connect(socket).await.map_err(|_| not_running())?;
        let (read, write) = stream.into_split();
        let (events, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            write: tokio::sync::Mutex::new(Some(write)),
            pending: Mutex::new(Some(HashMap::new())),
            next_id: AtomicU64::new(1),
            timeout,
            hello: OnceLock::new(),
            closed: watch::Sender::new(None),
            reader: Mutex::new(None),
        });
        let reader = tokio::spawn(read_loop(inner.clone(), read, events));
        *lock(&inner.reader) = Some(reader);
        let conn = Connection { inner };
        let params = HelloParams { protocol: PROTOCOL_VERSION.into(), client: client.to_string() };
        match conn.call_value(HELLO, to_value(params)?).await.and_then(|v| shape::<HelloResult>(HELLO, v)) {
            Ok(hello) => {
                let _ = conn.inner.hello.set(hello);
                Ok((conn, rx))
            }
            Err(e) => {
                conn.inner.shut(e.clone());
                Err(e)
            }
        }
    }

    /// What yardd answered to `hello`.
    pub fn hello(&self) -> &HelloResult {
        self.inner.hello.get().expect("open sets hello before it returns")
    }

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        m: Method<P, R>,
        params: P,
    ) -> Result<R, YardError> {
        let value = self.call_value(m.name, to_value(params)?).await?;
        shape(m.name, value)
    }

    /// A call by name with untyped params and answer.
    pub async fn call_value(&self, method: &str, params: Value) -> Result<Value, YardError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match lock(&self.inner.pending).as_mut() {
            Some(pending) => pending.insert(id, tx),
            None => return Err(self.inner.why()),
        };
        let line = encode_line(&Request::new(id, method, params));
        // One deadline covers the write too: a yardd that stops reading fills the socket's buffer,
        // and a write left waiting would hold the write lock past every later call's timeout.
        let deadline = tokio::time::Instant::now() + self.inner.timeout;
        let write = async {
            match self.inner.write.lock().await.as_mut() {
                Some(w) => w.write_all(&line).await.map_err(lost),
                None => Err(self.inner.why()),
            }
        };
        let answered = match tokio::time::timeout_at(deadline, write).await {
            Ok(Ok(())) => tokio::time::timeout_at(deadline, rx).await,
            Ok(Err(e)) => {
                self.inner.shut(e);
                return Err(self.inner.why());
            }
            Err(elapsed) => Err(elapsed),
        };
        match answered {
            Ok(Ok(answer)) => {
                let protocol = self.inner.hello.get().map_or("", |h| h.protocol.as_str());
                answer.map_err(|e| stale(e, method, protocol))
            }
            Ok(Err(_)) => Err(self.inner.why()),
            Err(_) => {
                let e = YardError::internal(format!(
                    "yardd did not answer {method} within {}",
                    seconds(self.inner.timeout)
                ))
                .with_hint("yard doctor checks what is stuck");
                self.inner.shut(e.clone());
                Err(e)
            }
        }
    }

    pub async fn subscribe(&self, topics: &[Topic]) -> Result<(), YardError> {
        self.call_value(SUBSCRIBE, to_value(SubscribeParams { topics: topics.to_vec() })?).await.map(|_| ())
    }

    /// Closes the connection: pending calls fail and the event channel ends.
    pub fn close(&self) {
        self.inner.shut(YardError::internal("the connection to yardd was closed"));
    }

    /// Resolves, with why, once the connection has closed.
    pub fn closed(&self) -> impl std::future::Future<Output = YardError> + Send + 'static {
        let mut rx = self.inner.closed.subscribe();
        async move {
            loop {
                if let Some(e) = rx.borrow_and_update().clone() {
                    return e;
                }
                if rx.changed().await.is_err() {
                    return closed_by_daemon();
                }
            }
        }
    }
}

impl Inner {
    /// Why the connection closed, once it has.
    fn why(&self) -> YardError {
        self.closed.borrow().clone().unwrap_or_else(closed_by_daemon)
    }

    /// Records the first reason, fails every pending call with it, and stops reading and writing.
    fn shut(&self, why: YardError) {
        self.closed.send_if_modified(|v| {
            if v.is_some() {
                return false;
            }
            *v = Some(why);
            true
        });
        let why = self.why();
        if let Some(pending) = lock(&self.pending).take() {
            for (_, tx) in pending {
                let _ = tx.send(Err(why.clone()));
            }
        }
        if let Ok(mut w) = self.write.try_lock() {
            w.take();
        }
        if let Some(reader) = lock(&self.reader).take() {
            reader.abort();
        }
    }
}

async fn read_loop(inner: Arc<Inner>, read: OwnedReadHalf, events: mpsc::UnboundedSender<Event>) {
    let mut reader = BufReader::with_capacity(64 * 1024, read);
    let mut buf = Vec::new();
    let why = loop {
        buf.clear();
        let n = match (&mut reader).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf).await {
            Ok(n) => n,
            Err(e) => break lost(e),
        };
        if buf.last() != Some(&b'\n') {
            if buf.len() > MAX_LINE {
                break YardError::internal(format!("yardd sent a line longer than {} MiB", MAX_LINE >> 20))
                    .with_hint("yard doctor checks why");
            }
            // The end of the stream, with or without half a line before it.
            let _ = n;
            break closed_by_daemon();
        }
        let line = buf.trim_ascii();
        if line.is_empty() {
            continue;
        }
        if let Ok(note) = serde_json::from_slice::<Notification>(line) {
            if note.params != Event::Unknown {
                let _ = events.send(note.params);
            }
            continue;
        }
        if let Ok(response) = serde_json::from_slice::<Response>(line) {
            let waiter = lock(&inner.pending).as_mut().and_then(|p| p.remove(&response.id));
            // An answer nobody waits for (its call timed out) is dropped.
            if let Some(tx) = waiter {
                let _ = tx.send(response.into_result());
            }
        }
    };
    // This task is ending, so dropping its handle in `shut` is harmless.
    lock(&inner.reader).take();
    inner.shut(why);
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn to_value<P: Serialize>(params: P) -> Result<Value, YardError> {
    serde_json::to_value(params).map_err(YardError::internal)
}

fn shape<R: DeserializeOwned>(method: &str, value: Value) -> Result<R, YardError> {
    serde_json::from_value(value).map_err(|e| {
        YardError::new(
            ErrorCode::ProtocolMismatch,
            format!("yardd answered {method} in an unexpected shape: {e}"),
        )
        .with_hint("upgrade yard and yardd together")
    })
}

fn lost(e: std::io::Error) -> YardError {
    YardError::internal(format!("lost the connection to yardd: {e}")).with_hint("yard doctor checks why")
}

fn closed_by_daemon() -> YardError {
    YardError::internal("yardd closed the connection").with_hint("yard doctor checks why")
}

/// `60 s`, or `50 ms` below a second.
fn seconds(d: Duration) -> String {
    if d >= Duration::from_secs(1) { format!("{} s", d.as_secs()) } else { format!("{} ms", d.as_millis()) }
}
