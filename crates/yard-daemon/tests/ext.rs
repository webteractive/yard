//! `ext.add` over the socket: brew, then pecl, then a `.so`, verified with `php -m`.

mod support;

use serde_json::json;
use std::path::PathBuf;
use support::daemon::{Daemon, prepared, run_job};
use yard_proto::ErrorCode;

fn yard_toml(d: &Daemon) -> Option<Vec<u8>> {
    std::fs::read(d.fx.paths.yard_toml()).ok()
}

fn conf_d(d: &Daemon) -> PathBuf {
    d.fx.paths.php_conf_d("8.4".parse().unwrap())
}

/// A `.so` outside the data root, named `file`.
fn so(dir: &tempfile::TempDir, file: &str) -> String {
    let path = dir.path().join(file);
    std::fs::write(&path, "not really a library").unwrap();
    path.display().to_string()
}

#[tokio::test]
async fn ext_add_from_brew_formula_loads_and_is_recorded() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "redis\n").unwrap();
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "ext.add", json!({"name": "redis", "php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
    let toml = String::from_utf8(yard_toml(&d).unwrap()).unwrap();
    assert!(toml.contains("redis = { source = \"brew\" }"), "{toml}");
    assert!(conf_d(&d).join("50-redis.ini").is_file());
    assert!(!conf_d(&d).join("10-brew-20-redis.ini").exists(), "brew's own ini must not load it");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_falls_back_to_pecl() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "ext.add", json!({"name": "mongodb"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("fake pecl install mongodb")), "{lines:?}");
    let toml = String::from_utf8(yard_toml(&d).unwrap()).unwrap();
    assert!(toml.contains("mongodb = { source = \"pecl\" }"), "{toml}");
    assert!(conf_d(&d).join("50-mongodb.ini").is_file());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_from_so_path_copies_it() {
    let d = prepared("").await.start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = d.client().await;
    let (lines, error) =
        run_job(&mut c, "ext.add", json!({"name": so(&dir, "custom.so"), "php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
    let copy = d.fx.paths.php_dir("8.4".parse().unwrap()).join("ext/custom.so");
    assert!(copy.is_file());
    let ini = std::fs::read_to_string(conf_d(&d).join("50-custom.ini")).unwrap();
    assert!(ini.contains(&copy.display().to_string()), "{ini}");
    let toml = String::from_utf8(yard_toml(&d).unwrap()).unwrap();
    assert!(toml.contains("source = \"path\""), "{toml}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_that_does_not_load_is_rolled_back() {
    let d = prepared("").await.start().await;
    let before = yard_toml(&d);
    let dir = tempfile::tempdir().unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": so(&dir, "broken.so"), "php": "8.4"})).await;
    let e = error.expect("a failure");
    assert_eq!(e.code, ErrorCode::ExtVerifyFailed);
    assert_eq!(e.message, "broken did not load in PHP 8.4");
    let output = e.data.as_ref().and_then(|d| d["output"].as_str()).expect("data.output");
    assert!(output.contains("Unable to load dynamic library"), "{output}");
    assert_eq!(yard_toml(&d), before);
    assert!(!conf_d(&d).join("50-broken.ini").exists());
    assert!(!d.fx.paths.php_dir("8.4".parse().unwrap()).join("ext/broken.so").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_with_no_source_leaves_yard_toml_unchanged() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.brew.path().join("fake-brew-fail"), "nosuch").unwrap();
    let before = yard_toml(&d);
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": "nosuch", "php": "8.4"})).await;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::JobFailed));
    assert_eq!(yard_toml(&d), before);
    assert!(!conf_d(&d).join("50-nosuch.ini").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_refuses_a_managed_name_and_a_bad_one() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.paths.yard_toml(), "[php.\"8.4\".extensions]\nredis = { source = \"pecl\" }\n")
        .unwrap();
    let mut c = d.client().await;
    let e = c.call("ext.add", json!({"name": "redis", "php": "8.4"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    assert_eq!(e.hint.as_deref(), Some("yard ext enable redis --php 8.4"));
    let e = c.call("ext.add", json!({"name": "../x"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    let e = c.call("ext.add", json!({"name": "redis", "php": "7.1"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PhpNotInstalled);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn brew_ext_that_does_not_load_stays_installed_but_disabled() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "broken\n").unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": "broken", "php": "8.4"})).await;
    let e = error.expect("a failure");
    assert_eq!(e.code, ErrorCode::ExtVerifyFailed);
    assert_eq!(e.hint.as_deref(), Some("yard ext remove broken --php 8.4"));
    let toml = String::from_utf8(yard_toml(&d).unwrap()).unwrap();
    assert!(toml.contains("broken = { source = \"brew\", enabled = false }"), "{toml}");
    assert!(!conf_d(&d).join("50-broken.ini").exists());
    assert!(!conf_d(&d).join("10-brew-20-broken.ini").exists(), "brew's ini must not load it either");
    d.shutdown().await.unwrap();
}
