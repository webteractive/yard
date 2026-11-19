//! Runtime installs (runtimes spec §6, §9): Composer versions from an index and the recipe's
//! pinned versions, checked by SHA-256 and renamed into `installs/<key>/<version>/`. The
//! downloads are `file://` fixtures shaped like getcomposer.org.

mod support;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use support::daemon::{Client, prepared, run_job};
use yard_core::Paths;
use yard_core::runtimes::Origin;
use yard_core::runtimes::recipe::{RuntimeRecipe, parse_runtime_recipe};
use yard_core::runtimes::version::VersionSpec;
use yard_core::services::engines::LoadOptions;
use yard_daemon::events::EventBus;
use yard_daemon::jobs::{JobCtx, Jobs};
use yard_daemon::runtimes;
use yard_proto::{ErrorCode, Event, YardError};

const OPTS: LoadOptions = LoadOptions { allow_file_urls: true };
const INDEX: &str = include_str!("../../yard-core/src/runtimes/testdata/composer-versions.json");

struct Fixture {
    dir: tempfile::TempDir,
    site: PathBuf,
    paths: Paths,
    hashes: std::collections::BTreeMap<String, String>,
}

/// getcomposer.org's layout under a temp dir: `versions`, and per version `composer.phar`
/// and its `.sha256sum`.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let site = dir.path().join("getcomposer");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("versions"), INDEX).unwrap();
    let mut hashes = std::collections::BTreeMap::new();
    for v in ["2.2.32", "2.11.2", "2.7.7"] {
        let d = site.join(v);
        std::fs::create_dir_all(&d).unwrap();
        let phar = format!("<?php echo 'Composer version {v}';\n");
        std::fs::write(d.join("composer.phar"), &phar).unwrap();
        let hash = yard_core::install::sha256_hex(phar.as_bytes());
        std::fs::write(d.join("composer.phar.sha256sum"), format!("{hash}  composer.phar\n")).unwrap();
        hashes.insert(v.to_string(), hash);
    }
    let paths = Paths::new(dir.path().join("root"));
    Fixture { dir, site, paths, hashes }
}

impl Drop for Fixture {
    /// Installs are read-only; let the temp dir go.
    fn drop(&mut self) {
        let _ = std::process::Command::new("/bin/chmod").args(["-R", "u+w"]).arg(self.dir.path()).status();
    }
}

/// The carried Composer recipe with its URLs pointing at the fixture.
fn recipe(f: &Fixture, index: &str) -> RuntimeRecipe {
    let (_, text) =
        yard_core::runtimes::carried::texts().into_iter().find(|(k, _)| *k == "composer").unwrap();
    let base = format!("file://{}", f.site.display());
    let text = text
        .replace("https://getcomposer.org/download", &base)
        .replace("https://getcomposer.org/versions", &format!("{base}/{index}"))
        .replace("241a3d13b7c6d35d970b1f69a000298b66943541302148a438a932f5dd44a9fc", &f.hashes["2.2.32"]);
    parse_runtime_recipe(&text, Path::new("composer.toml"), Origin::Carried, &OPTS).unwrap()
}

/// Runs `work` as a job and answers its progress lines and its error.
async fn job<F, Fut>(work: F) -> (Vec<String>, Option<YardError>)
where
    F: FnOnce(JobCtx) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), YardError>> + Send + 'static,
{
    let bus = EventBus::new();
    let mut events = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let id = jobs.start("runtime-install", work).job_id;
    let mut lines = vec![];
    loop {
        match events.recv().await.unwrap() {
            Event::JobProgress { job_id, line } if job_id == id => lines.push(line),
            Event::JobFinished { job_id, error, .. } if job_id == id => return (lines, error),
            _ => {}
        }
    }
}

async fn install(f: &Fixture, r: &RuntimeRecipe, spec: &str) -> (Vec<String>, Option<YardError>) {
    let (paths, r, spec) = (f.paths.clone(), r.clone(), VersionSpec::parse(spec).unwrap());
    job(move |ctx| async move { runtimes::install(&ctx, &paths, &r, spec, &OPTS).await.map(|_| ()) }).await
}

fn children(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn installs_a_channel_from_the_index() {
    let f = fixture();
    let (lines, error) = install(&f, &recipe(&f, "versions"), "2.2").await;
    assert_eq!(error, None, "{lines:?}");
    let root = f.paths.install_root("composer", "2.2.32");
    assert!(root.join("composer.phar").is_file());
    let got = runtimes::installed(&f.paths, "composer");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.requires_php, None);
    assert_eq!(got[0].1.sha256, f.hashes["2.2.32"]);
    assert!(lines.iter().any(|l| l == "Installed Composer 2.2.32"), "{lines:?}");
}

/// Composer's self-update refuses a phar it cannot write, so a spelling the shim's refusal
/// misses still cannot replace Yard's checked file; uninstall can still remove it.
#[tokio::test]
async fn an_installed_version_is_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let r = recipe(&f, "versions");
    assert_eq!(install(&f, &r, "2.2").await.1, None);
    let root = f.paths.install_root("composer", "2.2.32");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root.join("composer.phar")), 0o444);
    assert_eq!(mode(&root.join(".yard-install.toml")), 0o444);
    assert_eq!(mode(&root), 0o555);
    assert!(std::fs::write(root.join("composer.phar"), "x").is_err());
    let (paths, rr) = (f.paths.clone(), r.clone());
    let v = "2.2.32".parse().unwrap();
    let (_, error) = job(move |ctx| async move { runtimes::uninstall(&ctx, &paths, &rr, &v).await }).await;
    assert_eq!(error, None);
    assert!(!root.exists());
    assert_eq!(children(&f.paths.installs_dir().join("composer")), Vec::<String>::new());
}

#[tokio::test]
async fn installs_the_current_line_with_its_requirement() {
    let f = fixture();
    let (lines, error) = install(&f, &recipe(&f, "versions"), "2").await;
    assert_eq!(error, None, "{lines:?}");
    let got = runtimes::installed(&f.paths, "composer");
    assert_eq!(got[0].0.to_string(), "2.11.2");
    assert_eq!(got[0].1.requires_php.as_ref().map(ToString::to_string).as_deref(), Some("7.2.5"));
    // An exact version the index does not list comes by the URL template.
    let (lines, error) = install(&f, &recipe(&f, "versions"), "2.7.7").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(f.paths.install_root("composer", "2.7.7").join("composer.phar").is_file());
}

#[tokio::test]
async fn an_installed_version_is_not_fetched_again() {
    let f = fixture();
    let r = recipe(&f, "versions");
    assert_eq!(install(&f, &r, "2.2").await.1, None);
    std::fs::remove_file(f.site.join("2.2.32/composer.phar")).unwrap();
    let (lines, error) = install(&f, &r, "2.2").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "Composer 2.2.32 is installed"), "{lines:?}");
}

#[tokio::test]
async fn a_bad_hash_leaves_nothing() {
    let f = fixture();
    std::fs::write(
        f.site.join("2.2.32/composer.phar.sha256sum"),
        format!("{}  composer.phar\n", "0".repeat(64)),
    )
    .unwrap();
    // 2.2.32 is pinned in the recipe, so its hash is the recipe's; 2.11.2's comes from the
    // .sha256sum, which this test garbles.
    std::fs::write(
        f.site.join("2.11.2/composer.phar.sha256sum"),
        format!("{}  composer.phar\n", "0".repeat(64)),
    )
    .unwrap();
    let (_, error) = install(&f, &recipe(&f, "versions"), "2").await;
    assert_eq!(error.unwrap().code, ErrorCode::DownloadHashMismatch);
    assert_eq!(children(&f.paths.installs_dir().join("composer")), Vec::<String>::new());
}

#[tokio::test]
async fn a_garbled_sha256sum_is_refused() {
    let f = fixture();
    std::fs::write(f.site.join("2.11.2/composer.phar.sha256sum"), "<html>not found</html>\n").unwrap();
    let (_, error) = install(&f, &recipe(&f, "versions"), "2").await;
    let error = error.unwrap();
    assert!(error.message.contains("2.11.2/composer.phar.sha256sum"), "{}", error.message);
    assert_eq!(children(&f.paths.installs_dir().join("composer")), Vec::<String>::new());
}

#[tokio::test]
async fn an_unreachable_index_falls_back_to_the_pinned_lts() {
    let f = fixture();
    let r = recipe(&f, "missing-index");
    let (lines, error) = install(&f, &r, "2.2").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(f.paths.install_root("composer", "2.2.32").join("composer.phar").is_file());
    let (_, error) = install(&f, &r, "2").await;
    let error = error.unwrap();
    assert!(error.message.contains("missing-index"), "{}", error.message);
}

/// A download cancelled while curl waits on it (the phar is a FIFO nobody writes) ends the job
/// as JOB_CANCELLED and leaves no stage and no install.
#[tokio::test]
async fn a_cancelled_download_leaves_nothing() {
    let f = fixture();
    let phar = f.site.join("2.11.2/composer.phar");
    std::fs::remove_file(&phar).unwrap();
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&phar).status().unwrap().success());
    let (paths, r, spec) = (f.paths.clone(), recipe(&f, "versions"), VersionSpec::parse("2").unwrap());
    let bus = EventBus::new();
    let mut events = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let id = jobs
        .start("runtime-install", move |ctx| async move {
            runtimes::install(&ctx, &paths, &r, spec, &OPTS).await.map(|_| ())
        })
        .job_id;
    let error = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
            .await
            .expect("the job finished")
            .unwrap();
        match event {
            Event::JobProgress { job_id, line } if job_id == id && line.starts_with("downloading") => {
                // Long enough for curl to be blocked opening the FIFO.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                jobs.cancel(&id).unwrap();
            }
            Event::JobFinished { job_id, error, .. } if job_id == id => break error,
            _ => {}
        }
    };
    assert_eq!(error.unwrap().code, ErrorCode::JobCancelled);
    assert_eq!(children(&f.paths.installs_dir().join("composer")), Vec::<String>::new());
}

/// A captive portal answers the index URL with a page: as unreadable as no answer, so the
/// pinned LTS still installs, and the error for the rest says why.
#[tokio::test]
async fn an_index_that_is_not_json_falls_back_to_the_pinned_lts() {
    let f = fixture();
    std::fs::write(f.site.join("portal"), "<html><body>Sign in to the Wi-Fi</body></html>\n").unwrap();
    let r = recipe(&f, "portal");
    let (lines, error) = install(&f, &r, "2.2").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(f.paths.install_root("composer", "2.2.32").join("composer.phar").is_file());
    let (_, error) = install(&f, &r, "2").await;
    let error = error.unwrap();
    assert_eq!(error.code, ErrorCode::JobFailed);
    assert!(error.message.contains("portal") && error.message.contains("not JSON"), "{}", error.message);
}

#[tokio::test]
async fn a_partial_minor_is_refused_before_downloading() {
    let f = fixture();
    let (lines, error) = install(&f, &recipe(&f, "versions"), "2.7").await;
    let error = error.unwrap();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("exact"), "{}", error.message);
    assert!(lines.iter().all(|l| !l.contains("downloading")), "{lines:?}");
}

#[tokio::test]
async fn uninstall_removes_only_that_version() {
    let f = fixture();
    let r = recipe(&f, "versions");
    assert_eq!(install(&f, &r, "2.2").await.1, None);
    assert_eq!(install(&f, &r, "2").await.1, None);
    let (paths, rr) = (f.paths.clone(), r.clone());
    let v = "2.2.32".parse().unwrap();
    let (_, error) = job(move |ctx| async move { runtimes::uninstall(&ctx, &paths, &rr, &v).await }).await;
    assert_eq!(error, None);
    assert_eq!(children(&f.paths.installs_dir().join("composer")), ["2.11.2"]);
    let (paths, rr) = (f.paths.clone(), r.clone());
    let v = "2.2.32".parse().unwrap();
    let (_, error) = job(move |ctx| async move { runtimes::uninstall(&ctx, &paths, &rr, &v).await }).await;
    assert_eq!(error.unwrap().code, ErrorCode::RuntimeMissing);
}

/// A Composer version as `runtime.install` leaves it, without a download.
fn put_composer(paths: &Paths, version: &str) {
    let root = paths.install_root("composer", version);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("composer.phar"), "<?php\n").unwrap();
    let v: yard_core::runtimes::version::RuntimeVersion = version.parse().unwrap();
    let record = yard_core::runtimes::select::InstallRecord {
        requires_php: (v >= "2.3.0".parse().unwrap()).then(|| "7.2.5".parse().unwrap()),
        version: v,
        url: format!("https://getcomposer.org/download/{version}/composer.phar"),
        sha256: "a".repeat(64),
        installed_at: "2026-11-19T00:00:00Z".into(),
    };
    std::fs::write(paths.install_record("composer", version), record.render()).unwrap();
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

async fn site(c: &mut Client, name: &str) -> Value {
    let list = c.call("sites.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap_or_else(|| panic!("{list}"))
}

fn composer_of(site: &Value) -> Value {
    site["runtimes"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["key"] == "composer"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn runtimes_list_names_the_three_carried() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let list = c.call("runtimes.list", Value::Null).await.unwrap();
    let keys: Vec<&str> =
        list["runtimes"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["composer", "node", "php"], "{list}");
    assert!(list["runtimes"].as_array().unwrap().iter().all(|r| r["origin"] == "carried"), "{list}");
    let php = &list["runtimes"][2];
    assert!(php["installed"].as_array().unwrap().iter().any(|v| v == "8.4"), "{list}");
    assert_eq!(list["errors"], json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_site_with_composer_json_shows_its_composer() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    let dir = d.project("shop");
    write(&dir, "composer.json", "{}");
    write(&dir, ".yard", "php = \"7.1\"\n\n[runtimes]\ncomposer = \"2.2.32\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let composer = composer_of(&site(&mut c, "shop").await);
    assert_eq!(composer["spec"], "2.2.32", "{composer}");
    assert_eq!(composer["source"], "project_file", "{composer}");
    assert_eq!(composer["name"], "Composer", "{composer}");
    assert_eq!(composer["version"], Value::Null, "{composer}");
    put_composer(&d.fx.paths, "2.2.32");
    let composer = composer_of(&site(&mut c, "shop").await);
    assert_eq!(composer["version"], "2.2.32", "{composer}");
    let info = c.call("sites.info", json!({"site": "shop"})).await.unwrap();
    assert_eq!(composer_of(&info["site"])["version"], "2.2.32", "{info}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_site_without_composer_json_shows_none() {
    let d = prepared("").await;
    d.project("plain");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "plain").await;
    assert!(s.get("runtimes").is_none(), "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn use_writes_the_managed_file_and_unuse_removes_it() {
    let d = prepared("").await;
    let dir = d.project("shop");
    write(&dir, "composer.json", "{}");
    let managed = d.fx.paths.managed_project_file("shop");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, format!("# mine\nproject = {:?}\n", dir.display().to_string())).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let s = c.call("runtime.use", json!({"key": "composer", "site": "shop", "spec": "2.2"})).await.unwrap();
    let composer = composer_of(&s);
    assert_eq!(
        (composer["spec"].as_str(), composer["source"].as_str()),
        (Some("2.2"), Some("managed_file")),
        "{s}"
    );
    let text = std::fs::read_to_string(&managed).unwrap();
    assert!(text.contains("[runtimes]\ncomposer = \"2.2\""), "{text}");
    assert!(text.starts_with("# mine\n"), "{text}");
    c.call("runtime.unuse", json!({"key": "composer", "site": "shop"})).await.unwrap();
    let text = std::fs::read_to_string(&managed).unwrap();
    assert!(!text.contains("runtimes"), "{text}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn use_is_refused_for_a_committed_yard() {
    let d = prepared("").await;
    let dir = d.project("shop");
    write(&dir, ".yard", "php = \"8.4\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let e =
        c.call("runtime.use", json!({"key": "composer", "site": "shop", "spec": "2.2"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::SiteHasProjectFile);
    assert!(e.hint.unwrap().contains("[runtimes] composer = \"2.2\""));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn use_writes_sites_toml_without_a_managed_file() {
    let d = prepared("").await;
    let dir = d.project("shop");
    write(&dir, "composer.json", "{}");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = c.call("runtime.use", json!({"key": "composer", "site": "shop", "spec": "2"})).await.unwrap();
    assert_eq!(composer_of(&s)["source"], "sites_toml", "{s}");
    let sites = yard_core::config::load_sites(&d.fx.paths).unwrap();
    assert_eq!(sites.sites["shop"].runtimes["composer"].to_string(), "2");
    c.call("runtime.unuse", json!({"key": "composer", "site": "shop"})).await.unwrap();
    let sites = yard_core::config::load_sites(&d.fx.paths).unwrap();
    assert!(sites.sites["shop"].runtimes.is_empty());
    let e = c.call("runtime.use", json!({"key": "bun", "site": "shop", "spec": "1"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RuntimeUnknown);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn default_writes_yard_toml_and_needs_an_installed_match() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let e = c.call("runtime.default", json!({"key": "composer", "spec": "2"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RuntimeMissing);
    put_composer(&d.fx.paths, "2.11.2");
    c.call("runtime.default", json!({"key": "composer", "spec": "2"})).await.unwrap();
    let (yard, _) = yard_core::config::load_yard(&d.fx.paths).unwrap();
    assert_eq!(yard.runtime_defaults["composer"].to_string(), "2");
    let list = c.call("runtime.list", json!({"key": "composer"})).await.unwrap();
    assert_eq!(list["default"], "2", "{list}");
    assert_eq!(list["versions"][0]["version"], "2.11.2", "{list}");
    assert_eq!(list["versions"][0]["default"], true, "{list}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn uninstall_refuses_the_only_version_a_site_runs() {
    let d = prepared("").await;
    d.fx.add_php("7.1");
    let dir = d.project("shop");
    write(&dir, "composer.json", "{}");
    write(&dir, ".yard", "php = \"7.1\"\n\n[runtimes]\ncomposer = \"2.2.32\"\n");
    put_composer(&d.fx.paths, "2.2.32");
    put_composer(&d.fx.paths, "2.11.2");
    let d = d.start().await;
    let mut c = d.client().await;
    let e = c.call("runtime.uninstall", json!({"key": "composer", "version": "2.2.32"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RuntimeInUse);
    assert!(e.message.contains("shop"), "{}", e.message);
    let (_, error) =
        run_job(&mut c, "runtime.uninstall", json!({"key": "composer", "version": "2.11.2"})).await;
    assert_eq!(error, None);
    assert!(!d.fx.paths.install_root("composer", "2.11.2").exists());
    d.shutdown().await.unwrap();
}

/// yard.toml's default is a plain prefix, as the shim takes it, so the recipe's rule for the
/// same spec (`2`, excluding 2.2) must not decide what else can run the site.
#[tokio::test]
async fn uninstall_follows_yard_toml_default_not_the_recipe_rule() {
    let d = prepared("").await;
    let dir = d.project("shop");
    write(&dir, "composer.json", "{}");
    write(&dir, ".yard", "php = \"8.4\"\n");
    put_composer(&d.fx.paths, "2.2.32");
    put_composer(&d.fx.paths, "2.11.2");
    let d = d.start().await;
    let mut c = d.client().await;
    c.call("runtime.default", json!({"key": "composer", "spec": "2"})).await.unwrap();
    assert_eq!(composer_of(&site(&mut c, "shop").await)["version"], "2.11.2");
    let (_, error) =
        run_job(&mut c, "runtime.uninstall", json!({"key": "composer", "version": "2.11.2"})).await;
    assert_eq!(error, None);
    assert_eq!(composer_of(&site(&mut c, "shop").await)["version"], "2.2.32");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unknown_runtime_in_yard_is_reported_not_fatal() {
    let d = prepared("").await;
    let dir = d.project("shop");
    write(&dir, ".yard", "php = \"8.4\"\n\n[runtimes]\nbun = \"1\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "shop").await;
    assert!(s.get("project_error").is_none() && s.get("needs").is_none(), "{s}");
    let bun = s["runtimes"].as_array().unwrap().iter().find(|r| r["key"] == "bun").cloned().unwrap();
    assert_eq!(bun["error"]["code"], "RUNTIME_UNKNOWN", "{bun}");
    assert_eq!(bun["spec"], "1", "{bun}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_install_node_delegates() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let started = c.call("runtime.install", json!({"key": "node", "spec": "22"})).await.unwrap();
    assert!(started["job_id"].as_str().unwrap().starts_with("node-install-"), "{started}");
    // Installed already, so the job finds it without the network.
    put_composer(&d.fx.paths, "2.2.32");
    let (lines, error) =
        run_job(&mut c, "runtime.install", json!({"key": "composer", "spec": "2.2.32"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "Composer 2.2.32 is installed"), "{lines:?}");
    // Refused before any job: 2.7 is not a channel.
    let e = c.call("runtime.install", json!({"key": "composer", "spec": "2.7"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    let e = c.call("runtime.install", json!({"key": "composer"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    let e = c.call("runtime.list", json!({"key": "bun"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RuntimeUnknown);
    d.shutdown().await.unwrap();
}

/// `yard init` moves a site's `yard runtime use` pin from sites.toml into the managed `.yard`,
/// so a later `init --force` cannot write the old pin back over a newer one.
#[tokio::test]
async fn init_moves_a_runtime_pin_out_of_sites_toml() {
    let d = prepared("").await;
    let dir = d.project("shop");
    write(&dir, "composer.json", "{}");
    let d = d.start().await;
    let mut c = d.client().await;
    c.call("runtime.use", json!({"key": "composer", "site": "shop", "spec": "2.2"})).await.unwrap();
    c.call("sites.init", json!({"path": dir.display().to_string(), "name": "shop"})).await.unwrap();
    let sites = yard_core::config::load_sites(&d.fx.paths).unwrap();
    assert!(sites.sites["shop"].runtimes.is_empty(), "{:?}", sites.sites["shop"]);
    let managed = d.fx.paths.managed_project_file("shop");
    assert!(std::fs::read_to_string(&managed).unwrap().contains("[runtimes]\ncomposer = \"2.2\""));
    c.call("runtime.use", json!({"key": "composer", "site": "shop", "spec": "2"})).await.unwrap();
    c.call("sites.init", json!({"path": dir.display().to_string(), "name": "shop", "force": true}))
        .await
        .unwrap();
    let text = std::fs::read_to_string(&managed).unwrap();
    assert!(text.contains("composer = \"2\"") && !text.contains("composer = \"2.2\""), "{text}");
    d.shutdown().await.unwrap();
}
