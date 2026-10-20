//! Reconcile against a temp data root, a fake brew, an in-process helper and fake units.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{Fixture, fast, free_port, spawn_helper};
use tempfile::TempDir;
use yard_core::brew::Brew;
use yard_daemon::events::EventBus;
use yard_daemon::helper_client::HelperClient;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::supervisor::Supervisor;
use yard_proto::{ErrorCode, Event, UnitInfo, UnitState};

const FPM: &str = "php-fpm-8.4";

struct Env {
    fx: Fixture,
    resolver: TempDir,
    parked: TempDir,
    sup: Arc<Supervisor>,
    rec: Reconciler,
    bus: EventBus,
}

async fn env() -> Env {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;
    env_with(fx, resolver, helper)
}

fn env_with(fx: Fixture, resolver: TempDir, helper: HelperClient) -> Env {
    let parked = tempfile::tempdir().unwrap();
    std::fs::write(fx.paths.yard_toml(), format!("parked = [{:?}]\n", parked.path().display().to_string()))
        .unwrap();
    let ports = format!(
        "[ports]\nnginx-http = {{ port = {}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n",
        free_port(),
        free_port(),
        free_port()
    );
    std::fs::write(fx.paths.ports_toml(), ports).unwrap();
    let bus = EventBus::new();
    let sup = Arc::new(Supervisor::new(fast(), fx.paths.logs_dir(), bus.clone()));
    let rec = Reconciler::new(
        fx.paths.clone(),
        Brew::new(fx.brew.path()),
        sup.clone(),
        helper,
        bus.clone(),
        resolver.path().to_path_buf(),
    )
    .unwrap();
    Env { fx, resolver, parked, sup, rec, bus }
}

impl Env {
    fn site(&self, name: &str) -> PathBuf {
        let dir = self.parked.path().join(name);
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("public/index.php"), "<?php echo 'hi';\n").unwrap();
        dir
    }

    fn conf(&self, label: &str) -> PathBuf {
        self.fx.paths.nginx_site(label)
    }

    async fn unit(&self, name: &str, done: impl Fn(&UnitInfo) -> bool) -> UnitInfo {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let u = self.sup.list().into_iter().find(|u| u.name == name);
            if let Some(u) = u.clone().filter(|u| done(u)) {
                return u;
            }
            assert!(Instant::now() < deadline, "{name}: timed out; last seen {u:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn unit_names(&self) -> Vec<String> {
        self.sup.list().into_iter().map(|u| u.name).collect()
    }
}

fn running(u: &UnitInfo) -> bool {
    u.state == UnitState::Running
}

fn listing(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    files
}

#[tokio::test]
async fn first_reconcile_writes_configs_and_starts_units() {
    let e = env().await;
    e.site("blog");

    let report = e.rec.reconcile().await.unwrap();
    assert!(report.written.contains(&e.conf("blog")), "{report:?}");
    for file in
        [e.conf("blog"), e.fx.paths.fpm_conf(yard_core::PhpVersion::DEFAULT), e.fx.paths.dnsmasq_conf()]
    {
        assert!(file.exists(), "{}", file.display());
    }
    assert!(e.fx.paths.nginx_override("blog").exists(), "the user-owned override is created");
    for unit in ["nginx", "dnsmasq", FPM] {
        e.unit(unit, running).await;
    }
    let resolver = std::fs::read_to_string(e.resolver.path().join("yard")).unwrap();
    assert!(resolver.starts_with("# Managed by Yard"), "{resolver}");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn reconcile_is_idempotent() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    let mut pids = vec![];
    for unit in ["nginx", "dnsmasq", FPM] {
        pids.push(e.unit(unit, running).await.pid);
    }

    let report = e.rec.reconcile().await.unwrap();
    assert!(report.written.is_empty() && report.removed.is_empty(), "{report:?}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after: Vec<Option<u32>> = ["nginx", "dnsmasq", FPM]
        .iter()
        .map(|n| e.sup.list().into_iter().find(|u| u.name == *n).and_then(|u| u.pid))
        .collect();
    assert_eq!(after, pids, "no unit restarted");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn no_php_sites_means_no_fpm_pool() {
    let e = env().await;
    std::fs::write(e.fx.paths.sites_toml(), "[sites.api]\nproxy = \"http://127.0.0.1:3000\"\n").unwrap();

    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    assert_eq!(e.unit_names(), vec!["dnsmasq", "nginx"]);
    assert!(e.conf("api").exists());
    let ports = std::fs::read_to_string(e.fx.paths.ports_toml()).unwrap();
    assert!(ports.contains("proxy:api") && ports.contains("3000"), "{ports}");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn removing_a_site_removes_its_conf_but_keeps_override() {
    let e = env().await;
    let dir = e.site("blog");
    e.rec.reconcile().await.unwrap();
    std::fs::write(e.fx.paths.nginx_override("blog"), "# mine\n").unwrap();

    std::fs::remove_dir_all(dir).unwrap();
    let report = e.rec.reconcile().await.unwrap();
    assert!(report.removed.contains(&e.conf("blog")), "{report:?}");
    assert!(!e.conf("blog").exists());
    assert_eq!(std::fs::read_to_string(e.fx.paths.nginx_override("blog")).unwrap(), "# mine\n");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn nginx_test_failure_restores_previous_configs() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    let before = std::fs::read(e.conf("blog")).unwrap();
    let index_before = std::fs::read(e.fx.paths.nginx_default_index()).unwrap();
    let broken = e.fx.paths.nginx_override("blog");
    std::fs::write(&broken, "yard-test-broken;\n").unwrap();

    e.site("shop");
    let err = e.rec.reconcile().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NginxConfigInvalid);
    let output = err.data.as_ref().and_then(|d| d["output"].as_str()).unwrap_or_default().to_string();
    assert!(err.message.contains(&broken.display().to_string()), "{} / {output}", err.message);
    assert!(output.contains(&broken.display().to_string()), "{output}");
    // The rendered files went back; whatever caused the change (here a new site) did not.
    assert!(err.message.contains("Yard kept serving the previous config"), "{}", err.message);
    assert!(err.hint.as_deref().unwrap_or_default().contains("saved"), "{:?}", err.hint);
    assert!(!e.conf("shop").exists(), "the new site's conf was rolled back");
    assert_eq!(std::fs::read(e.conf("blog")).unwrap(), before);
    assert_eq!(std::fs::read(e.fx.paths.nginx_default_index()).unwrap(), index_before);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn bad_toml_keeps_last_good_state() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    let sites_before = listing(&e.fx.paths.nginx_sites_dir());

    std::fs::write(e.fx.paths.sites_toml(), "[sites.blog\npath = 1\n").unwrap();
    let err = e.rec.reconcile().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);
    assert!(err.data.as_ref().is_some_and(|d| d["line"].is_number()), "{err:?}");
    assert_eq!(e.rec.last_error().await.map(|e| e.code), Some(ErrorCode::ConfigInvalid));
    assert!(e.rec.snapshot().await.is_some_and(|s| s.resolution.sites.len() == 1), "last good state kept");
    tokio::time::sleep(Duration::from_millis(100)).await;
    e.unit("nginx", running).await;
    assert_eq!(listing(&e.fx.paths.nginx_sites_dir()), sites_before);

    std::fs::write(e.fx.paths.sites_toml(), "").unwrap();
    e.rec.reconcile().await.unwrap();
    assert!(e.rec.last_error().await.is_none());
    e.sup.shutdown().await;
}

#[tokio::test]
async fn serves_site_in_dir_with_space() {
    let e = env().await;
    let dir = e.site("my app");

    e.rec.reconcile().await.unwrap();
    let conf = std::fs::read_to_string(e.conf("my-app")).unwrap();
    let root = format!("root \"{}\";", dir.join("public").display());
    assert!(conf.contains(&root), "{conf}");
    e.unit("nginx", running).await;
    e.sup.shutdown().await;
}

#[tokio::test]
async fn helper_missing_still_serves_nginx() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let e = env_with(fx, resolver, HelperClient::new("/tmp/yard-test-missing/helper.sock"));
    e.site("blog");

    // Only the resolver needs the helper now.
    let report = e.rec.reconcile().await.unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("HELPER_NOT_INSTALLED")), "{report:?}");
    for unit in ["dnsmasq", FPM, "nginx"] {
        e.unit(unit, running).await;
    }
    e.sup.shutdown().await;
}

#[tokio::test]
async fn nginx_is_a_child_of_yardd() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    let pid = e.unit("nginx", running).await.pid.expect("a running unit has a pid");
    assert_eq!(support::ppid(pid), std::process::id(), "yardd runs nginx itself, not through the helper");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn emits_site_added_and_removed() {
    let e = env().await;
    let mut rx = e.bus.subscribe();
    let dir = e.site("blog");
    e.rec.reconcile().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    e.rec.reconcile().await.unwrap();

    let mut sites = vec![];
    while let Ok(event) = rx.try_recv() {
        match event {
            Event::SiteAdded { site } => sites.push(format!("+{}", site.name)),
            Event::SiteRemoved { name } => sites.push(format!("-{name}")),
            _ => {}
        }
    }
    assert_eq!(sites, vec!["+blog", "-blog"]);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn failed_reconcile_is_remembered_until_one_succeeds() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    let broken = e.fx.paths.nginx_override("blog");
    std::fs::write(&broken, "yard-test-broken;\n").unwrap();

    e.site("shop");
    e.rec.reconcile().await.unwrap_err();
    let last = e.rec.state().await.last_error.expect("the failure is kept");
    assert_eq!(last.code, ErrorCode::NginxConfigInvalid);

    std::fs::write(&broken, "").unwrap();
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.rec.state().await.last_error, None);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn privileged_profile_keeps_the_tld() {
    let e = env().await;
    let ports = std::fs::read_to_string(e.fx.paths.ports_toml()).unwrap();
    std::fs::write(e.fx.paths.ports_toml(), format!("profile = \"privileged\"\n{ports}")).unwrap();
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.rec.snapshot().await.unwrap().tld, "yard");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn tld_with_a_foreign_resolver_is_refused_and_the_old_tld_serves() {
    let e = env().await;
    e.rec.reconcile().await.unwrap();
    // Herd's file: no Yard marker.
    std::fs::write(e.resolver.path().join("test"), "nameserver 127.0.0.1\n").unwrap();
    let yard = std::fs::read_to_string(e.fx.paths.yard_toml()).unwrap();
    std::fs::write(e.fx.paths.yard_toml(), format!("{yard}tld = \"test\"\n")).unwrap();

    let err = e.rec.reconcile().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ResolverForeign, "{err:?}");
    assert!(err.message.contains(".test"), "{}", err.message);
    assert_eq!(e.rec.snapshot().await.unwrap().tld, "yard");
    assert_eq!(e.rec.last_error().await.map(|e| e.code), Some(ErrorCode::ResolverForeign));
    e.sup.shutdown().await;
}
