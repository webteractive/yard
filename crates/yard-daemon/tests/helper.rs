//! The root helper's verbs, run in-process against a temp resolver dir.

mod support;

use serde_json::{Value, json};
use support::{Fixture, spawn_helper};
use yard_core::render::resolver::resolver_file;
use yard_daemon::helper_client::HelperClient;
use yard_proto::ErrorCode;

#[tokio::test]
async fn resolver_install_and_remove() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let dir = resolver.path().join("resolver");
    let helper = spawn_helper(&fx, &dir).await;
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
    let helper = spawn_helper(&fx, resolver.path()).await;
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
    let helper = spawn_helper(&fx, resolver.path()).await;

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
    let helper = spawn_helper(&fx, resolver.path()).await;

    let err = helper.call::<Value>("exec", json!({"cmd": "/bin/sh"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::MethodNotFound);
}

#[tokio::test]
async fn nginx_verbs_are_gone() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path()).await;

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
