//! Test doubles for the app's state: `FakeDaemon` answers on GPUI's executor, so timing tests stay
//! deterministic; sample data is built from yard-proto's types.
#![cfg(test)]

use crate::daemon::{BoxFuture, Daemon};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use yard_client::session::{ConnectionState, SessionInfo, Update};
use yard_proto::methods::HelloResult;
use yard_proto::{ErrorCode, Event, YardError};

type Answer = Result<Value, YardError>;

#[derive(Default)]
struct Inner {
    /// Answers in order; the last one repeats.
    answers: HashMap<String, VecDeque<Answer>>,
    holding: HashSet<String>,
    held: HashMap<String, Vec<oneshot::Sender<Answer>>>,
    calls: Vec<(String, Value)>,
    no_events: bool,
}

pub struct FakeDaemon {
    inner: Mutex<Inner>,
    tx: mpsc::UnboundedSender<Update>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Update>>>,
}

impl FakeDaemon {
    pub fn new() -> Arc<FakeDaemon> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(FakeDaemon { inner: Mutex::default(), tx, rx: Mutex::new(Some(rx)) })
    }

    pub fn answer(&self, name: &str, value: Value) {
        self.answers(name, vec![Ok(value)]);
    }

    pub fn fail(&self, name: &str, e: YardError) {
        self.answers(name, vec![Err(e)]);
    }

    pub fn answers(&self, name: &str, answers: Vec<Answer>) {
        self.inner.lock().unwrap().answers.insert(name.into(), answers.into());
    }

    /// Calls to `name` wait until `release`.
    pub fn hold(&self, name: &str) {
        self.inner.lock().unwrap().holding.insert(name.into());
    }

    pub fn release(&self, name: &str, answer: Answer) {
        let mut inner = self.inner.lock().unwrap();
        inner.holding.remove(name);
        for tx in inner.held.remove(name).unwrap_or_default() {
            let _ = tx.send(answer.clone());
        }
    }

    pub fn without_events(&self) {
        self.inner.lock().unwrap().no_events = true;
    }

    pub fn push(&self, update: Update) {
        self.tx.send(update).unwrap();
    }

    pub fn event(&self, event: Event) {
        self.push(Update::Event(event));
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.inner.lock().unwrap().calls.clone()
    }

    pub fn count(&self, name: &str) -> usize {
        self.calls().iter().filter(|(n, _)| n == name).count()
    }
}

impl Daemon for FakeDaemon {
    fn call_value(&self, name: &'static str, _slow: bool, params: Value) -> BoxFuture<'static, Answer> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push((name.to_string(), params));
        if inner.holding.contains(name) {
            let (tx, rx) = oneshot::channel();
            inner.held.entry(name.to_string()).or_default().push(tx);
            return Box::pin(async move { rx.await.unwrap_or_else(|_| Err(YardError::internal("dropped"))) });
        }
        let answer = match inner.answers.get_mut(name) {
            Some(q) if q.len() > 1 => q.pop_front().unwrap(),
            Some(q) => q.front().cloned().unwrap(),
            None => Err(YardError::new(ErrorCode::Internal, format!("the fake has no answer for {name}"))),
        };
        Box::pin(async move { answer })
    }

    fn take_updates(&self) -> mpsc::UnboundedReceiver<Update> {
        self.rx.lock().unwrap().take().expect("updates are taken once")
    }

    fn reconnect_now(&self) {}

    fn can_follow_jobs(&self) -> bool {
        !self.inner.lock().unwrap().no_events
    }

    fn logs(&self) -> BoxFuture<'static, Result<mpsc::UnboundedReceiver<Event>, YardError>> {
        let (_tx, rx) = mpsc::unbounded_channel();
        Box::pin(async move { Ok(rx) })
    }
}

pub fn connected(epoch: u64) -> Update {
    let hello =
        HelloResult { protocol: "2.11".into(), daemon_version: "0.1.0".into(), commit: "abc1234".into() };
    Update::State(ConnectionState::Connected(SessionInfo { epoch, hello, stale: false }))
}

pub fn status() -> Value {
    json!({
        "version": {"cli": null, "daemon": "0.1.0", "helper": "0.1.0", "protocol": "2.11", "commit": "abc1234", "build_date": "2026-11-06"},
        "profile": "standard", "tld": "yard", "units": [
            {"name": "nginx", "state": "running", "pid": 10, "restarts": 0, "last_error": null},
            {"name": "redis", "state": "running", "pid": 11, "restarts": 0, "last_error": null}
        ],
        "sites": 1, "config_error": null, "sleeping": []
    })
}

pub fn site(name: &str, needs: Value) -> Value {
    json!({
        "name": name, "host": format!("{name}.yard"), "url": format!("http://{name}.yard:8080/"), "source": "linked",
        "kind": "php", "path": format!("/Users/me/Sites/{name}"), "docroot": format!("/Users/me/Sites/{name}/public"),
        "php": "7.3", "needs": needs
    })
}

pub fn job(id: &str) -> Value {
    json!({"job_id": id})
}

pub fn finished(id: &str, error: Option<YardError>) -> Event {
    Event::JobFinished { job_id: id.into(), error, result: None }
}

/// An `AppEnv` over a `FakeDaemon`, connected, with no `yard` located.
pub fn env(cx: &mut gpui::TestAppContext) -> (Arc<FakeDaemon>, crate::env::AppEnv) {
    let fake = FakeDaemon::new();
    fake.answer("status", status());
    let env = cx.update(|cx| {
        // Any path chosen is taken as executable.
        let locate_yard: crate::env::LocateYard = std::rc::Rc::new(|setting| setting);
        crate::env::AppEnv::new(
            fake.clone(),
            test_location(),
            locate_yard,
            Arc::new(crate::bridge::Bridge::new()),
            cx,
        )
    });
    fake.push(connected(1));
    cx.run_until_parked();
    (fake, env)
}

pub fn test_location() -> yard_app_model::locate::Location {
    yard_app_model::locate::Location {
        root: std::path::PathBuf::from("/tmp/yard-test-root"),
        root_is_default: false,
        yard: None,
        agent_installed: false,
    }
}

pub fn services() -> Value {
    json!([
        {"name": "redis", "engine": "redis", "version": null, "formula": "redis", "ports": [{"name": "redis", "port": 6380}],
         "state": "running", "stopped_by_user": false, "account": {"user": null, "password": null}, "used_by": ["blog"],
         "sleep_after_secs": 900},
        {"name": "mysql-8.4", "engine": "mysql", "version": "8.4", "formula": "mysql@8.4", "ports": [{"name": "mysql-8.4", "port": 3307}],
         "state": {"failed": {"last_lines": ["boom"], "reason": {"code": "PORT_IN_USE", "message": "port 3307 is held by mysqld (pid 9)", "hint": "yard service port mysql-8.4 <port>"}}},
         "stopped_by_user": false, "account": {"user": "root", "password": null}, "used_by": [], "sleep_after_secs": 900},
        {"name": "emailulator", "engine": "emailulator", "version": "0.1.4", "formula": "", "ports": [{"name": "emailulator-smtp", "port": 2525}],
         "state": "running", "stopped_by_user": false, "account": {"user": null, "password": null}, "used_by": [],
         "managed_by": {"kind": "launch_agent", "label": "co.webteractive.emailulator", "pid": 5}}
    ])
}
