//! The `path` and `download` sources (recipes spec §5): what `yard service add` installs, the
//! version an instance keeps, and `engines.install` moving it (§5.1). Downloads come from
//! `file://` archives the tests build around `yard-fake-unit` as `fake-service`.

mod support;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::daemon::{Client, Daemon, prepared, run_job};
use support::{free_port, served, wait_until};
use yard_core::config::load_services;

/// A `.tar.gz` holding `fake-service` (a link to the fake) under `top/`, and its SHA-256.
fn archive(dir: &Path, fake: &Path, name: &str) -> (PathBuf, String) {
    let stage = dir.join(format!("stage-{name}"));
    std::fs::create_dir_all(stage.join("top")).unwrap();
    std::os::unix::fs::symlink(fake, stage.join("top/fake-service")).unwrap();
    let file = dir.join(format!("{name}.tar.gz"));
    let ok = std::process::Command::new("/usr/bin/tar")
        .args(["-czf", &file.display().to_string(), "-C", &stage.display().to_string(), "top"])
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let sha = yard_core::install::sha256_hex(&std::fs::read(&file).unwrap());
    (file, sha)
}

fn download_recipe(version: &str, url: &str, sha: &str, usual: u16) -> String {
    format!(
        "schema = 1\nkey = \"dl\"\nname = \"Downloaded\"\n\n[source]\nkind = \"download\"\nversion = \"{version}\"\n\
         url = \"{url}\"\nsha256 = \"{sha}\"\nstrip = 1\n\n[ports]\nmain = {usual}\n\n[run]\n\
         command = [\"{{home}}/fake-service\", \"127.0.0.1:{{port.main}}\"]\n\n[health]\ntcp = \"main\"\n"
    )
}

fn path_recipe(program: &Path, usual: u16) -> String {
    format!(
        "schema = 1\nkey = \"pathed\"\nname = \"Pathed\"\n\n[source]\nkind = \"path\"\nprogram = \"{}\"\n\n\
         [ports]\nmain = {usual}\n\n[run]\ncommand = [\"{{program}}\", \"127.0.0.1:{{port.main}}\"]\n\n[health]\ntcp = \"main\"\n",
        program.display()
    )
}

/// Adds `engine` once yardd knows it (its recipe was just written): the call is refused with
/// `INVALID_ARGUMENT` until the watcher's reconcile.
async fn add_when_known(c: &mut Client, engine: &str) -> (Vec<String>, Option<yard_proto::YardError>) {
    for _ in 0..100 {
        let known = c.call("engines.list", Value::Null).await.unwrap();
        if known["engines"].as_array().unwrap().iter().any(|e| e["key"] == engine) {
            return run_job(c, "services.add", json!({"engine": engine})).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("yardd never learned {engine}");
}

fn entry(d: &Daemon, instance: &str) -> Option<yard_core::config::services::ServiceEntry> {
    load_services(&d.fx.paths).unwrap().services.get(instance).cloned()
}

fn children(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).map_or_else(
        |_| vec![],
        |r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
    )
}

async fn pid(c: &mut Client, name: &str) -> Option<u64> {
    let s = c.call("status", Value::Null).await.unwrap();
    s["units"].as_array()?.iter().find(|u| u["name"] == name && u["state"] == "running")?["pid"].as_u64()
}

async fn running_pid(c: &mut Client, name: &str) -> u64 {
    for _ in 0..200 {
        if let Some(p) = pid(c, name).await {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{name} never ran");
}

#[tokio::test]
async fn path_source_runs_the_program() {
    let d = prepared("").await;
    let tools = tempfile::tempdir().unwrap();
    let program = tools.path().join("fake-service");
    std::os::unix::fs::symlink(&d.fx.fake, &program).unwrap();
    let port = free_port();
    d.fx.add_recipe("pathed", &path_recipe(&program, port));
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = add_when_known(&mut c, "pathed").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(wait_until(|| served(port), Duration::from_secs(5)).await);
    let e = entry(&d, "pathed").unwrap();
    assert_eq!((e.formula, e.version), (None, None), "a path program is the user's: nothing to record");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn path_source_missing_is_refused() {
    let d = prepared("").await;
    d.fx.add_recipe("pathed", &path_recipe(Path::new("/nonexistent/yard-test/fake-service"), free_port()));
    let d = d.start().await;
    let mut c = d.client().await;
    let (_, error) = add_when_known(&mut c, "pathed").await;
    let error = error.expect("refused");
    assert_eq!(error.code, yard_proto::ErrorCode::EngineNotInstalled, "{error:?}");
    assert!(entry(&d, "pathed").is_none());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn download_installs_and_records_version() {
    let d = prepared("").await;
    let files = tempfile::tempdir().unwrap();
    let (file, sha) = archive(files.path(), &d.fx.fake, "one");
    let port = free_port();
    d.fx.add_recipe("dl", &download_recipe("1.0", &format!("file://{}", file.display()), &sha, port));
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = add_when_known(&mut c, "dl").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(wait_until(|| served(port), Duration::from_secs(5)).await);
    let e = entry(&d, "dl").unwrap();
    assert_eq!((e.formula.as_deref(), e.version.as_deref()), (None, Some("1.0")));
    assert!(d.fx.paths.program_root("dl", "1.0").join("fake-service").exists(), "strip = 1 drops top/");
    assert_eq!(children(&d.fx.paths.programs_dir().join("dl")), ["1.0"], "no stage is left");

    // The recipe still pins 1.0, so removing the instance keeps it; once the pin moves, it goes.
    c.call("services.remove", json!({"name": "dl"})).await.unwrap();
    assert!(d.fx.paths.program_root("dl", "1.0").exists(), "the recipe still pins it");
    d.fx.add_recipe("dl", &download_recipe("1.1", &format!("file://{}", file.display()), &sha, port));
    let mut gone = false;
    for _ in 0..100 {
        let known = c.call("engines.list", Value::Null).await.unwrap();
        if known["engines"].as_array().unwrap().iter().any(|e| e["key"] == "dl" && e["version"] == "1.1") {
            let (_, error) = run_job(&mut c, "engines.install", json!({"key": "dl"})).await;
            assert_eq!(error, None);
            gone = !d.fx.paths.program_root("dl", "1.0").exists();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "1.0 is swept once nothing names it");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn download_bad_hash_leaves_nothing() {
    let d = prepared("").await;
    let files = tempfile::tempdir().unwrap();
    let (file, _) = archive(files.path(), &d.fx.fake, "one");
    d.fx.add_recipe(
        "dl",
        &download_recipe("1.0", &format!("file://{}", file.display()), &"b".repeat(64), free_port()),
    );
    let d = d.start().await;
    let mut c = d.client().await;
    let (_, error) = add_when_known(&mut c, "dl").await;
    let error = error.expect("refused");
    assert_eq!(error.code, yard_proto::ErrorCode::DownloadHashMismatch, "{error:?}");
    assert!(error.message.contains(&"b".repeat(64)), "{}", error.message);
    assert!(children(&d.fx.paths.programs_dir().join("dl")).is_empty());
    assert!(entry(&d, "dl").is_none());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_download_leaves_nothing() {
    let d = prepared("").await;
    let files = tempfile::tempdir().unwrap();
    // A FIFO nobody writes: the download waits on it until cancelled.
    let fifo = files.path().join("never.tar.gz");
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&fifo).status().unwrap().success());
    d.fx.add_recipe(
        "dl",
        &download_recipe("1.0", &format!("file://{}", fifo.display()), &"a".repeat(64), free_port()),
    );
    let d = d.start().await;
    let mut c = d.client().await;
    for _ in 0..100 {
        let known = c.call("engines.list", Value::Null).await.unwrap();
        if known["engines"].as_array().unwrap().iter().any(|e| e["key"] == "dl") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    c.call("subscribe", json!({"topics": ["jobs"]})).await.unwrap();
    let job = c.call("services.add", json!({"engine": "dl"})).await.unwrap();
    let id = job["job_id"].as_str().unwrap().to_string();
    let stage = d.fx.paths.programs_dir().join("dl");
    assert!(wait_until(|| !children(&stage).is_empty(), Duration::from_secs(5)).await, "the download began");
    c.call("jobs.cancel", json!({"job_id": id})).await.unwrap();
    let done = c
        .event_where(Duration::from_secs(10), |e| e["type"] == "job_finished" && e["job_id"] == id.as_str())
        .await
        .expect("the job ended");
    assert_eq!(done["error"]["code"], "JOB_CANCELLED", "{done}");
    assert!(children(&stage).is_empty(), "no stage and no install root: {:?}", children(&stage));
    assert!(entry(&d, "dl").is_none());
    d.shutdown().await.unwrap();
}

/// Installs `dl` 1.0, then pins 2.0: the instance keeps 1.0 until `engines.install` (§5.1).
#[tokio::test]
async fn pin_change_keeps_running_version_until_install() {
    let d = prepared("").await;
    let files = tempfile::tempdir().unwrap();
    let (one, one_sha) = archive(files.path(), &d.fx.fake, "one");
    let (two, two_sha) = archive(files.path(), &d.fx.fake, "two");
    let port = free_port();
    d.fx.add_recipe("dl", &download_recipe("1.0", &format!("file://{}", one.display()), &one_sha, port));
    let d = d.start().await;
    let mut c = d.client().await;
    let (lines, error) = add_when_known(&mut c, "dl").await;
    assert_eq!(error, None, "{lines:?}");
    let first = running_pid(&mut c, "dl").await;

    // pin_change_keeps_running_version
    d.fx.add_recipe("dl", &download_recipe("2.0", &format!("file://{}", two.display()), &two_sha, port));
    let mut pinned = Value::Null;
    for _ in 0..100 {
        let list = c.call("engines.list", Value::Null).await.unwrap();
        pinned = list["engines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["key"] == "dl")
            .cloned()
            .unwrap_or_default();
        if pinned["version"] == "2.0" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(pinned["version"], "2.0", "{pinned}");
    assert_eq!(pinned["installed"], false, "2.0 is not installed yet: {pinned}");
    assert_eq!(entry(&d, "dl").unwrap().version.as_deref(), Some("1.0"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(pid(&mut c, "dl").await, Some(first), "the instance keeps 1.0");

    // engines_install_moves_instance
    let (lines, error) = run_job(&mut c, "engines.install", json!({"key": "dl"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert_eq!(entry(&d, "dl").unwrap().version.as_deref(), Some("2.0"));
    assert!(
        wait_until(|| !d.fx.paths.program_root("dl", "1.0").exists(), Duration::from_secs(5)).await,
        "1.0 is swept"
    );
    let mut second = first;
    for _ in 0..200 {
        second = pid(&mut c, "dl").await.unwrap_or(first);
        if second != first {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(second, first, "the instance restarted on 2.0");

    // missing_install_root_is_not_installed
    std::fs::remove_dir_all(d.fx.paths.program_root("dl", "2.0")).unwrap();
    d.fx.add_recipe("dl", &download_recipe("2.0", &format!("file://{}", two.display()), &two_sha, port));
    let mut state = Value::Null;
    for _ in 0..200 {
        let rows = c.call("services.list", Value::Null).await.unwrap();
        state = rows.as_array().unwrap().iter().find(|r| r["name"] == "dl").unwrap()["state"].clone();
        if state["failed"]["reason"]["code"] == "ENGINE_NOT_INSTALLED" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(state["failed"]["reason"]["code"], "ENGINE_NOT_INSTALLED", "{state}");
    assert_eq!(state["failed"]["reason"]["hint"], "yard engines install dl");
    d.shutdown().await.unwrap();
}
