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
    let ini = std::fs::read_to_string(conf_d(&d).join("50-redis.ini")).unwrap();
    let keg_so = d.fx.brew.path().join("opt/redis@8.4/redis.so");
    assert!(
        ini.contains(&format!("extension = \"{}\"", keg_so.display())),
        "loads brew's .so by path: {ini}"
    );
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
    assert_eq!(e.hint.as_deref(), Some("yard ext add --help"));
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

fn toml_text(d: &Daemon) -> String {
    String::from_utf8(yard_toml(d).unwrap_or_default()).unwrap()
}

#[tokio::test]
async fn disable_removes_the_ini_and_enable_restores_it() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "ext.add", json!({"name": "mongodb"})).await;
    assert_eq!(error, None, "{lines:?}");
    c.call("ext.disable", json!({"name": "mongodb"})).await.unwrap();
    assert!(!conf_d(&d).join("50-mongodb.ini").exists());
    assert!(toml_text(&d).contains("mongodb = { source = \"pecl\", enabled = false }"), "{}", toml_text(&d));
    let list = c.call("ext.list", json!({})).await.unwrap();
    assert_eq!(list, json!([{"name": "mongodb", "source": "pecl", "enabled": false}]));
    c.call("ext.enable", json!({"name": "mongodb", "php": "8.4"})).await.unwrap();
    assert!(conf_d(&d).join("50-mongodb.ini").is_file());
    assert!(toml_text(&d).contains("mongodb = { source = \"pecl\", enabled = true }"), "{}", toml_text(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabling_a_bundled_module_drops_its_mirror() {
    let d = prepared("").await;
    let brew_ini = d.fx.brew.path().join("etc/php/8.4/conf.d/ext-opcache.ini");
    std::fs::write(&brew_ini, "[opcache]\nzend_extension=\"opcache.so\"\n").unwrap();
    let d = d.start().await;
    let mirror = conf_d(&d).join("10-brew-ext-opcache.ini");
    assert!(mirror.is_file());
    let mut c = d.client().await;
    let list = c.call("ext.list", json!({"php": "8.4"})).await.unwrap();
    assert_eq!(list, json!([{"name": "opcache", "source": "bundled", "enabled": true}]));

    c.call("ext.disable", json!({"name": "opcache"})).await.unwrap();
    assert!(!mirror.exists());
    assert!(
        toml_text(&d).contains("opcache = { source = \"bundled\", enabled = false }"),
        "{}",
        toml_text(&d)
    );
    c.call("ext.enable", json!({"name": "opcache"})).await.unwrap();
    assert!(mirror.is_file());
    assert!(!toml_text(&d).contains("opcache"), "{}", toml_text(&d));
    assert!(brew_ini.is_file(), "brew's file is never touched");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn remove_uninstalls_through_brew_and_forgets() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "redis\n").unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": "redis"})).await;
    assert_eq!(error, None);
    let (lines, error) = run_job(&mut c, "ext.remove", json!({"name": "redis", "php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("fake brew uninstall shivammathur/extensions/redis@8.4")),
        "{lines:?}"
    );
    assert!(!toml_text(&d).contains("redis"), "{}", toml_text(&d));
    assert!(!conf_d(&d).join("50-redis.ini").exists());
    assert!(!conf_d(&d).join("10-brew-20-redis.ini").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn remove_deletes_a_copied_so() {
    let d = prepared("").await.start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": so(&dir, "custom.so")})).await;
    assert_eq!(error, None);
    let (_, error) = run_job(&mut c, "ext.remove", json!({"name": "custom"})).await;
    assert_eq!(error, None);
    assert!(!d.fx.paths.php_dir("8.4".parse().unwrap()).join("ext/custom.so").exists());
    assert!(!toml_text(&d).contains("custom"), "{}", toml_text(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn remove_refuses_bundled() {
    let d = prepared("").await.start().await;
    std::fs::write(
        d.fx.paths.yard_toml(),
        "[php.\"8.4\".extensions]\nopcache = { source = \"bundled\", enabled = false }\n",
    )
    .unwrap();
    let mut c = d.client().await;
    let e = c.call("ext.remove", json!({"name": "opcache"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    assert_eq!(e.hint.as_deref(), Some("yard ext disable opcache --php 8.4"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn xdebug_on_without_xdebug_is_ext_not_found() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let e = c.call("xdebug.set", json!({"on": true})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ExtNotFound);
    assert_eq!(e.hint.as_deref(), Some("yard ext add xdebug --php 8.4"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn xdebug_off_and_on_toggle_it() {
    let d = prepared("").await.start().await;
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "xdebug\n").unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": "xdebug"})).await;
    assert_eq!(error, None);
    c.call("xdebug.set", json!({"on": false})).await.unwrap();
    assert!(!conf_d(&d).join("50-xdebug.ini").exists());
    c.call("xdebug.set", json!({"on": true, "php": "8.4"})).await.unwrap();
    let ini = std::fs::read_to_string(conf_d(&d).join("50-xdebug.ini")).unwrap();
    assert!(ini.contains("zend_extension"), "{ini}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_extension_is_ext_not_found() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    for method in ["ext.enable", "ext.disable", "ext.remove"] {
        let e = c.call(method, json!({"name": "nosuch"})).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ExtNotFound, "{method}");
        assert_eq!(e.hint.as_deref(), Some("yard ext add nosuch --php 8.4"), "{method}");
    }
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_ini_brew_left_behind_is_neither_mirrored_nor_listed() {
    let d = prepared("").await;
    let gone = d.fx.brew.path().join("opt/phpredis@8.4/redis.so");
    let left = format!("[redis]\nextension=\"{}\"\n", gone.display());
    std::fs::write(d.fx.brew.path().join("etc/php/8.4/conf.d/20-redis.ini"), left).unwrap();
    let d = d.start().await;
    assert!(!conf_d(&d).join("10-brew-20-redis.ini").exists());
    let mut c = d.client().await;
    assert_eq!(c.call("ext.list", json!({})).await.unwrap(), json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_pinned_pecl_version_installs_that_release_under_the_module_name() {
    let d = prepared("").await.start().await;
    // A brew formula exists, but a pinned release is pecl's to give.
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "xdebug\n").unwrap();
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "ext.add", json!({"name": "xdebug-2.9.8"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("fake pecl install xdebug-2.9.8")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("fake brew")), "{lines:?}");
    let toml = toml_text(&d);
    assert!(toml.contains("xdebug = { source = \"pecl\" }"), "{toml}");
    assert!(conf_d(&d).join("50-xdebug.ini").is_file());
    d.shutdown().await.unwrap();
}
