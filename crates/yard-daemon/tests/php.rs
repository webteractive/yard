//! `php.*` over the socket: installing, removing and choosing PHP versions.

mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::daemon::{Daemon, prepared, run_job};
use yard_proto::ErrorCode;

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

#[tokio::test]
async fn isolate_returns_the_site_on_its_version() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    d.project("old");
    let d = d.start().await;
    let mut c = d.client().await;
    let site = c.call("sites.isolate", json!({"site": "old", "php": "7.1"})).await.unwrap();
    assert_eq!(site["php"], "7.1", "{site}");
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(toml.contains("php = \"7.1\""), "{toml}");
    let site = c.call("sites.isolate", json!({"site": "old"})).await.unwrap();
    assert_eq!(site["php"], "8.4", "{site}");
    assert!(!std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap().contains("php ="));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn isolate_refuses_uninstalled_proxied_and_unknown() {
    let d = prepared("").await;
    d.project("old");
    let d = d.start().await;
    let mut c = d.client().await;
    let err = c.call("sites.isolate", json!({"site": "old", "php": "7.1"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PhpNotInstalled);
    assert_eq!(err.hint.as_deref(), Some("yard php install 7.1"));
    c.call("sites.proxy", json!({"name": "api", "url": "http://127.0.0.1:3000"})).await.unwrap();
    let err = c.call("sites.isolate", json!({"site": "api", "php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = c.call("sites.isolate", json!({"site": "nope", "php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::SiteNotFound);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn uninstall_refuses_an_isolated_version() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    c.call("sites.isolate", json!({"site": "blog", "php": "7.1"})).await.unwrap();
    let err = c.call("php.uninstall", json!({"php": "7.1"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PhpInUse);
    assert!(err.message.contains("blog"), "{}", err.message);
    assert_eq!(err.hint.as_deref(), Some("yard isolate <version> --site blog"));
    assert!(!brew_log(&d).contains("uninstall"));
    d.shutdown().await.unwrap();
}

fn fpm_pid(units: &Value, name: &str) -> Value {
    units["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == name)
        .map(|u| u["pid"].clone())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn config_set_writes_overrides_and_reloads_the_pool() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    let mut pid = Value::Null;
    for _ in 0..50 {
        pid = fpm_pid(&c.call("status", Value::Null).await.unwrap(), "php-fpm-8.4");
        if pid.is_number() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(pid.is_number(), "the 8.4 pool runs");

    c.call("php.config.set", json!({"key": "memory_limit", "value": "1G"})).await.unwrap();
    let overrides = std::fs::read_to_string(d.fx.paths.php_overrides("8.4".parse().unwrap())).unwrap();
    assert!(overrides.contains("memory_limit = 1G"), "{overrides}");
    assert!(overrides.starts_with("; USER-OWNED"), "{overrides}");
    assert_eq!(c.call("php.config.get", json!({"key": "memory_limit"})).await.unwrap(), "1G");

    let log = d.fx.paths.log("php-fpm-8.4");
    let reloaded = || std::fs::read_to_string(&log).unwrap_or_default().contains("fake php-fpm reloaded");
    assert!(support::wait_until(reloaded, Duration::from_secs(5)).await, "the pool got SIGUSR2");
    assert_eq!(
        fpm_pid(&c.call("status", Value::Null).await.unwrap(), "php-fpm-8.4"),
        pid,
        "a reload, not a restart"
    );

    c.call("php.config.unset", json!({"key": "memory_limit"})).await.unwrap();
    assert_eq!(c.call("php.config.get", json!({"key": "memory_limit"})).await.unwrap(), Value::Null);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn config_refuses_bad_keys_values_and_versions() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let bad = [
        json!({"key": "a b", "value": "1"}),
        json!({"key": "x", "value": "a\nb"}),
        json!({"key": "x", "value": "1", "php": "5.6"}),
    ];
    for params in bad {
        let err = c.call("php.config.set", params.clone()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{params}");
    }
    let err = c.call("php.config.set", json!({"key": "x", "value": "1", "php": "7.3"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PhpNotInstalled);
    let err = c.call("php.config.unset", json!({"key": "never_set"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    d.shutdown().await.unwrap();
}
