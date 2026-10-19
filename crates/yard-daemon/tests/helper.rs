//! The root helper's verbs, run in-process against a fake nginx and a temp resolver dir.

mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::{FAKE, Fixture, free_port, spawn_helper, wait_until};
use yard_core::render::resolver::resolver_file;
use yard_daemon::helper_client::HelperClient;
use yard_daemon::probe::is_listening;
use yard_proto::ErrorCode;

const FIVE_S: Duration = Duration::from_secs(5);

#[tokio::test]
async fn nginx_start_reload_stop_cycle() {
    let fx = Fixture::new();
    let p = free_port();
    fx.write_nginx_conf(p);
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path()).await;

    helper.start_nginx().await.unwrap();
    assert!(wait_until(|| is_listening(p), FIVE_S).await, "nginx listens after start");
    assert!(helper.status().await.unwrap().nginx_pid.is_some());

    helper.reload_nginx().await.unwrap();
    assert!(is_listening(p), "SIGHUP keeps nginx serving");

    helper.stop_nginx().await.unwrap();
    assert!(wait_until(|| !is_listening(p), FIVE_S).await, "nginx stops listening");
    assert_eq!(helper.status().await.unwrap().nginx_pid, None);
}

#[tokio::test]
async fn start_is_idempotent() {
    let fx = Fixture::new();
    let p = free_port();
    fx.write_nginx_conf(p);
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path()).await;

    helper.start_nginx().await.unwrap();
    let first = helper.status().await.unwrap().nginx_pid;
    helper.start_nginx().await.unwrap();
    assert!(first.is_some());
    assert_eq!(helper.status().await.unwrap().nginx_pid, first, "a second start is a no-op");
    helper.stop_nginx().await.unwrap();
}

#[tokio::test]
async fn test_config_reports_failure_output() {
    let fx = Fixture::new();
    fx.write_nginx_conf(free_port());
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path()).await;

    assert!(helper.test_nginx().await.unwrap().ok);

    let broken = fx.paths.nginx_override("blog");
    std::fs::write(&broken, "yard-test-broken;\n").unwrap();
    let result = helper.test_nginx().await.unwrap();
    assert!(!result.ok);
    assert!(result.output.contains(&broken.display().to_string()), "{}", result.output);
}

#[tokio::test]
async fn refuses_to_signal_non_nginx_pid() {
    let fx = Fixture::new();
    fx.write_nginx_conf(free_port());
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path()).await;
    let mut victim = std::process::Command::new(FAKE).arg("hang").spawn().unwrap();
    std::fs::write(fx.paths.nginx_pid(), format!("{}\n", victim.id())).unwrap();

    assert_eq!(helper.reload_nginx().await.unwrap_err().code, ErrorCode::HelperRefused);
    assert_eq!(helper.stop_nginx().await.unwrap_err().code, ErrorCode::HelperRefused);
    assert_eq!(helper.status().await.unwrap().nginx_pid, None);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(victim.try_wait().unwrap().is_none(), "the process was not signalled");
    victim.kill().unwrap();
    victim.wait().unwrap();
}

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
async fn client_without_helper_reports_not_installed() {
    let client = HelperClient::new("/tmp/yard-test-missing/helper.sock");
    let err = client.status().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::HelperNotInstalled);
    assert_eq!(err.hint.as_deref(), Some("sudo yard helper install"));
}
