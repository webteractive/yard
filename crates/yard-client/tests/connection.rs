//! `Connection` against `FakeYardd`: framing, ids, events, timeouts and the client's errors.

mod support;

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;
use support::FakeYardd;
use yard_client::api;
use yard_client::connection::{Connection, MAX_LINE};
use yard_proto::methods::{PhpListParams, PhpParams};
use yard_proto::rpc::{Response, encode_line};
use yard_proto::{ErrorCode, Event, PROTOCOL_VERSION, Topic, YardError};

const NAME: &str = "Yard.app test";
const SECOND: Duration = Duration::from_secs(1);

async fn open(fake: &FakeYardd) -> (Connection, tokio::sync::mpsc::UnboundedReceiver<Event>) {
    Connection::open(&fake.socket(), NAME, Duration::from_secs(5)).await.unwrap()
}

/// Waits until the fake has seen `method`.
async fn requested(fake: &FakeYardd, method: &str) {
    for _ in 0..500 {
        if fake.requests().iter().any(|(_, m, _)| m == method) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("the fake never saw {method}: {:?}", fake.requests());
}

fn site(i: usize) -> Value {
    json!({
        "name": format!("site{i}"), "host": format!("site{i}.yard"), "url": format!("http://site{i}.yard:8080/"),
        "source": "linked", "kind": "php", "path": format!("/Users/me/Sites/site{i}"),
        "docroot": format!("/Users/me/Sites/site{i}/public"), "php": "8.4", "secure": false,
    })
}

fn progress(line: &str) -> Value {
    json!({"type": "job_progress", "job_id": "php-install-1", "line": line})
}

fn notification(event: Value) -> Vec<u8> {
    encode_line(&json!({"jsonrpc": "2.0", "method": "event", "params": event}))
}

#[tokio::test]
async fn lines_survive_any_chunking() {
    let sites: Value = (0..1500).map(site).collect();
    let mut bytes = encode_line(&Response::ok(2, sites.clone()));
    assert!(bytes.len() > 300_000, "{} bytes", bytes.len());
    bytes.extend(notification(progress("one")));
    bytes.extend(notification(progress("two")));
    for chunk in [1, 2, 3, 4, 5, 6, 7, 0] {
        let fake = FakeYardd::start_new();
        let (conn, mut events) = open(&fake).await;
        let call = tokio::spawn({
            let conn = conn.clone();
            async move { conn.call(api::SITES_LIST, ()).await }
        });
        requested(&fake, "sites.list").await;
        fake.send_raw(&bytes, chunk);
        let got = tokio::time::timeout(Duration::from_secs(30), call).await.unwrap().unwrap().unwrap();
        assert_eq!(got.len(), 1500, "chunk {chunk}");
        for want in ["one", "two"] {
            let e = tokio::time::timeout(SECOND, events.recv()).await.unwrap().unwrap();
            assert!(matches!(&e, Event::JobProgress { line, .. } if line == want), "chunk {chunk}: {e:?}");
        }
    }
}

#[tokio::test]
async fn an_over_long_line_closes_the_connection() {
    let fake = FakeYardd::start_new();
    let (conn, mut events) = open(&fake).await;
    let call = tokio::spawn({
        let conn = conn.clone();
        async move { conn.call(api::STATUS, ()).await }
    });
    requested(&fake, "status").await;
    fake.send_raw(&vec![b'x'; MAX_LINE + 1], 0);
    let e = tokio::time::timeout(Duration::from_secs(10), call).await.unwrap().unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal, "{e:?}");
    let closed = tokio::time::timeout(SECOND, conn.closed()).await.unwrap();
    assert_eq!(closed, e);
    assert!(tokio::time::timeout(SECOND, events.recv()).await.unwrap().is_none(), "the events end too");
}

#[tokio::test]
async fn empty_lines_are_skipped() {
    let fake = FakeYardd::start_new();
    let (conn, _events) = open(&fake).await;
    let call = tokio::spawn({
        let conn = conn.clone();
        async move { conn.call(api::SITES_LIST, ()).await }
    });
    requested(&fake, "sites.list").await;
    let mut bytes = b"\n\r\n\n".to_vec();
    bytes.extend(encode_line(&Response::ok(2, json!([]))));
    fake.send_raw(&bytes, 0);
    assert_eq!(call.await.unwrap().unwrap(), vec![]);
}

#[tokio::test]
async fn hello_is_the_first_request() {
    let fake = FakeYardd::start_new();
    let (conn, _events) = open(&fake).await;
    let first = fake.requests().into_iter().next().unwrap();
    assert_eq!(first.1, "hello");
    assert_eq!(first.2, json!({"protocol": PROTOCOL_VERSION, "client": NAME}));
    assert_eq!(conn.hello().protocol, "2.11");
}

#[tokio::test]
async fn major_mismatch_is_returned() {
    let fake = FakeYardd::start_new();
    let refusal =
        YardError::new(ErrorCode::ProtocolMismatch, "yardd speaks protocol 3.0").with_hint("upgrade");
    fake.refuse_hello(refusal.clone());
    let e = Connection::open(&fake.socket(), NAME, SECOND).await.unwrap_err();
    assert_eq!(e, refusal);
}

#[tokio::test]
async fn answers_are_matched_by_id() {
    let fake = FakeYardd::start_new();
    fake.answer(
        "php.list",
        json!([{"version": "8.4", "default": true, "installed": true, "sites": [], "pool": null}]),
    );
    fake.answer("sites.list", json!([site(1)]));
    // php.list's answer is held until sites.list arrives, then sent after sites.list's.
    fake.answer_out_of_order();
    let (conn, _events) = open(&fake).await;
    let php = tokio::spawn({
        let conn = conn.clone();
        async move { conn.call(api::PHP_LIST, PhpListParams::default()).await }
    });
    requested(&fake, "php.list").await;
    let sites = conn.call(api::SITES_LIST, ()).await.unwrap();
    assert_eq!(sites[0].name, "site1");
    let php = tokio::time::timeout(SECOND, php).await.unwrap().unwrap().unwrap();
    assert_eq!(php[0].version, "8.4");
}

#[tokio::test]
async fn an_event_before_the_answer_is_an_event() {
    let fake = FakeYardd::start_new();
    fake.answer("php.install", json!({"job_id": "php-install-1"}));
    fake.delay("php.install", Duration::from_millis(150));
    let (conn, mut events) = open(&fake).await;
    conn.subscribe(&[Topic::Jobs]).await.unwrap();
    let call = tokio::spawn({
        let conn = conn.clone();
        async move { conn.call(api::PHP_INSTALL, PhpParams { php: "8.3".into() }).await }
    });
    requested(&fake, "php.install").await;
    fake.push(progress("==> Downloading"));
    let e = tokio::time::timeout(SECOND, events.recv()).await.unwrap().unwrap();
    assert!(matches!(&e, Event::JobProgress { line, .. } if line == "==> Downloading"), "{e:?}");
    assert!(!call.is_finished(), "the event arrived before the answer");
    assert_eq!(call.await.unwrap().unwrap().job_id, "php-install-1");
}

#[tokio::test]
async fn a_silent_call_times_out() {
    let fake = FakeYardd::start_new();
    let (conn, _events) = Connection::open(&fake.socket(), NAME, Duration::from_millis(50)).await.unwrap();
    let e = conn.call(api::STATUS, ()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal);
    assert!(e.message.starts_with("yardd did not answer status within"), "{}", e.message);
    assert_eq!(e.hint.as_deref(), Some("yard doctor checks what is stuck"));
}

#[tokio::test]
async fn a_write_yardd_never_reads_times_out() {
    let fake = FakeYardd::start_new();
    let (conn, _events) = Connection::open(&fake.socket(), NAME, Duration::from_millis(200)).await.unwrap();
    fake.stop_reading();
    // Far more than a socket buffer holds, so the write cannot finish.
    let big = json!({"pad": "x".repeat(8 << 20)});
    let e = tokio::time::timeout(SECOND * 5, conn.call_value("status", big))
        .await
        .expect("the write hung")
        .unwrap_err();
    assert!(e.message.starts_with("yardd did not answer status within"), "{}", e.message);
}

#[tokio::test]
async fn stale_method_not_found_names_the_restart() {
    let fake = FakeYardd::start_new();
    fake.protocol("2.10");
    fake.fail("php.list", YardError::new(ErrorCode::MethodNotFound, "yardd has no method \"php.list\""));
    let (conn, _events) = open(&fake).await;
    let e = conn.call(api::PHP_LIST, PhpListParams::default()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::MethodNotFound);
    assert_eq!(e.message, "yardd speaks protocol 2.10 and has no php.list");
    assert_eq!(e.hint.as_deref(), Some("yard stop && yard start"));
}

#[tokio::test]
async fn pending_calls_fail_when_the_daemon_goes() {
    let fake = FakeYardd::start_new();
    let (conn, _events) = open(&fake).await;
    let call = tokio::spawn({
        let conn = conn.clone();
        async move { conn.call(api::STATUS, ()).await }
    });
    requested(&fake, "status").await;
    fake.drop_connections();
    let e = tokio::time::timeout(SECOND, call).await.unwrap().unwrap().unwrap_err();
    assert_eq!(e.message, "yardd closed the connection");
    assert_eq!(e.hint.as_deref(), Some("yard doctor checks why"));
}

#[tokio::test]
async fn missing_socket_is_not_running() {
    let e = Connection::open(&support::missing_socket(), NAME, SECOND).await.unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str(), e.hint.as_deref()),
        (ErrorCode::DaemonNotRunning, "yardd is not running", Some("yard start"))
    );
}

#[tokio::test]
async fn long_path_is_refused() {
    let path = Path::new("/tmp").join("y".repeat(110)).join("yardd.sock");
    let e = Connection::open(&path, NAME, SECOND).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    assert!(e.message.ends_with("is longer than a unix socket path may be (103 bytes)"), "{}", e.message);
}

#[tokio::test]
async fn a_path_of_exactly_the_limit_is_refused() {
    // sun_path holds 104 bytes with its terminating NUL, so 104 bytes of path do not fit.
    let path =
        Path::new("/tmp").join("y".repeat(104 - "/tmp/".len() - "/yardd.sock".len())).join("yardd.sock");
    assert_eq!(path.as_os_str().len(), 104);
    let e = Connection::open(&path, NAME, SECOND).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument, "{}", e.message);
}

#[tokio::test]
async fn daemon_errors_pass_through() {
    let fake = FakeYardd::start_new();
    let held =
        YardError::new(ErrorCode::PortInUse, "port 8080 is held by nginx (pid 1)").with_hint("yard ports");
    fake.fail("sites.list", held.clone());
    let (conn, _events) = open(&fake).await;
    assert_eq!(conn.call(api::SITES_LIST, ()).await.unwrap_err(), held);
}

#[test]
fn api_covers_every_method() {
    let mut covered: BTreeSet<&str> = api::NAMES.iter().copied().collect();
    covered.extend(["hello", "subscribe"]);
    let all: BTreeSet<&str> = yard_proto::methods::ALL.iter().copied().collect();
    assert_eq!(covered, all);
    assert_eq!(api::NAMES.len(), all.len() - 2, "each name once");
}
