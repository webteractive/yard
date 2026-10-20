//! The root helper's verbs, run in-process against a temp resolver dir.

mod support;

use serde_json::{Value, json};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use support::{Fixture, free_port, spawn_helper};
use yard_core::render::resolver::resolver_file;
use yard_daemon::helper_client::HelperClient;
use yard_proto::ErrorCode;

#[tokio::test]
async fn resolver_install_and_remove() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let dir = resolver.path().join("resolver");
    let helper = spawn_helper(&fx, &dir, &[]).await;
    let file = dir.join("yard");

    helper.install_resolver("yard", 53535).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), resolver_file(53535));
    helper.install_resolver("yard", 53536).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), resolver_file(53536), "Yard's own file is rewritten");

    helper.remove_resolver("yard").await.unwrap();
    assert!(!file.exists());
    helper.remove_resolver("yard").await.unwrap();
}

#[tokio::test]
async fn resolver_refuses_foreign_file() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;
    let herd = resolver.path().join("test");
    std::fs::write(&herd, "nameserver 127.0.0.1\n").unwrap();

    let err = helper.install_resolver("test", 53535).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ResolverForeign);
    helper.remove_resolver("test").await.unwrap();
    assert_eq!(std::fs::read_to_string(&herd).unwrap(), "nameserver 127.0.0.1\n");
}

#[tokio::test]
async fn rejects_bad_tld_and_low_port() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;

    for (tld, port) in [("../etc", 53535), ("Yard", 53535), ("", 53535), ("yard", 53)] {
        let err = helper.install_resolver(tld, port).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{tld:?} {port}");
    }
    assert_eq!(helper.remove_resolver("../etc").await.unwrap_err().code, ErrorCode::InvalidArgument);
    assert_eq!(std::fs::read_dir(resolver.path()).unwrap().count(), 0, "nothing was written");
}

#[tokio::test]
async fn unknown_method_is_method_not_found() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;

    let err = helper.call::<Value>("exec", json!({"cmd": "/bin/sh"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::MethodNotFound);
}

#[tokio::test]
async fn nginx_verbs_are_gone() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;

    // yardd runs nginx as the user now; root never starts it.
    for verb in ["nginx.start", "nginx.stop", "nginx.reload", "nginx.test_config"] {
        let err = helper.call::<Value>(verb, Value::Null).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::MethodNotFound, "{verb}");
    }
}

#[tokio::test]
async fn client_without_helper_reports_not_installed() {
    let client = HelperClient::new("/tmp/yard-test-missing/helper.sock");
    let err = client.status().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::HelperNotInstalled);
    assert_eq!(err.hint.as_deref(), Some("sudo yard helper install"));
}

/// A helper that may bind one free port, standing in for 80.
async fn helper_for_one_port() -> (Fixture, tempfile::TempDir, HelperClient, u16) {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let port = free_port();
    let helper = spawn_helper(&fx, resolver.path(), &[port]).await;
    (fx, resolver, helper, port)
}

/// Accepts one connection on `fd` after `connect` has dialled it.
fn accepts(fd: std::os::fd::OwnedFd, connect: impl FnOnce() -> std::io::Result<TcpStream>) -> bool {
    let listener = TcpListener::from(fd);
    let _client = connect().unwrap();
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if listener.accept().is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[tokio::test]
async fn ports_bind_hands_over_loopback_listeners() {
    let (_fx, _resolver, helper, port) = helper_for_one_port().await;

    let mut fds = helper.bind_port(port).await.unwrap().into_iter();
    let (v4, v6) = (fds.next().unwrap(), fds.next().unwrap());
    assert!(fds.next().is_none(), "one socket per loopback address");
    assert!(accepts(v4, || TcpStream::connect(("127.0.0.1", port))), "127.0.0.1:{port}");
    assert!(accepts(v6, || TcpStream::connect(("::1", port))), "[::1]:{port}");
}

#[tokio::test]
async fn ports_bind_refuses_unlisted_ports() {
    let (_fx, _resolver, helper, _port) = helper_for_one_port().await;
    let other = free_port();

    let err = helper.bind_port(other).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    assert!(TcpListener::bind(("127.0.0.1", other)).is_ok(), "the helper bound nothing");
}

#[tokio::test]
async fn ports_bind_reports_a_held_port() {
    let (_fx, _resolver, helper, port) = helper_for_one_port().await;
    let _held = TcpListener::bind(("127.0.0.1", port)).unwrap();

    let err = helper.bind_port(port).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PortInUse, "{err:?}");
    assert_eq!(err.data.as_ref().and_then(|d| d["port"].as_u64()), Some(u64::from(port)));
}
