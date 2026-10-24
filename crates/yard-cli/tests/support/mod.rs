//! A mock `yardd`: NDJSON on `<root>/yardd.sock` with canned results per method, and a
//! record of every request, so `yard --root <root>` can run against it.
#![allow(dead_code)]

use assert_cmd::Command;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use yard_proto::rpc::{Request, Response, encode_line};
use yard_proto::{ErrorCode, PROTOCOL_VERSION, YardError};

pub const MOCK_VERSION: &str = "9.9.9";

/// Each method's answers in order; the last one repeats.
type Answers = Mutex<HashMap<String, VecDeque<Result<Value, YardError>>>>;
/// Event payloads pushed, in order, right after the answer to a method.
type Events = HashMap<String, Vec<Value>>;

pub struct MockDaemon {
    pub root: TempDir,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl MockDaemon {
    pub fn start(answers: &[(&str, Result<Value, YardError>)]) -> Self {
        Self::start_with_events(answers, &[])
    }

    /// As `start`, and after answering a method in `events` it pushes those events.
    pub fn start_with_events(
        answers: &[(&str, Result<Value, YardError>)],
        events: &[(&str, Vec<Value>)],
    ) -> Self {
        let answers: Vec<(&str, Vec<Result<Value, YardError>>)> =
            answers.iter().map(|(m, a)| (*m, vec![a.clone()])).collect();
        Self::serve(&answers, events)
    }

    /// As `start`, with answers given in turn to each call of a method; the last repeats.
    pub fn start_sequenced(answers: &[(&str, Vec<Result<Value, YardError>>)]) -> Self {
        Self::serve(answers, &[])
    }

    /// As `start_sequenced`, and after answering a method in `events` it pushes those events.
    pub fn start_sequenced_with_events(
        answers: &[(&str, Vec<Result<Value, YardError>>)],
        events: &[(&str, Vec<Value>)],
    ) -> Self {
        Self::serve(answers, events)
    }

    fn serve(answers: &[(&str, Vec<Result<Value, YardError>>)], events: &[(&str, Vec<Value>)]) -> Self {
        let root = short_root();
        let events: Arc<Events> = Arc::new(events.iter().map(|(m, e)| (m.to_string(), e.clone())).collect());
        let listener = UnixListener::bind(root.path().join("yardd.sock")).unwrap();
        let answers: Arc<Answers> = Arc::new(Mutex::new(
            answers.iter().map(|(m, a)| (m.to_string(), a.iter().cloned().collect())).collect(),
        ));
        let requests = Arc::new(Mutex::new(vec![]));
        let seen = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (answers, events, seen) = (answers.clone(), events.clone(), seen.clone());
                std::thread::spawn(move || serve(stream, &answers, &events, &seen));
            }
        });
        Self { root, requests }
    }

    /// The params of every request for `method`, in order.
    pub fn params(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method)
            .map(|r| r.params.clone())
            .collect()
    }

    pub fn yard(&self) -> Command {
        yard_at(self.root.path())
    }

    /// As `yard`, as a plain process, for a test that signals it while it runs.
    pub fn yard_process(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_yard"));
        cmd.env_remove("YARD_ROOT").arg("--root").arg(self.root.path());
        cmd
    }
}

fn serve(stream: UnixStream, answers: &Answers, events: &Events, seen: &Mutex<Vec<Request>>) {
    let mut write = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let request: Request = serde_json::from_str(&line).unwrap();
        seen.lock().unwrap().push(request.clone());
        let id = request.id.unwrap_or_default();
        let response = match request.method.as_str() {
            "hello" => Response::ok(
                id,
                json!({"protocol": PROTOCOL_VERSION, "daemon_version": MOCK_VERSION, "commit": "mock"}),
            ),
            method => match next_answer(answers, method) {
                Some(Ok(value)) => Response::ok(id, value),
                Some(Err(e)) => Response::err(id, e),
                None => Response::err(
                    id,
                    YardError::new(ErrorCode::MethodNotFound, format!("mock has no {method}")),
                ),
            },
        };
        if write.write_all(&encode_line(&response)).is_err() {
            return;
        }
        for event in events.get(&request.method).into_iter().flatten() {
            let note = json!({"jsonrpc": "2.0", "method": "event", "params": event});
            if write.write_all(&encode_line(&note)).is_err() {
                return;
            }
        }
    }
}

fn next_answer(answers: &Answers, method: &str) -> Option<Result<Value, YardError>> {
    let mut answers = answers.lock().unwrap();
    let queue = answers.get_mut(method)?;
    if queue.len() > 1 { queue.pop_front() } else { queue.front().cloned() }
}

/// A data root short enough for a unix socket path.
pub fn short_root() -> TempDir {
    tempfile::Builder::new().prefix("yard-test-").tempdir_in("/tmp").unwrap()
}

/// `yard --root <root>`, with no `YARD_ROOT` leaking in from the environment. A command
/// still running after 60 s is killed, so a hang fails its test instead of stalling the suite.
pub fn yard_at(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("yard").unwrap();
    cmd.env_remove("YARD_ROOT").arg("--root").arg(root).timeout(std::time::Duration::from_secs(60));
    cmd
}

pub fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// An OS-assigned port, released at once.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
