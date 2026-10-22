//! Sites served from their project's `.yard` (project-file spec §5).

mod support;

use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use support::daemon::{Client, prepared, run_job};
use support::wait_until;

async fn site(c: &mut Client, name: &str) -> Value {
    let list = c.call("sites.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap_or_else(|| panic!("{list}"))
}

async fn unit_running(c: &mut Client, name: &str) -> bool {
    let status = c.call("status", Value::Null).await.unwrap();
    status["units"].as_array().unwrap().iter().any(|u| u["name"] == name && u["state"] == "running")
}

async fn has_unit(c: &mut Client, name: &str) -> bool {
    let status = c.call("status", Value::Null).await.unwrap();
    status["units"].as_array().unwrap().iter().any(|u| u["name"] == name)
}

fn write_yard(dir: &Path, text: &str) {
    std::fs::write(dir.join(".yard"), text).unwrap();
}

/// Reconciles through a job that installs nothing: PHP 8.4 is already there.
async fn reconcile(c: &mut Client) {
    let (lines, error) = run_job(c, "php.install", json!({"php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
}

async fn wait_running(c: &mut Client, unit: &str) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if unit_running(c, unit).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn project_file_php_is_served() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    let dir = d.project("ee");
    write_yard(&dir, "php = \"7.3\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(s["php"], "7.3", "{s}");
    assert_eq!(s["project_file"], dir.join(".yard").display().to_string());
    assert!(wait_running(&mut c, "php-fpm-7.3").await);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unmet_site_serves_setup_page_and_others_keep_serving() {
    let d = prepared("").await;
    let ee = d.project("ee");
    write_yard(&ee, "php = \"7.3\"\nsecure = true\n");
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(s["needs"], json!([{"kind": "php", "php": "7.3"}]), "{s}");
    let page = std::fs::read_to_string(d.fx.paths.nginx_setup_page("ee")).unwrap();
    assert!(page.contains("ee needs PHP 7.3."), "{page}");
    let conf = std::fs::read_to_string(d.fx.paths.nginx_site("ee")).unwrap();
    assert!(conf.contains("return 503;"), "{conf}");
    assert!(d.fx.paths.cert("ee").exists(), "a secured setup page still answers https");
    assert!(!has_unit(&mut c, "php-fpm-7.3").await, "no pool for a version that is not installed");
    assert_eq!(site(&mut c, "blog").await["needs"], Value::Null);
    assert!(wait_running(&mut c, "php-fpm-8.4").await);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn compiled_in_extension_is_not_a_need() {
    let d = prepared("").await;
    let dir = d.project("ee");
    write_yard(&dir, "extensions = [\"mbstring\", \"redis\"]\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(s["needs"], json!([{"kind": "extension", "name": "redis", "php": "8.4"}]), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn module_cache_is_read_not_rerun() {
    let d = prepared("").await;
    let dir = d.project("ee");
    let d = d.start().await;
    let mut c = d.client().await;
    let cache = d.fx.paths.php_modules_cache();
    write_yard(&dir, "extensions = [\"mbstring\"]\n");
    reconcile(&mut c).await;
    let text = std::fs::read_to_string(&cache).unwrap();
    let mut map: serde_json::Map<String, Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(map.len(), 1, "{text}");
    for list in map.values_mut() {
        list.as_array_mut().unwrap().push(json!("sentinel"));
    }
    std::fs::write(&cache, serde_json::to_string(&map).unwrap()).unwrap();
    write_yard(&dir, "extensions = [\"sentinel\"]\n");
    reconcile(&mut c).await;
    assert_eq!(site(&mut c, "ee").await["needs"], Value::Null);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn managed_extension_counts_even_when_disabled() {
    let d = prepared("").await;
    std::fs::write(
        d.fx.paths.yard_toml(),
        "[php.\"8.4\".extensions]\nredis = { source = \"brew\", enabled = false }\n",
    )
    .unwrap();
    let dir = d.project("ee");
    write_yard(&dir, "extensions = [\"redis\"]\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(s["project_file"], dir.join(".yard").display().to_string(), "{s}");
    assert_eq!(s["needs"], Value::Null, "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn fifo_project_file_does_not_block_reconcile() {
    let d = prepared("").await;
    let ee = d.project("ee");
    let fifo = ee.join(".yard");
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&fifo).status().unwrap().success());
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;
    let list = c.call_within("sites.list", Value::Null, Duration::from_secs(5)).await.unwrap();
    let ee = list.as_array().unwrap().iter().find(|s| s["name"] == "ee").unwrap();
    assert_eq!(ee["project_error"]["code"], "PROJECT_FILE_INVALID", "{ee}");
    assert!(list.as_array().unwrap().iter().any(|s| s["name"] == "blog" && s["needs"].is_null()));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn setup_page_goes_once_met() {
    let d = prepared("").await;
    let dir = d.project("ee");
    write_yard(&dir, "php = \"7.3\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let page = d.fx.paths.nginx_setup_page("ee");
    assert!(page.exists());
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.3"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(wait_until(|| !page.exists(), Duration::from_secs(5)).await, "the setup page is removed");
    assert_eq!(site(&mut c, "ee").await["needs"], Value::Null);
    assert!(wait_running(&mut c, "php-fpm-7.3").await);
    d.shutdown().await.unwrap();
}
