//! `php.*` over the socket: installing, removing and choosing PHP versions.

mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::daemon::{Client, Daemon, prepared};
use yard_proto::{ErrorCode, YardError};

/// Calls a job method after subscribing to `jobs`, then waits for its `job_finished`.
/// Returns the progress lines and the job's error.
pub async fn run_job(c: &mut Client, method: &str, params: Value) -> (Vec<String>, Option<YardError>) {
    c.call("subscribe", json!({"topics": ["jobs"]})).await.unwrap();
    let started = c.call(method, params).await.unwrap();
    let id = started["job_id"].as_str().expect("a job_id").to_string();
    let mut lines = vec![];
    loop {
        let event = c.event(Duration::from_secs(10)).await.expect("the job finished");
        if event["job_id"] != id.as_str() {
            continue;
        }
        match event["type"].as_str() {
            Some("job_progress") => lines.push(event["line"].as_str().unwrap_or_default().to_string()),
            Some("job_finished") => {
                let error = serde_json::from_value(event["error"].clone()).unwrap();
                return (lines, error);
            }
            _ => {}
        }
    }
}

fn versions(list: &Value) -> Vec<(String, bool)> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|p| (p["version"].as_str().unwrap().to_string(), p["default"].as_bool().unwrap()))
        .collect()
}

fn brew_log(d: &Daemon) -> String {
    std::fs::read_to_string(d.fx.brew.path().join("fake-brew.log")).unwrap_or_default()
}

#[tokio::test]
async fn install_creates_the_keg_and_list_shows_it() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("fake brew install shivammathur/php/php@7.1")), "{lines:?}");
    let list = c.call("php.list", Value::Null).await.unwrap();
    assert_eq!(versions(&list), [("7.1".to_string(), false), ("8.4".to_string(), true)], "{list}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn install_of_an_installed_version_finishes_ok_without_brew() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "8.4"})).await;
    assert_eq!(error, None);
    assert!(lines.iter().any(|l| l.contains("PHP 8.4 is already installed")), "{lines:?}");
    assert_eq!(brew_log(&d), "");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn install_of_an_unsupported_version_is_refused() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let err = c.call("php.install", json!({"php": "5.6"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_brew_install_is_job_failed() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.brew.path().join("fake-brew-fail"), "shivammathur/php/php@7.1").unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::JobFailed));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn uninstall_removes_the_keg() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    let (_, error) = run_job(&mut c, "php.uninstall", json!({"php": "7.1"})).await;
    assert_eq!(error, None);
    assert!(!d.fx.brew.path().join("opt/php@7.1").exists());
    let list = c.call("php.list", Value::Null).await.unwrap();
    assert_eq!(versions(&list), [("8.4".to_string(), true)]);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn uninstall_refuses_the_default_version() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    let err = c.call("php.uninstall", json!({"php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PhpInUse);
    assert!(err.message.contains("default"), "{}", err.message);
    assert_eq!(err.hint.as_deref(), Some("yard php default 7.1"));
    assert!(!brew_log(&d).contains("uninstall"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn default_requires_installed() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let err = c.call("php.default", json!({"php": "7.3"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PhpNotInstalled);
    assert_eq!(err.hint.as_deref(), Some("yard php install 7.3"));

    run_job(&mut c, "php.install", json!({"php": "7.3"})).await;
    c.call("php.default", json!({"php": "7.3"})).await.unwrap();
    let toml = std::fs::read_to_string(d.fx.paths.yard_toml()).unwrap();
    assert!(toml.contains("default_php = \"7.3\""), "{toml}");
    let list = c.call("php.list", Value::Null).await.unwrap();
    assert_eq!(versions(&list), [("7.3".to_string(), true), ("8.4".to_string(), false)]);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn list_names_the_sites_and_the_pool() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    let list = c.call("php.list", Value::Null).await.unwrap();
    let php = &list[0];
    assert_eq!(php["version"], "8.4");
    assert_eq!(php["sites"], json!(["blog"]));
    assert!(php["pool"].is_string(), "{list}");
    d.shutdown().await.unwrap();
}
