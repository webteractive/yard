//! `FakeYardd`: a yardd stand-in on a Unix socket under `/tmp/yard-test-<random>`, scripted per
//! test. A method with no answer set is never answered, so a test can make a call time out.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use yard_proto::rpc::{Request, Response, encode_line};
use yard_proto::{Event, Topic, YardError};

struct Conn {
    id: usize,
    out: mpsc::UnboundedSender<Vec<Vec<u8>>>,
    topics: HashSet<Topic>,
    tasks: Vec<JoinHandle<()>>,
}

#[derive(Default)]
struct State {
    answers: HashMap<String, Result<Value, YardError>>,
    delays: HashMap<String, Duration>,
    protocol: Option<String>,
    refuse_hello: Option<YardError>,
    out_of_order: bool,
    held: Option<(usize, Vec<u8>)>,
    requests: Vec<(usize, String, Value)>,
    conns: Vec<Conn>,
    next_conn: usize,
}

pub struct FakeYardd {
    dir: PathBuf,
    state: Arc<Mutex<State>>,
    accept: Mutex<Option<JoinHandle<()>>>,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

impl FakeYardd {
    /// Binds `/tmp/yard-test-<random>/yardd.sock` and starts answering.
    pub fn start_new() -> FakeYardd {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let dir = PathBuf::from(format!(
            "/tmp/yard-test-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = FakeYardd { dir, state: Arc::default(), accept: Mutex::new(None) };
        fake.start();
        fake
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("yardd.sock")
    }

    pub fn answer(&self, method: &str, result: Value) {
        self.state.lock().unwrap().answers.insert(method.into(), Ok(result));
    }

    pub fn fail(&self, method: &str, error: YardError) {
        self.state.lock().unwrap().answers.insert(method.into(), Err(error));
    }

    pub fn delay(&self, method: &str, by: Duration) {
        self.state.lock().unwrap().delays.insert(method.into(), by);
    }

    /// What `hello` answers as the protocol; `2.11` unless set.
    pub fn protocol(&self, protocol: &str) {
        self.state.lock().unwrap().protocol = Some(protocol.into());
    }

    pub fn refuse_hello(&self, error: YardError) {
        self.state.lock().unwrap().refuse_hello = Some(error);
    }

    /// Undoes `refuse_hello`.
    pub fn allow_hello(&self) {
        self.state.lock().unwrap().refuse_hello = None;
    }

    /// Holds each answer until the next request arrives, then sends that one's answer first.
    pub fn answer_out_of_order(&self) {
        self.state.lock().unwrap().out_of_order = true;
    }

    /// `(connection, method, params)` of every request, in arrival order; connections count
    /// from 0 in the order they were accepted.
    pub fn requests(&self) -> Vec<(usize, String, Value)> {
        self.state.lock().unwrap().requests.clone()
    }

    /// Sends an event to every connection subscribed to its topic.
    pub fn push(&self, event: Value) {
        let topic = serde_json::from_value::<Event>(event.clone()).ok().and_then(|e| e.topic());
        let line = encode_line(&json!({"jsonrpc": "2.0", "method": "event", "params": event}));
        let state = self.state.lock().unwrap();
        for c in state.conns.iter().filter(|c| topic.is_some_and(|t| c.topics.contains(&t))) {
            let _ = c.out.send(vec![line.clone()]);
        }
    }

    /// Writes `bytes` to every connection, `chunk` bytes per write (0 is one write).
    pub fn send_raw(&self, bytes: &[u8], chunk: usize) {
        let pieces: Vec<Vec<u8>> =
            if chunk == 0 { vec![bytes.to_vec()] } else { bytes.chunks(chunk).map(<[u8]>::to_vec).collect() };
        for c in &self.state.lock().unwrap().conns {
            let _ = c.out.send(pieces.clone());
        }
    }

    /// Closes every connection; the socket keeps accepting.
    pub fn drop_connections(&self) {
        for c in self.state.lock().unwrap().conns.drain(..) {
            c.tasks.iter().for_each(JoinHandle::abort);
        }
    }

    /// Stops accepting, closes every connection and removes the socket, as a yardd that quit.
    pub fn stop(&self) {
        if let Some(a) = self.accept.lock().unwrap().take() {
            a.abort();
        }
        self.drop_connections();
        let _ = std::fs::remove_file(self.socket());
    }

    /// Binds the socket again (after `stop`) and accepts.
    pub fn start(&self) {
        let _ = std::fs::remove_file(self.socket());
        let listener = UnixListener::bind(self.socket()).unwrap();
        let state = self.state.clone();
        *self.accept.lock().unwrap() = Some(tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serve(state.clone(), stream);
            }
        }));
    }
}

impl Drop for FakeYardd {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn serve(state: Arc<Mutex<State>>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let (out, mut rx) = mpsc::unbounded_channel::<Vec<Vec<u8>>>();
    let writer = tokio::spawn(async move {
        while let Some(pieces) = rx.recv().await {
            let few = pieces.len() <= 64;
            for p in pieces {
                if write.write_all(&p).await.is_err() || write.flush().await.is_err() {
                    return;
                }
                // Separate writes, so the client sees separate reads: a pause between a few, a
                // yield between many (300,000 one-byte writes would take a minute of pauses).
                if few {
                    tokio::time::sleep(Duration::from_micros(200)).await;
                } else {
                    tokio::task::yield_now().await;
                }
            }
        }
    });
    let id = {
        let mut s = state.lock().unwrap();
        s.next_conn += 1;
        s.next_conn - 1
    };
    let reader_state = state.clone();
    let reader_out = out.clone();
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(req) = serde_json::from_str::<Request>(&line) else { continue };
            handle(&reader_state, id, &reader_out, req);
        }
    });
    state.lock().unwrap().conns.push(Conn { id, out, topics: HashSet::new(), tasks: vec![writer, reader] });
}

fn handle(state: &Arc<Mutex<State>>, conn: usize, out: &mpsc::UnboundedSender<Vec<Vec<u8>>>, req: Request) {
    let Some(rid) = req.id else { return };
    let mut s = state.lock().unwrap();
    s.requests.push((conn, req.method.clone(), req.params.clone()));
    let result: Option<Result<Value, YardError>> = match req.method.as_str() {
        "hello" => Some(match s.refuse_hello.clone() {
            Some(e) => Err(e),
            None => Ok(json!({
                "protocol": s.protocol.clone().unwrap_or_else(|| "2.11".into()),
                "daemon_version": "0.1.0",
                "commit": "fake",
            })),
        }),
        "subscribe" => {
            let topics: Vec<Topic> = serde_json::from_value(req.params["topics"].clone()).unwrap_or_default();
            if let Some(c) = s.conns.iter_mut().find(|c| c.id == conn) {
                c.topics.extend(topics);
            }
            Some(Ok(Value::Null))
        }
        m => s.answers.get(m).cloned(),
    };
    let Some(result) = result else { return };
    let line = encode_line(&match result {
        Ok(v) => Response::ok(rid, v),
        Err(e) => Response::err(rid, e),
    });
    if let Some(by) = s.delays.get(&req.method).copied() {
        let out = out.clone();
        tokio::spawn(async move {
            tokio::time::sleep(by).await;
            let _ = out.send(vec![line]);
        });
        return;
    }
    if s.out_of_order && req.method != "hello" {
        match s.held.take() {
            None => {
                s.held = Some((conn, line));
                return;
            }
            Some((_, first)) => {
                let _ = out.send(vec![line, first]);
                return;
            }
        }
    }
    let _ = out.send(vec![line]);
}

/// A path that does not exist, under `/tmp`.
pub fn missing_socket() -> PathBuf {
    Path::new("/tmp").join(format!("yard-test-missing-{}", std::process::id())).join("yardd.sock")
}
