//! `Session` against `FakeYardd`: which connection does what, reconnecting, and epochs.

mod support;

use serde_json::json;
use std::time::{Duration, Instant};
use support::FakeYardd;
use tokio::sync::mpsc::UnboundedReceiver;
use yard_client::api;
use yard_client::session::{ConnectionState, Session, SessionConfig, SessionInfo, Update};
use yard_proto::methods::DoctorParams;
use yard_proto::{ErrorCode, Event, YardError};

fn config(fake: &FakeYardd) -> SessionConfig {
    SessionConfig {
        backoff: vec![Duration::from_millis(10), Duration::from_millis(20)],
        timeout: Duration::from_secs(5),
        ..SessionConfig::new(fake.socket(), "Yard.app test".into())
    }
}

/// The next state update, skipping events, within 5 s.
async fn next_state(updates: &mut UnboundedReceiver<Update>) -> ConnectionState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, updates.recv()).await {
            Ok(Some(Update::State(s))) => return s,
            Ok(Some(Update::Event(_))) => continue,
            other => panic!("no state update: {other:?}"),
        }
    }
}

async fn connected(updates: &mut UnboundedReceiver<Update>) -> SessionInfo {
    loop {
        match next_state(updates).await {
            ConnectionState::Connected(info) => return info,
            ConnectionState::Connecting | ConnectionState::Disconnected(_) => continue,
            s => panic!("expected Connected, got {s:?}"),
        }
    }
}

#[tokio::test]
async fn subscribes_before_the_first_call() {
    let fake = FakeYardd::start_new();
    fake.answer("sites.list", json!([]));
    let (session, mut updates) = Session::start(config(&fake));
    connected(&mut updates).await;
    session.call(api::SITES_LIST, ()).await.unwrap();
    let requests = fake.requests();
    let subscribe = requests.iter().position(|(_, m, _)| m == "subscribe").expect("a subscribe");
    let events_conn = requests[subscribe].0;
    let first_other = requests.iter().position(|(c, _, _)| *c != events_conn).expect("a second connection");
    assert!(subscribe < first_other, "{requests:?}");
}

#[tokio::test]
async fn stale_daemon_subscribes_without_state() {
    let fake = FakeYardd::start_new();
    fake.protocol("2.10");
    let (_session, mut updates) = Session::start(config(&fake));
    let info = connected(&mut updates).await;
    assert!(info.stale);
    let (_, _, params) = fake.requests().into_iter().find(|(_, m, _)| m == "subscribe").unwrap();
    assert_eq!(params, json!({"topics": ["units", "sites", "ports", "jobs", "tunnels"]}));
}

#[tokio::test]
async fn reconnects_with_a_new_epoch() {
    let fake = FakeYardd::start_new();
    let (_session, mut updates) = Session::start(config(&fake));
    assert_eq!(connected(&mut updates).await.epoch, 1);
    fake.stop();
    assert!(matches!(next_state(&mut updates).await, ConnectionState::Disconnected(_)));
    fake.start();
    let info = connected(&mut updates).await;
    assert_eq!(info.epoch, 2);
    assert!(!info.stale);
    let subscribes = fake.requests().iter().filter(|(_, m, _)| m == "subscribe").count();
    assert_eq!(subscribes, 2);
}

/// `reconnect_now` while connected keeps the connection (and the jobs it follows): the app calls
/// it after any `yard start`, which leaves a running yardd alone (found by the M6 final review).
#[tokio::test]
async fn reconnect_now_while_connected_keeps_the_connection() {
    let fake = FakeYardd::start_new();
    fake.answer("sites.list", json!([]));
    let (session, mut updates) = Session::start(config(&fake));
    assert_eq!(connected(&mut updates).await.epoch, 1);
    session.reconnect_now();
    session.reconnect_now();
    let quiet = tokio::time::timeout(Duration::from_millis(300), next_state(&mut updates)).await;
    assert!(quiet.is_err(), "no state change: {quiet:?}");
    session.call(api::SITES_LIST, ()).await.unwrap();
    // A permit left from those calls must not cut the next connection short either.
    fake.stop();
    assert!(matches!(next_state(&mut updates).await, ConnectionState::Disconnected(_)));
    fake.start();
    assert_eq!(connected(&mut updates).await.epoch, 2);
    let quiet = tokio::time::timeout(Duration::from_millis(300), next_state(&mut updates)).await;
    assert!(quiet.is_err(), "epoch 2 stays up: {quiet:?}");
}

#[tokio::test]
async fn pending_calls_fail_on_disconnect() {
    let fake = FakeYardd::start_new();
    let (session, mut updates) = Session::start(config(&fake));
    connected(&mut updates).await;
    let call = tokio::spawn({
        let session = session.clone();
        async move { session.call(api::SITES_LIST, ()).await }
    });
    while !fake.requests().iter().any(|(_, m, _)| m == "sites.list") {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    fake.drop_connections();
    let e = tokio::time::timeout(Duration::from_secs(1), call).await.unwrap().unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal, "{e:?}");
    // While disconnected a call is refused at once.
    fake.stop();
    let refused = session.call(api::SITES_LIST, ()).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::DaemonNotRunning, "{refused:?}");
}

#[tokio::test]
async fn incompatible_waits_for_reconnect_now() {
    let fake = FakeYardd::start_new();
    fake.refuse_hello(YardError::new(ErrorCode::ProtocolMismatch, "yardd speaks protocol 3.0"));
    let (session, mut updates) = Session::start(config(&fake));
    loop {
        match next_state(&mut updates).await {
            ConnectionState::Incompatible(e) => {
                assert_eq!(e.code, ErrorCode::ProtocolMismatch);
                break;
            }
            ConnectionState::Connecting => continue,
            s => panic!("expected Incompatible, got {s:?}"),
        }
    }
    let hellos = || fake.requests().iter().filter(|(_, m, _)| m == "hello").count();
    let before = hellos();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(hellos(), before, "no retry until asked");
    fake.allow_hello();
    session.reconnect_now();
    connected(&mut updates).await;
}

#[tokio::test]
async fn a_slow_call_does_not_hold_up_a_quick_one() {
    let fake = FakeYardd::start_new();
    fake.answer("doctor", json!({"checks": []}));
    fake.delay("doctor", Duration::from_secs(2));
    fake.answer("sites.list", json!([]));
    let (session, mut updates) = Session::start(config(&fake));
    connected(&mut updates).await;
    let doctor = tokio::spawn({
        let session = session.clone();
        async move { session.call(api::DOCTOR, DoctorParams::default()).await }
    });
    while !fake.requests().iter().any(|(_, m, _)| m == "doctor") {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let started = Instant::now();
    session.call(api::SITES_LIST, ()).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
    assert!(doctor.await.unwrap().is_ok());
}

#[tokio::test]
async fn logs_use_their_own_connection() {
    let fake = FakeYardd::start_new();
    let (session, mut updates) = Session::start(config(&fake));
    connected(&mut updates).await;
    let mut logs = session.logs().await.unwrap();
    let subscribes: Vec<_> = fake.requests().into_iter().filter(|(_, m, _)| m == "subscribe").collect();
    let (conn, _, params) = subscribes.last().unwrap().clone();
    assert_eq!(params, json!({"topics": ["logs"]}));
    assert_ne!(conn, subscribes[0].0, "a connection of its own");
    fake.push(json!({"type": "log_line", "unit": "nginx", "line": "GET /"}));
    let e = tokio::time::timeout(Duration::from_secs(1), logs.recv()).await.unwrap().unwrap();
    assert!(matches!(&e, Event::LogLine { line, .. } if line == "GET /"), "{e:?}");
    let leaked = tokio::time::timeout(Duration::from_millis(100), updates.recv()).await;
    assert!(!matches!(leaked, Ok(Some(Update::Event(Event::LogLine { .. })))), "{leaked:?}");
}
