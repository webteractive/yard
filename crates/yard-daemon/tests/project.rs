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

/// A managed `.yard` for `name`, written for `dir` as `sites.toml` spells it.
fn write_managed(d: &support::daemon::Daemon, name: &str, dir: &Path, body: &str) -> std::path::PathBuf {
    let file = d.fx.paths.managed_project_file(name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, format!("{body}project = {:?}\n", dir.display().to_string())).unwrap();
    file
}

#[tokio::test]
async fn managed_file_is_served_and_committed_wins() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    let managed = write_managed(&d, "ee", &dir, "php = \"7.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(
        (s["php"].as_str(), s["project_file"].as_str(), s["project_file_kind"].as_str()),
        (Some("7.4"), Some(managed.to_str().unwrap()), Some("managed")),
        "{s}"
    );
    write_yard(&dir, "php = \"7.3\"\n");
    reconcile(&mut c).await;
    let s = site(&mut c, "ee").await;
    assert_eq!((s["php"].as_str(), s["project_file_kind"].as_str()), (Some("7.3"), Some("project")), "{s}");
    assert_eq!(s["ignored_managed_file"]["because"], "committed", "{s}");
    assert_eq!(s["ignored_managed_file"]["file"], managed.display().to_string(), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_dangling_committed_symlink_does_not_fall_through() {
    let d = prepared("").await;
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    write_managed(&d, "ee", &dir, "php = \"7.4\"\n");
    std::os::unix::fs::symlink(dir.join("nowhere"), dir.join(".yard")).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(s["project_file_kind"], "project", "{s}");
    assert_eq!(s["project_error"]["code"], "PROJECT_FILE_INVALID", "{s}");
    assert!(s["project_error"]["message"].as_str().unwrap().contains(".yard"), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_managed_file_for_another_project_is_not_used() {
    let d = prepared("").await;
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    let other = d.projects.path().join("elsewhere");
    let managed = write_managed(&d, "ee", &other, "php = \"7.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!(s["php"], "8.4", "{s}");
    assert!(s.get("project_file").is_none(), "{s}");
    assert_eq!(s["ignored_managed_file"]["because"], "other_project", "{s}");
    assert_eq!(s["ignored_managed_file"]["project"], other.display().to_string(), "{s}");
    assert_eq!(s["ignored_managed_file"]["file"], managed.display().to_string(), "{s}");
    let _ = dir;
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

async fn php_within(c: &mut Client, name: &str, want: &str, within: Duration) -> Value {
    let deadline = std::time::Instant::now() + within;
    loop {
        let s = site(c, name).await;
        if s["php"] == want || std::time::Instant::now() > deadline {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A linked `ee` served from its managed file, which says `body`.
async fn managed_site(body: &str) -> (support::daemon::Daemon, std::path::PathBuf, std::path::PathBuf) {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    std::fs::create_dir_all(dir.join("web")).unwrap();
    let managed = write_managed(&d, "ee", &dir, body);
    (d.start().await, dir, managed)
}

#[tokio::test]
async fn isolate_edits_the_managed_file_and_keeps_comments() {
    let (d, _dir, managed) = managed_site("# keep me\nphp = \"7.3\"\n").await;
    let mut c = d.client().await;
    let s = c.call("sites.isolate", json!({"site": "ee", "php": "7.4"})).await.unwrap();
    assert_eq!(s["php"], "7.4", "{s}");
    let text = std::fs::read_to_string(&managed).unwrap();
    assert!(text.starts_with("# keep me\nphp = \"7.4\"\n"), "{text}");
    assert!(text.contains("project = "), "{text}");
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(!toml.contains("php ="), "{toml}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unisolate_and_a_docroot_reset_keep_the_header_and_comments() {
    let header = "# .yard for ee, kept by Yard outside the project.\n";
    let body = format!("{header}# legacy mcrypt\nphp = \"7.3\"\n# served from web\ndocroot = \"web\"\n");
    let (d, _dir, managed) = managed_site(&body).await;
    let mut c = d.client().await;
    let read = || std::fs::read_to_string(&managed).unwrap();
    c.call("sites.isolate", json!({"site": "ee"})).await.unwrap();
    assert!(
        read().starts_with(&format!("{header}# legacy mcrypt\n# served from web\ndocroot = ")),
        "{}",
        read()
    );
    c.call("sites.docroot", json!({"site": "ee"})).await.unwrap();
    assert!(
        read().starts_with(&format!("{header}# legacy mcrypt\n# served from web\nproject = ")),
        "{}",
        read()
    );
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unisolate_secure_unsecure_docroot_edit_the_managed_file() {
    let (d, _dir, managed) = managed_site("php = \"7.3\"\n").await;
    let mut c = d.client().await;
    let read = || std::fs::read_to_string(&managed).unwrap();
    c.call("sites.isolate", json!({"site": "ee"})).await.unwrap();
    assert!(!read().contains("php ="), "{}", read());
    let s = c.call("sites.secure", json!({"site": "ee"})).await.unwrap();
    assert!(read().contains("secure = true") && s["secure"] == true, "{}", read());
    let s = c.call("sites.unsecure", json!({"site": "ee"})).await.unwrap();
    assert!(read().contains("secure = false") && s["secure"] == false, "{}", read());
    let s = c.call("sites.docroot", json!({"site": "ee", "path": "web"})).await.unwrap();
    assert!(read().contains("docroot = \"web\""), "{}", read());
    assert!(s["docroot"].as_str().unwrap().ends_with("/ee/web"), "{s}");
    c.call("sites.docroot", json!({"site": "ee"})).await.unwrap();
    assert!(!read().contains("docroot ="), "{}", read());
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(!toml.contains("secure") && !toml.contains("docroot"), "{toml}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn isolate_refuses_a_broken_managed_file() {
    let (d, _dir, managed) = managed_site("php = 7.3\n").await;
    let mut c = d.client().await;
    let before = std::fs::read(&managed).unwrap();
    let e = c.call("sites.isolate", json!({"site": "ee", "php": "7.4"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::ProjectFileInvalid, "{e:?}");
    assert!(e.message.contains(&managed.display().to_string()), "{e:?}");
    assert_eq!(std::fs::read(&managed).unwrap(), before);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn isolate_to_a_missing_version_leaves_the_managed_file() {
    let (d, dir, managed) = managed_site("php = \"7.3\"\n").await;
    let mut c = d.client().await;
    let before = std::fs::read(&managed).unwrap();
    let e = c.call("sites.isolate", json!({"site": "ee", "php": "7.2"})).await.unwrap_err();
    // The same refusal a site without a file gets.
    let plain = d.project("plain");
    let _ = (dir, plain);
    reconcile(&mut c).await;
    let e2 = c.call("sites.isolate", json!({"site": "plain", "php": "7.2"})).await.unwrap_err();
    assert_eq!(e.code, e2.code, "{e:?} {e2:?}");
    assert_eq!(std::fs::read(&managed).unwrap(), before);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn docroot_outside_the_project_is_refused_for_a_managed_file() {
    let (d, _dir, managed) = managed_site("php = \"7.3\"\n").await;
    let mut c = d.client().await;
    let before = std::fs::read(&managed).unwrap();
    for outside in ["../x", "/elsewhere/public"] {
        let e = c.call("sites.docroot", json!({"site": "ee", "path": outside})).await.unwrap_err();
        assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument, "{e:?}");
        assert!(e.message.contains("outside"), "{e:?}");
    }
    assert_eq!(std::fs::read(&managed).unwrap(), before);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_absolute_docroot_inside_the_project_is_written_relative() {
    let (d, dir, managed) = managed_site("php = \"7.3\"\n").await;
    let mut c = d.client().await;
    let web = dir.join("web");
    let s = c.call("sites.docroot", json!({"site": "ee", "path": web.display().to_string()})).await.unwrap();
    assert!(std::fs::read_to_string(&managed).unwrap().contains("docroot = \"web\""));
    assert_eq!(s["docroot"], web.display().to_string(), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_symlinked_managed_file_is_not_edited() {
    let d = prepared("").await;
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    let target = d.projects.path().join("dotfiles.yard");
    std::fs::write(&target, format!("php = \"8.4\"\nproject = {:?}\n", dir.display().to_string())).unwrap();
    let managed = d.fx.paths.managed_project_file("ee");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &managed).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let e = c.call("sites.isolate", json!({"site": "ee", "php": "7.4"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some(format!("edit {}", target.display()).as_str()), "{e:?}");
    assert!(managed.symlink_metadata().unwrap().file_type().is_symlink());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_managed_file_written_later_is_applied() {
    let d = prepared("").await;
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    let d = d.start().await;
    let mut c = d.client().await;
    assert_eq!(site(&mut c, "ee").await["php"], "8.4");
    // sites/ may be missing while yardd runs: one appearing later is watched too.
    let _ = std::fs::remove_dir_all(d.fx.paths.managed_sites_dir());
    tokio::time::sleep(Duration::from_millis(300)).await;
    write_managed(&d, "ee", &dir, "php = \"7.4\"\n");
    let s = php_within(&mut c, "ee", "7.4", Duration::from_secs(5)).await;
    assert_eq!(s["php"], "7.4", "{s}");
    // And an edit to it afterwards.
    write_managed(&d, "ee", &dir, "php = \"8.4\"\n");
    let s = php_within(&mut c, "ee", "8.4", Duration::from_secs(5)).await;
    assert_eq!(s["php"], "8.4", "{s}");
    // The site's directory removed and made again, as unlink, rm and init would.
    std::fs::remove_dir_all(d.fx.paths.managed_project_file("ee").parent().unwrap()).unwrap();
    // PHP stays 8.4 (the default), so wait for the file to be dropped instead.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut s = site(&mut c, "ee").await;
    while s.get("project_file").is_some() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        s = site(&mut c, "ee").await;
    }
    assert!(s.get("project_file").is_none(), "{s}");
    write_managed(&d, "ee", &dir, "php = \"7.4\"\n");
    let s = php_within(&mut c, "ee", "7.4", Duration::from_secs(5)).await;
    assert_eq!(s["php"], "7.4", "{s}");
    write_managed(&d, "ee", &dir, "php = \"8.4\"\n");
    let s = php_within(&mut c, "ee", "8.4", Duration::from_secs(5)).await;
    assert_eq!(s["php"], "8.4", "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn removing_the_committed_file_falls_back_to_the_managed_one() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    d.fx.add_php("7.4");
    let dir = d.project("ee");
    write_yard(&dir, "php = \"7.3\"\n");
    write_managed(&d, "ee", &dir, "php = \"7.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    assert_eq!(site(&mut c, "ee").await["php"], "7.3");
    std::fs::remove_file(dir.join(".yard")).unwrap();
    let s = php_within(&mut c, "ee", "7.4", Duration::from_secs(5)).await;
    assert_eq!((s["php"].as_str(), s["project_file_kind"].as_str()), (Some("7.4"), Some("managed")), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn editing_the_project_file_reconciles() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    let dir = d.project("ee");
    write_yard(&dir, "php = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    assert_eq!(site(&mut c, "ee").await["php"], "8.4");
    write_yard(&dir, "php = \"7.3\"\n");
    let s = php_within(&mut c, "ee", "7.3", Duration::from_secs(2)).await;
    assert_eq!(s["php"], "7.3", "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn deleting_the_project_file_reverts_the_site() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    let dir = d.project("ee");
    write_yard(&dir, "php = \"7.3\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    assert_eq!(site(&mut c, "ee").await["php"], "7.3");
    std::fs::remove_file(dir.join(".yard")).unwrap();
    let s = php_within(&mut c, "ee", "8.4", Duration::from_secs(2)).await;
    assert_eq!(s["php"], "8.4", "{s}");
    assert_eq!(s["project_file"], Value::Null, "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_newly_linked_project_is_watched() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    let d = d.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("fresh");
    std::fs::create_dir_all(&dir).unwrap();
    c.call("sites.link", json!({"name": "fresh", "path": dir.display().to_string()})).await.unwrap();
    // Written at once, before the watcher may have seen the link. The watcher adds the
    // directory when sites.toml changes and reconciles only after that, so whichever comes
    // first, this edit is read, and once it is, the directory is watched.
    write_yard(&dir, "php = \"7.3\"\n");
    let s = php_within(&mut c, "fresh", "7.3", Duration::from_secs(10)).await;
    assert_eq!(s["php"], "7.3", "{s}");
    // Only the watch on the project directory sees this one.
    write_yard(&dir, "php = \"8.4\"\n");
    let s = php_within(&mut c, "fresh", "8.4", Duration::from_secs(10)).await;
    assert_eq!(s["php"], "8.4", "{s}");
    d.shutdown().await.unwrap();
}

/// What `yard init` appends: one commented line per service engine (M4b spec §9).
const SERVICES_BLOCK: &str =
    "\n# [services]\n# mysql = \"8.4\"\n# postgres = \"17\"\n# redis = true\n# minio = true\n";

/// `sites.init` writing into the project, as every test written before managed files did.
async fn init(c: &mut Client, dir: &Path, extra: Value) -> Result<Value, yard_proto::YardError> {
    let mut params = json!({"path": dir.display().to_string(), "local": true});
    params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    c.call("sites.init", params).await
}

/// `sites.init` as the CLI sends it by default: the managed file.
async fn init_managed(c: &mut Client, dir: &Path, extra: Value) -> Result<Value, yard_proto::YardError> {
    let mut params = json!({"path": dir.display().to_string()});
    params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    c.call("sites.init", params).await
}

#[tokio::test]
async fn init_writes_the_managed_file_by_default() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("ee");
    std::fs::create_dir_all(dir.join("public")).unwrap();
    let r = init_managed(&mut c, &dir, json!({})).await.unwrap();
    let managed = d.fx.paths.managed_project_file("ee");
    assert_eq!(r["file"], managed.display().to_string(), "{r}");
    let text = r["text"].as_str().unwrap();
    assert!(text.starts_with("# .yard for ee, kept by Yard outside the project.\n"), "{text}");
    let canonical = dir.canonicalize().unwrap();
    assert!(text.contains(&format!("project = {:?}", canonical.display().to_string())), "{text}");
    assert_eq!(std::fs::read_to_string(&managed).unwrap(), text);
    assert!(!dir.join(".yard").exists());
    assert_eq!(r["site"]["project_file_kind"], "managed", "{r}");
    assert_eq!(r["written"], true);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_applies_an_existing_managed_file() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("ee");
    std::fs::create_dir_all(&dir).unwrap();
    let first = init_managed(&mut c, &dir, json!({})).await.unwrap();
    let again = init_managed(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!((again["written"].as_bool(), &again["text"]), (Some(false), &first["text"]), "{again}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_local_moves_the_managed_file() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("ee");
    std::fs::create_dir_all(&dir).unwrap();
    init_managed(&mut c, &dir, json!({})).await.unwrap();
    let managed = d.fx.paths.managed_project_file("ee");
    // A committed file already there wins (managed-file spec §5's table): it is applied, and
    // the managed file it hides is neither moved nor touched.
    write_yard(&dir, "php = \"8.4\"\n");
    let hidden = std::fs::read(&managed).unwrap();
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!((r["written"].as_bool(), &r["from_managed"]), (Some(false), &Value::Null), "{r}");
    assert_eq!(std::fs::read(&managed).unwrap(), hidden);
    std::fs::remove_file(dir.join(".yard")).unwrap();
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    let text = std::fs::read_to_string(dir.join(".yard")).unwrap();
    assert!(text.starts_with("# .yard: how Yard serves this project. Commit it.\n"), "{text}");
    assert!(!text.contains("project ="), "{text}");
    assert!(!managed.exists());
    assert_eq!(r["from_managed"], managed.display().to_string(), "{r}");
    assert_eq!(r["file"], dir.canonicalize().unwrap().join(".yard").display().to_string(), "{r}");
    assert_eq!(r["site"]["project_file_kind"], "project", "{r}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_force_local_moves_then_rewrites() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("ee");
    std::fs::create_dir_all(&dir).unwrap();
    init_managed(&mut c, &dir, json!({})).await.unwrap();
    let managed = d.fx.paths.managed_project_file("ee");
    let text =
        std::fs::read_to_string(&managed).unwrap().replace("docroot", "extensions = [\"redis\"]\ndocroot");
    std::fs::write(&managed, text).unwrap();
    init(&mut c, &dir, json!({"force": true})).await.unwrap();
    let text = std::fs::read_to_string(dir.join(".yard")).unwrap();
    assert!(text.contains("extensions = [\"redis\"]"), "{text}");
    assert!(!text.contains("project =") && !managed.exists(), "{text}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_with_a_committed_file_ignores_the_managed_one() {
    let d = prepared("").await;
    let dir = d.project("ee");
    let managed = write_managed(&d, "ee", &dir, "php = \"7.4\"\n");
    write_yard(&dir, "php = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let before = std::fs::read(&managed).unwrap();
    let r = init_managed(&mut c, &dir, json!({"force": true})).await.unwrap();
    assert_eq!(r["site"]["project_file_kind"], "project", "{r}");
    assert_eq!(std::fs::read(&managed).unwrap(), before);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_refuses_a_symlinked_managed_file() {
    let d = prepared("").await;
    let dir = d.project("ee");
    let target = d.projects.path().join("dotfiles.yard");
    std::fs::write(&target, format!("php = \"8.4\"\nproject = {:?}\n", dir.display().to_string())).unwrap();
    let managed = d.fx.paths.managed_project_file("ee");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &managed).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    // Read through the link it serves; written through, never.
    assert_eq!(site(&mut c, "ee").await["project_file_kind"], "managed");
    let before = std::fs::read(&target).unwrap();
    let e = init_managed(&mut c, &dir, json!({"force": true})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument, "{e:?}");
    assert_eq!(std::fs::read(&target).unwrap(), before);
    assert!(managed.symlink_metadata().unwrap().file_type().is_symlink());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_writes_project_as_sites_toml_spells_it() {
    let d = prepared("").await;
    let real = d.projects.path().join("real-ee");
    std::fs::create_dir_all(&real).unwrap();
    let link = d.projects.path().join("ee");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut file = yard_core::config::TomlFile::load(&d.fx.paths.sites_toml()).unwrap();
    file.set(&["sites", "ee", "path"], toml_edit::value(link.display().to_string()));
    file.save().unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let r = init_managed(&mut c, &real, json!({})).await.unwrap();
    let text = r["text"].as_str().unwrap();
    assert!(text.contains(&format!("project = {:?}", link.display().to_string())), "{text}");
    assert_eq!(r["site"]["project_file_kind"], "managed", "{r}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_replace_backs_up_another_projects_managed_file() {
    let d = prepared("").await;
    let a = d.project("app");
    let managed = write_managed(&d, "app", &a, "php = \"8.4\"\n");
    let old = std::fs::read(&managed).unwrap();
    let b = d.projects.path().join("b");
    std::fs::create_dir_all(&b).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let e = init_managed(&mut c, &b, json!({"name": "app"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::SiteNameTaken, "{e:?}");
    assert_eq!(std::fs::read(&managed).unwrap(), old);
    let r = init_managed(&mut c, &b, json!({"name": "app", "replace": true})).await.unwrap();
    let backup = managed.with_file_name(".yard-backup");
    assert_eq!(std::fs::read(&backup).unwrap(), old);
    assert_eq!(r["backup"], backup.display().to_string(), "{r}");
    let text = std::fs::read_to_string(&managed).unwrap();
    assert!(
        text.contains(&format!("project = {:?}", b.canonicalize().unwrap().display().to_string())),
        "{text}"
    );
    assert_eq!(r["site"]["project_file_kind"], "managed", "{r}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn link_adopts_a_matching_managed_file_and_names_another() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    d.fx.add_php("7.4");
    let a = d.projects.path().join("app");
    std::fs::create_dir_all(&a).unwrap();
    let a = a.canonicalize().unwrap();
    write_managed(&d, "app", &a, "php = \"7.4\"\n");
    // sites.toml carries a php the file now owns.
    let mut file = yard_core::config::TomlFile::load(&d.fx.paths.sites_toml()).unwrap();
    file.set(&["sites", "app", "php"], toml_edit::value("8.4"));
    file.save().unwrap();
    let s = c.call("sites.link", json!({"name": "app", "path": a.display().to_string()})).await.unwrap();
    assert_eq!((s["php"].as_str(), s["project_file_kind"].as_str()), (Some("7.4"), Some("managed")), "{s}");
    let toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(!toml.contains("php ="), "{toml}");
    c.call("sites.unlink", json!({"name": "app"})).await.unwrap();
    let b = d.projects.path().join("b");
    std::fs::create_dir_all(&b).unwrap();
    let s = c.call("sites.link", json!({"name": "app", "path": b.display().to_string()})).await.unwrap();
    assert_eq!(s["php"], "8.4", "{s}");
    assert_eq!(s["ignored_managed_file"]["because"], "other_project", "{s}");
    assert_eq!(s["ignored_managed_file"]["project"], a.display().to_string(), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_writes_links_and_moves_settings() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    let dir = d.project("ee");
    std::fs::create_dir_all(dir.join("web")).unwrap();
    let mut toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    toml.push_str("php = \"7.1\"\nsecure = true\ndocroot = \"web\"\n");
    std::fs::write(d.fx.paths.sites_toml(), toml).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let before = site(&mut c, "ee").await;
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!(r["written"], true);
    assert_eq!(
        r["text"],
        format!(
            "# .yard: how Yard serves this project. Commit it.\nphp = \"7.1\"\ndocroot = \"web\"\nsecure = true\n{SERVICES_BLOCK}"
        )
    );
    assert_eq!(std::fs::read_to_string(dir.join(".yard")).unwrap(), r["text"].as_str().unwrap());
    let sites = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(!sites.contains("php") && !sites.contains("secure") && !sites.contains("docroot"), "{sites}");
    let after = site(&mut c, "ee").await;
    for key in ["php", "docroot", "secure", "url"] {
        assert_eq!(after[key], before[key], "{key}: {after}");
    }
    assert_eq!(r["site"]["name"], "ee");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_links_an_unlinked_project() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("Fresh_App");
    std::fs::create_dir_all(dir.join("public")).unwrap();
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!(r["site"]["name"], "fresh-app");
    assert_eq!(
        r["text"],
        format!("# .yard: how Yard serves this project. Commit it.\ndocroot = \"public\"\n{SERVICES_BLOCK}")
    );
    let named = d.projects.path().join("other");
    std::fs::create_dir_all(&named).unwrap();
    assert_eq!(init(&mut c, &named, json!({"name": "mine"})).await.unwrap()["site"]["name"], "mine");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_twice_is_a_no_op() {
    let d = prepared("").await;
    let dir = d.project("ee");
    let d = d.start().await;
    let mut c = d.client().await;
    init(&mut c, &dir, json!({})).await.unwrap();
    let (file, sites) =
        (std::fs::read(dir.join(".yard")).unwrap(), std::fs::read(d.fx.paths.sites_toml()).unwrap());
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!(r["written"], false);
    assert_eq!(std::fs::read(dir.join(".yard")).unwrap(), file);
    assert_eq!(std::fs::read(d.fx.paths.sites_toml()).unwrap(), sites);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_keeps_an_existing_file_without_force() {
    let d = prepared("").await;
    let dir = d.project("ee");
    write_yard(&dir, "# hand written\nphp = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!(
        (r["written"].clone(), r["text"].clone()),
        (json!(false), json!("# hand written\nphp = \"8.4\"\n"))
    );
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_force_keeps_extensions() {
    let d = prepared("").await;
    let dir = d.project("ee");
    std::fs::create_dir_all(dir.join("public")).unwrap();
    write_yard(&dir, "docroot = \"web\"\nextensions = [\"redis\"]\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let r = init(&mut c, &dir, json!({"force": true})).await.unwrap();
    assert_eq!(r["written"], true);
    let text = std::fs::read_to_string(dir.join(".yard")).unwrap();
    assert!(text.contains("docroot = \"public\"") && text.contains("extensions = [\"redis\"]"), "{text}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_refuses_an_invalid_file() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("bad");
    std::fs::create_dir_all(&dir).unwrap();
    write_yard(&dir, "node = \"20\"\n");
    let e = init(&mut c, &dir, json!({})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::ProjectFileInvalid);
    let sites = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap_or_default();
    assert!(!sites.contains("bad"), "nothing was linked: {sites}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_force_refuses_a_symlinked_file() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let dir = d.projects.path().join("ln");
    std::fs::create_dir_all(&dir).unwrap();
    let target = d.projects.path().join("elsewhere.toml");
    std::fs::write(&target, "php = \"8.4\"\n").unwrap();
    std::os::unix::fs::symlink(&target, dir.join(".yard")).unwrap();
    let e = init(&mut c, &dir, json!({"force": true})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument, "{e:?}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "php = \"8.4\"\n",
        "the link's target is untouched"
    );
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_returns_needs() {
    let d = prepared("").await;
    let dir = d.project("ee");
    write_yard(&dir, "php = \"7.3\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert_eq!(r["site"]["needs"], json!([{"kind": "php", "php": "7.3"}]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_refuses_another_name_for_a_linked_project() {
    let d = prepared("").await;
    let dir = d.project("ee");
    let d = d.start().await;
    let mut c = d.client().await;
    let e = init(&mut c, &dir, json!({"name": "other"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument);
    assert_eq!(e.hint.as_deref(), Some("yard unlink ee"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_keeps_an_explicitly_unsecured_site_on_http() {
    let d = prepared("").await;
    std::fs::write(d.fx.paths.yard_toml(), "secure_by_default = true\n").unwrap();
    let dir = d.project("ee");
    let mut toml = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    toml.push_str("secure = false\n");
    std::fs::write(d.fx.paths.sites_toml(), toml).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    assert_eq!(site(&mut c, "ee").await["secure"], false);
    let r = init(&mut c, &dir, json!({})).await.unwrap();
    assert!(r["text"].as_str().unwrap().contains("secure = false\n"), "{}", r["text"]);
    assert_eq!(r["site"]["secure"], false, "serving does not change");
    d.shutdown().await.unwrap();
}

/// `ctl` linked at `<projects>/ctl/site` on PHP 7.1, as a project whose repository root
/// is one level up. Returns the repository root.
fn nested_site(d: &support::daemon::Daemon) -> std::path::PathBuf {
    let root = d.projects.path().join("ctl");
    std::fs::create_dir_all(root.join("site/public")).unwrap();
    let toml = format!("[sites.ctl]\npath = {:?}\nphp = \"7.1\"\n", root.join("site").display().to_string());
    std::fs::write(d.fx.paths.sites_toml(), toml).unwrap();
    root
}

#[tokio::test]
async fn init_moves_a_site_to_a_directory_without_a_file() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    let root = nested_site(&d);
    let d = d.start().await;
    let mut c = d.client().await;
    let r = init(&mut c, &root, json!({})).await.unwrap();
    let canonical = root.canonicalize().unwrap();
    assert_eq!(r["moved_from"], root.join("site").display().to_string(), "as sites.toml spelled it");
    assert_eq!(
        r["text"],
        format!(
            "# .yard: how Yard serves this project. Commit it.\nphp = \"7.1\"\ndocroot = \"site/public\"\n{SERVICES_BLOCK}"
        )
    );
    let sites = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(
        sites.contains(&format!("{:?}", canonical.display().to_string())) && !sites.contains("php"),
        "{sites}"
    );
    let s = site(&mut c, "ctl").await;
    assert_eq!(s["php"], "7.1", "{s}");
    assert_eq!(s["docroot"], canonical.join("site/public").display().to_string(), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_asks_before_moving_a_site_onto_an_existing_file() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    let root = nested_site(&d);
    write_yard(&root, "docroot = \"site/public\"\n");
    let before = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let e = init(&mut c, &root, json!({})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::SiteNameTaken, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some("yard init --replace"));
    assert_eq!(e.data.as_ref().unwrap()["name"], "ctl");
    assert_eq!(std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap(), before, "nothing moved");

    let r = init(&mut c, &root, json!({"replace": true})).await.unwrap();
    assert_eq!((r["written"].clone(), r["site"]["name"].clone()), (json!(false), json!("ctl")));
    assert!(r["moved_from"].as_str().unwrap().ends_with("/ctl/site"), "{r}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn init_asks_before_replacing_a_proxy() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    c.call("sites.proxy", json!({"name": "api", "url": "http://127.0.0.1:3000"})).await.unwrap();
    let dir = d.projects.path().join("api");
    std::fs::create_dir_all(&dir).unwrap();
    let e = init(&mut c, &dir, json!({})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::SiteNameTaken, "{e:?}");
    assert!(e.message.contains("http://127.0.0.1:3000"), "{}", e.message);
    assert!(!dir.join(".yard").exists());
    d.shutdown().await.unwrap();
}

// ---- sites.info ----

/// Makes `opt/php@<v>` what brew leaves: a link to `Cellar/php@<v>/<full>`, the directory
/// `Fixture::add_php` made.
fn cellar_php(d: &support::daemon::Daemon, v: &str, full: &str) {
    let prefix = d.fx.brew.path();
    let cellar = prefix.join(format!("Cellar/php@{v}/{full}"));
    std::fs::create_dir_all(cellar.parent().unwrap()).unwrap();
    let opt = prefix.join(format!("opt/php@{v}"));
    std::fs::rename(&opt, &cellar).unwrap();
    std::os::unix::fs::symlink(format!("../Cellar/php@{v}/{full}"), &opt).unwrap();
}

#[tokio::test]
async fn info_by_path_and_name() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    cellar_php(&d, "7.3", "7.3.33_8");
    let dir = d.project("ee");
    write_yard(&dir, "php = \"7.3\"\n");
    let inside = dir.join("app/Http");
    std::fs::create_dir_all(&inside).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;

    let by_name = c.call("sites.info", json!({"site": "ee"})).await.unwrap();
    let by_path = c.call("sites.info", json!({"path": inside.display().to_string()})).await.unwrap();
    assert_eq!(by_name, by_path);
    assert_eq!(by_name["site"]["name"], "ee");
    assert_eq!(by_name["site"]["php"], "7.3");
    assert_eq!(by_name["php_full"], "7.3.33");
    assert_eq!(
        by_name["sources"],
        json!({"php": "project_file", "docroot": "default", "secure": "default"}),
        "{by_name}"
    );
    assert_eq!(by_name["services"], json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn info_names_the_managed_file_as_the_source() {
    let (d, _dir, managed) = managed_site("php = \"7.4\"\n").await;
    let mut c = d.client().await;
    let detail = c.call("sites.info", json!({"site": "ee"})).await.unwrap();
    assert_eq!(
        detail["sources"],
        json!({"php": "managed_file", "docroot": "default", "secure": "default"}),
        "{detail}"
    );
    assert_eq!(detail["site"]["project_file"], managed.display().to_string(), "{detail}");
    assert_eq!(detail["site"]["project_file_kind"], "managed", "{detail}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn info_names_sites_toml_as_the_source_and_a_proxy_has_no_php() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    let dir = d.project("plain");
    std::fs::create_dir_all(dir.join("web")).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    c.call("sites.isolate", json!({"site": "plain", "php": "7.3"})).await.unwrap();
    c.call("sites.secure", json!({"site": "plain"})).await.unwrap();
    c.call("sites.proxy", json!({"name": "app", "url": "http://127.0.0.1:5173"})).await.unwrap();

    let detail = c.call("sites.info", json!({"site": "plain"})).await.unwrap();
    assert_eq!(
        detail["sources"],
        json!({"php": "sites_toml", "docroot": "default", "secure": "sites_toml"}),
        "{detail}"
    );
    // A keg that is a plain directory names no Cellar version.
    assert_eq!(detail["php_full"], Value::Null, "{detail}");

    // A proxied site shows its target, not PHP: no full version, no php or docroot source.
    let detail = c.call("sites.info", json!({"site": "app"})).await.unwrap();
    assert_eq!(detail["site"]["kind"], "proxy");
    assert_eq!(detail["site"]["target"], "http://127.0.0.1:5173");
    assert_eq!(detail["php_full"], Value::Null);
    assert_eq!(detail["sources"], json!({"secure": "default"}), "{detail}");
    assert_eq!(detail["services"], json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn info_outside_a_project_is_site_not_found() {
    let d = prepared("").await;
    d.project("blog");
    let d = d.start().await;
    let mut c = d.client().await;

    let outside = tempfile::tempdir().unwrap();
    for params in [json!({"path": outside.path().display().to_string()}), json!({})] {
        let e = c.call("sites.info", params).await.unwrap_err();
        assert_eq!(e.code, yard_proto::ErrorCode::SiteNotFound);
        assert_eq!(e.hint.as_deref(), Some("yard info <site> or yard link"));
    }
    let e = c.call("sites.info", json!({"site": "nope"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::SiteNotFound);
    assert!(e.message.contains("nope"), "{}", e.message);
    d.shutdown().await.unwrap();
}
