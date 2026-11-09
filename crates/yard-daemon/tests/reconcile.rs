//! Reconcile against a temp data root, a fake brew, an in-process helper and fake units.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{Fixture, fast, free_port, spawn_helper};
use tempfile::TempDir;
use yard_core::brew::Brew;
use yard_core::config::TomlFile;
use yard_core::tls::{CA_DAYS, LEAF_DAYS};
use yard_daemon::events::EventBus;
use yard_daemon::helper_client::HelperClient;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::supervisor::Supervisor;
use yard_daemon::tls;
use yard_proto::{ErrorCode, Event, UnitInfo, UnitState};

const FPM: &str = "php-fpm-8.4";

struct Env {
    fx: Fixture,
    resolver: TempDir,
    projects: TempDir,
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
    let projects = tempfile::tempdir().unwrap();
    let ports = format!(
        "[ports]\nnginx-http = {{ port = {}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n",
        free_port(),
        free_port(),
        free_port()
    );
    std::fs::write(fx.paths.ports_toml(), ports).unwrap();
    let bus = EventBus::new();
    let sockets = Arc::new(PrivilegedSockets::new(helper.clone()));
    let sup = Arc::new(Supervisor::new(fast(), fx.paths.logs_dir(), bus.clone(), sockets));
    let rec = Reconciler::new(
        fx.paths.clone(),
        Brew::new(fx.brew.path()),
        sup.clone(),
        helper,
        bus.clone(),
        resolver.path().to_path_buf(),
    )
    .unwrap();
    Env { fx, resolver, projects, sup, rec, bus }
}

impl Env {
    /// A project directory, linked as its host label, the way `yard link` run in it would.
    fn site(&self, name: &str) -> PathBuf {
        let dir = self.projects.path().join(name);
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("public/index.php"), "<?php echo 'hi';\n").unwrap();
        let label = yard_core::sites::host_label(name).unwrap();
        let mut file = TomlFile::load(&self.fx.paths.sites_toml()).unwrap();
        file.set(&["sites", &label, "path"], toml_edit::value(dir.display().to_string()));
        file.save().unwrap();
        dir
    }

    fn secure(&self, label: &str, on: bool) {
        let mut file = TomlFile::load(&self.fx.paths.sites_toml()).unwrap();
        file.set(&["sites", label, "secure"], toml_edit::value(on));
        file.save().unwrap();
    }

    /// The names `label`'s certificate is issued for, sorted.
    fn cert_names(&self, label: &str) -> Vec<String> {
        let mut names = tls::facts(&self.fx.paths, label).expect("a certificate").dns_names;
        names.sort();
        names
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
async fn only_sites_whose_services_can_sleep_ask() {
    let e = env().await;
    let ports = std::fs::read_to_string(e.fx.paths.ports_toml()).unwrap();
    let pin = format!("\"redis\" = {{ port = {}, mode = \"pinned\" }}\n", free_port());
    std::fs::write(e.fx.paths.ports_toml(), ports + &pin).unwrap();
    e.fx.add_formula("redis");
    e.fx.add_service("redis", "redis", false);
    std::fs::write(e.site("shop").join(".yard"), "[services]\nredis = true\n").unwrap();
    e.site("blog");

    // With sleeping off (the default) nothing can be asleep, so no request asks yardd.
    e.rec.reconcile().await.unwrap();
    let shop = std::fs::read_to_string(e.conf("shop")).unwrap();
    assert!(!shop.contains("__yard_wake"), "{shop}");

    std::fs::write(e.fx.paths.yard_toml(), "service_sleep_after = \"15m\"\n").unwrap();
    e.rec.reconcile().await.unwrap();
    let shop = std::fs::read_to_string(e.conf("shop")).unwrap();
    assert!(shop.contains("auth_request /__yard_wake;"), "{shop}");
    let wake = e.fx.paths.wake_socket();
    assert!(shop.contains(&format!("\"http://unix:{}:/wake/shop?wait=10\"", wake.display())), "{shop}");
    let blog = std::fs::read_to_string(e.conf("blog")).unwrap();
    assert!(!blog.contains("__yard_wake"), "{blog}");
    e.sup.shutdown().await;
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

/// A yardd on a scratch root never repoints the system's resolver at its own dnsmasq: the
/// installed helper serves another root (found running Yard.app's checklist on a scratch root,
/// which sent this Mac's .yard names to a dnsmasq that then stopped).
#[tokio::test]
async fn a_helper_installed_for_another_root_is_left_alone() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let plist = fx.paths.root().join("helper.plist");
    let elsewhere = tempfile::tempdir().unwrap();
    let xml = yard_core::render::launchd::helper_plist(
        Path::new("/x/helper"),
        501,
        "me",
        elsewhere.path(),
        Path::new("/opt/homebrew"),
    );
    std::fs::write(&plist, xml).unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await.serving(fx.paths.root(), &plist);
    assert_eq!(helper.other_root().as_deref(), Some(elsewhere.path()));
    let e = env_with(fx, resolver, helper);
    e.site("blog");

    let report = e.rec.reconcile().await.unwrap();
    assert!(!e.resolver.path().join("yard").exists(), "the resolver is the other root's");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("the helper serves") && w.contains(&elsewhere.path().display().to_string())),
        "{report:?}"
    );
    // Its hint would move the Mac's resolver here, so the message says when to follow it.
    assert!(
        report.warnings.iter().any(|w| w.contains("only if this root should own the Mac's resolver")),
        "{report:?}"
    );
    e.unit("nginx", running).await;
    e.sup.shutdown().await;
}

/// The helper installed for this root is used as before.
#[tokio::test]
async fn a_helper_installed_for_this_root_installs_the_resolver() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let plist = fx.paths.root().join("helper.plist");
    let xml = yard_core::render::launchd::helper_plist(
        Path::new("/x/helper"),
        501,
        "me",
        fx.paths.root(),
        Path::new("/opt/homebrew"),
    );
    std::fs::write(&plist, xml).unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await.serving(fx.paths.root(), &plist);
    assert_eq!(helper.other_root(), None);
    let e = env_with(fx, resolver, helper);
    e.rec.reconcile().await.unwrap();
    assert!(e.resolver.path().join("yard").exists());
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
    std::fs::write(e.fx.paths.yard_toml(), "tld = \"test\"\n").unwrap();

    let err = e.rec.reconcile().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::ResolverForeign, "{err:?}");
    assert!(err.message.contains(".test"), "{}", err.message);
    assert_eq!(e.rec.snapshot().await.unwrap().tld, "yard");
    assert_eq!(e.rec.last_error().await.map(|e| e.code), Some(ErrorCode::ResolverForeign));
    e.sup.shutdown().await;
}

/// Two free ports stand in for 80 and 443: the helper may bind them, and ports.toml puts
/// the privileged profile on them.
async fn privileged_env() -> (Env, u16) {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let (http, https) = (free_port(), free_port());
    let helper = spawn_helper(&fx, resolver.path(), &[http, https]).await;
    let e = env_with(fx, resolver, helper);
    let ports = format!("profile = \"privileged\"\n{}", support::ports_toml(http, https, free_port()));
    std::fs::write(e.fx.paths.ports_toml(), ports).unwrap();
    (e, http)
}

#[tokio::test]
async fn privileged_profile_serves_on_helper_sockets() {
    let (e, http) = privileged_env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    let inherited = || support::inherited_starts(&e.fx.paths) == 1;
    assert!(support::wait_until(inherited, Duration::from_secs(5)).await, "nginx took the helper's sockets");
    assert!(support::served(http), "nginx serves on the socket it inherited");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn privileged_port_held_fails_nginx_with_the_holder() {
    let (e, http) = privileged_env().await;
    let _held = std::net::TcpListener::bind(("127.0.0.1", http)).unwrap();
    let mut rx = e.bus.subscribe();
    e.site("blog");
    e.rec.reconcile().await.unwrap();

    let nginx = e.unit("nginx", |u| matches!(u.state, UnitState::Failed { .. })).await;
    assert_eq!(nginx.restarts, 0, "retrying cannot free a port someone else holds: {nginx:?}");
    assert!(nginx.last_error.as_deref().is_some_and(|m| m.starts_with("PORT_IN_USE")), "{nginx:?}");
    let conflict = std::iter::from_fn(|| rx.try_recv().ok())
        .any(|ev| matches!(ev, Event::PortConflict { unit, port, .. } if unit == "nginx" && port == http));
    assert!(conflict, "a port_conflict event names nginx and the port");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn switching_to_standard_releases_the_privileged_sockets() {
    let (e, http) = privileged_env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    let inherited = || support::inherited_starts(&e.fx.paths) == 1;
    assert!(support::wait_until(inherited, Duration::from_secs(5)).await, "nginx took the helper's sockets");

    let standard = support::ports_toml(free_port(), free_port(), free_port());
    std::fs::write(e.fx.paths.ports_toml(), standard).unwrap();
    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    assert!(std::net::TcpListener::bind(("127.0.0.1", http)).is_ok(), "yardd let go of {http}");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn failed_nginx_releases_the_privileged_sockets() {
    let (e, http) = privileged_env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    let pid = e.unit("nginx", running).await.pid.unwrap();

    // nginx can no longer start: it backs off, then fails for good.
    std::fs::remove_file(Brew::new(e.fx.brew.path()).nginx_bin()).unwrap();
    // SAFETY: kill(2) on the fake nginx this test's supervisor started.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    e.unit("nginx", |u| matches!(u.state, UnitState::Failed { .. })).await;
    // Another test's child can hold every descriptor for the moment between its fork and
    // exec, so the port is free soon after, not necessarily at once. A leak never frees it.
    let free = || std::net::TcpListener::bind(("127.0.0.1", http)).is_ok();
    assert!(support::wait_until(free, Duration::from_secs(2)).await, "a failed nginx holds no port");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn orphaned_nginx_is_stopped_before_nginx_starts() {
    let e = env().await;
    let http = free_port();
    std::fs::write(e.fx.paths.ports_toml(), support::ports_toml(http, free_port(), free_port())).unwrap();
    e.site("blog");
    // An nginx a killed yardd left behind: reparented to launchd, on the port, in the pid file.
    let conf = e.fx.root.path().join("orphan.conf");
    std::fs::write(
        &conf,
        format!("daemon off;\nhttp {{\n    server {{\n        listen 127.0.0.1:{http};\n    }}\n}}\n"),
    )
    .unwrap();
    let nginx = Brew::new(e.fx.brew.path()).nginx_bin();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "\"$0\" -p \"$1\" -c \"$2\" >/dev/null 2>&1 & echo $!"])
        .arg(&nginx)
        .arg(e.fx.paths.nginx_dir())
        .arg(&conf)
        .output()
        .unwrap();
    let orphan: i32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    std::fs::write(e.fx.paths.nginx_pid(), format!("{orphan}\n")).unwrap();
    assert!(support::wait_until(|| support::served(http), Duration::from_secs(5)).await, "the orphan serves");

    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    // SAFETY: signal 0 only checks that the pid exists.
    let alive = unsafe { libc::kill(orphan, 0) } == 0;
    if alive {
        // SAFETY: kill(2) on the orphan this test started.
        unsafe { libc::kill(orphan, libc::SIGKILL) };
    }
    assert!(!alive, "yardd stopped the orphan before starting its own nginx");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn privileged_profile_on_unlisted_ports_fails_without_retrying() {
    let (e, _http) = privileged_env().await;
    // Hand-written: privileged, but on ports the helper does not open.
    let ports =
        format!("profile = \"privileged\"\n{}", support::ports_toml(free_port(), free_port(), free_port()));
    std::fs::write(e.fx.paths.ports_toml(), ports).unwrap();
    e.site("blog");
    e.rec.reconcile().await.unwrap();

    let nginx = e.unit("nginx", |u| matches!(u.state, UnitState::Failed { .. })).await;
    assert_eq!(nginx.restarts, 0, "retrying cannot change what the helper opens: {nginx:?}");
    assert!(nginx.last_error.as_deref().is_some_and(|m| m.starts_with("INVALID_ARGUMENT")), "{nginx:?}");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn old_parked_key_keeps_serving_with_a_warning() {
    let e = env().await;
    e.site("blog");
    std::fs::write(e.fx.paths.yard_toml(), "parked = [\"~/Herd\"]\n").unwrap();
    let report = e.rec.reconcile().await.unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("parked is no longer used")), "{report:?}");
    assert!(e.conf("blog").exists());
    e.sup.shutdown().await;
}

#[tokio::test]
async fn isolating_starts_that_pool_and_points_nginx_at_it() {
    let e = env().await;
    e.fx.add_php("7.1");
    e.site("old");
    e.site("new");
    e.rec.reconcile().await.unwrap();
    e.unit(FPM, running).await;

    let mut file = TomlFile::load(&e.fx.paths.sites_toml()).unwrap();
    file.set(&["sites", "old", "php"], toml_edit::value("7.1"));
    file.save().unwrap();
    e.rec.reconcile().await.unwrap();
    let conf = std::fs::read_to_string(e.conf("old")).unwrap();
    assert!(conf.contains("run/php-7.1.sock"), "{conf}");
    assert!(std::fs::read_to_string(e.conf("new")).unwrap().contains("run/php-8.4.sock"));
    e.unit("php-fpm-7.1", running).await;

    let mut file = TomlFile::load(&e.fx.paths.sites_toml()).unwrap();
    file.remove(&["sites", "old", "php"]);
    file.save().unwrap();
    e.rec.reconcile().await.unwrap();
    assert!(!e.sup.list().iter().any(|u| u.name == "php-fpm-7.1"), "{:?}", e.sup.list());
    e.sup.shutdown().await;
}

#[tokio::test]
async fn every_installed_version_gets_its_inis_but_no_pool() {
    let e = env().await;
    e.fx.add_php("7.1");
    let brew_conf_d = e.fx.brew.path().join("etc/php/7.1/conf.d");
    std::fs::write(brew_conf_d.join("ext-opcache.ini"), "zend_extension=\"opcache.so\"\n").unwrap();
    e.site("app");
    e.rec.reconcile().await.unwrap();
    e.unit(FPM, running).await;

    let p = &e.fx.paths;
    let v71: yard_core::PhpVersion = "7.1".parse().unwrap();
    assert!(p.php_ini(v71).is_file());
    assert!(p.fpm_conf(v71).is_file());
    let mirrored = p.php_conf_d(v71).join("10-brew-ext-opcache.ini");
    let text = std::fs::read_to_string(&mirrored).unwrap();
    assert!(text.ends_with("zend_extension=\"opcache.so\"\n"), "{text}");
    assert!(!e.sup.list().iter().any(|u| u.name == "php-fpm-7.1"), "{:?}", e.sup.list());
    e.sup.shutdown().await;
}

#[tokio::test]
async fn reconcile_links_the_shims() {
    let fx = Fixture::new();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;
    let mut e = env_with(fx, resolver, helper);
    let shim = e.projects.path().join("yard-shim");
    std::fs::write(&shim, "").unwrap();
    e.rec = e.rec.with_shim(Some(shim.clone()));
    let bin = e.fx.paths.bin_dir();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("composer"), "#!/bin/sh\n").unwrap();
    std::os::unix::fs::symlink("/old/build/yard-shim", bin.join("pecl")).unwrap();

    let report = e.rec.reconcile().await.unwrap();
    for name in ["php", "pecl", "phpize", "php-config", "node", "npm", "npx"] {
        assert_eq!(std::fs::read_link(bin.join(name)).unwrap(), shim, "{name}");
    }
    assert_eq!(
        std::fs::read_to_string(bin.join("composer")).unwrap(),
        "#!/bin/sh\n",
        "not Yard's: left alone"
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("composer") && w.contains("not Yard's")),
        "{:?}",
        report.warnings
    );

    std::fs::remove_file(&shim).unwrap();
    let report = e.rec.reconcile().await.unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("yard-shim")), "{:?}", report.warnings);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn securing_a_site_writes_its_certificate_and_ssl_block() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);

    let report = e.rec.reconcile().await.unwrap();
    assert!(report.written.contains(&e.fx.paths.cert("blog")), "{report:?}");
    assert!(e.fx.paths.cert_key("blog").exists());
    assert_eq!(e.cert_names("blog"), ["*.blog.yard", "blog.yard"]);
    assert!(tls::facts(&e.fx.paths, "blog").unwrap().signed_by_ca);
    let conf = std::fs::read_to_string(e.conf("blog")).unwrap();
    assert!(conf.contains("ssl_certificate"), "{conf}");
    assert!(!e.rec.certs_due().await);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn unsecuring_removes_the_certificate() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();

    e.secure("blog", false);
    let report = e.rec.reconcile().await.unwrap();
    assert!(report.removed.contains(&e.fx.paths.cert("blog")), "{report:?}");
    assert!(!e.fx.paths.cert("blog").exists());
    assert!(!e.fx.paths.cert_key("blog").exists());
    let conf = std::fs::read_to_string(e.conf("blog")).unwrap();
    assert!(!conf.contains("ssl_certificate"), "{conf}");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn deleted_certificate_is_reissued() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();

    std::fs::remove_file(e.fx.paths.cert("blog")).unwrap();
    assert!(e.rec.certs_due().await);
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.cert_names("blog"), ["*.blog.yard", "blog.yard"]);
    assert!(!e.rec.certs_due().await);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_new_ca_reissues_every_leaf() {
    let e = env().await;
    for name in ["blog", "shop"] {
        e.site(name);
        e.secure(name, true);
    }
    e.rec.reconcile().await.unwrap();
    let old_ca = tls::ca_sha256(&e.fx.paths).unwrap();

    std::fs::remove_dir_all(e.fx.paths.ca_dir()).unwrap();
    assert!(e.rec.certs_due().await);
    e.rec.reconcile().await.unwrap();
    assert_ne!(tls::ca_sha256(&e.fx.paths).unwrap(), old_ca);
    for name in ["blog", "shop"] {
        assert!(tls::facts(&e.fx.paths, name).unwrap().signed_by_ca, "{name}");
    }
    e.sup.shutdown().await;
}

#[tokio::test]
async fn tld_change_reissues_for_the_new_host() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();

    std::fs::write(e.fx.paths.yard_toml(), "tld = \"yard2\"\n").unwrap();
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.cert_names("blog"), ["*.blog.yard2", "blog.yard2"]);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn an_expiring_certificate_is_renewed() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();
    let now = time::OffsetDateTime::now_utc();
    tls::issue(&e.fx.paths, "blog", "blog.yard", now, 20).unwrap();

    assert!(e.rec.certs_due().await);
    e.rec.reconcile().await.unwrap();
    let not_after = tls::facts(&e.fx.paths, "blog").unwrap().not_after;
    let expected = (now + time::Duration::days(yard_core::tls::LEAF_DAYS)).unix_timestamp();
    assert!((not_after - expected).abs() < 86_400, "not_after {not_after}, expected about {expected}");
    assert!(!e.rec.certs_due().await);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn no_secure_site_makes_no_ca() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    assert!(!e.fx.paths.ca_dir().exists());
    assert!(!e.fx.paths.certs_dir().exists());
    assert!(!e.rec.certs_due().await);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_deleted_or_mismatched_key_is_reissued() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();

    std::fs::remove_file(e.fx.paths.cert_key("blog")).unwrap();
    assert!(e.rec.certs_due().await, "a missing key is due");
    e.rec.reconcile().await.unwrap();
    assert!(e.fx.paths.cert_key("blog").exists());

    // Another certificate's key beside this certificate: nginx would refuse the pair.
    let other = tempfile::tempdir().unwrap();
    let other_paths = yard_core::Paths::new(other.path());
    tls::ensure_ca(&other_paths, "me", time::OffsetDateTime::now_utc()).unwrap();
    tls::issue(&other_paths, "blog", "blog.yard", time::OffsetDateTime::now_utc(), 30).unwrap();
    std::fs::copy(other_paths.cert_key("blog"), e.fx.paths.cert_key("blog")).unwrap();
    assert!(e.rec.certs_due().await, "a mismatched key is due");
    e.rec.reconcile().await.unwrap();
    assert!(!e.rec.certs_due().await);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_ca_near_expiry_is_replaced_and_its_leaves_reissued() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();
    // A CA made long ago, with less left than a leaf's life, and a leaf from it.
    std::fs::remove_dir_all(e.fx.paths.ca_dir()).unwrap();
    let made = time::OffsetDateTime::now_utc() - time::Duration::days(CA_DAYS - LEAF_DAYS + 2);
    tls::ensure_ca(&e.fx.paths, "me", made).unwrap();
    tls::issue(&e.fx.paths, "blog", "blog.yard", time::OffsetDateTime::now_utc(), LEAF_DAYS).unwrap();
    let old_ca = tls::ca_sha256(&e.fx.paths).unwrap();

    assert!(e.rec.certs_due().await, "the CA is due");
    e.rec.reconcile().await.unwrap();
    assert_ne!(tls::ca_sha256(&e.fx.paths).unwrap(), old_ca);
    assert!(tls::facts(&e.fx.paths, "blog").unwrap().signed_by_ca);
    assert!(!e.rec.certs_due().await);
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_renewal_reloads_nginx_even_after_a_failed_test() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();
    e.unit("nginx", running).await;
    let log = e.fx.paths.log("nginx");
    let reloads = || std::fs::read_to_string(&log).unwrap_or_default().matches("fake nginx reloaded").count();

    // Renewal is due, but an override breaks nginx -t, so this reconcile rolls back.
    tls::issue(&e.fx.paths, "blog", "blog.yard", time::OffsetDateTime::now_utc(), 20).unwrap();
    std::fs::write(e.fx.paths.nginx_override("blog"), "yard-test-broken\n").unwrap();
    let err = e.rec.reconcile().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NginxConfigInvalid);
    assert_eq!(reloads(), 0);

    std::fs::write(e.fx.paths.nginx_override("blog"), "").unwrap();
    e.rec.reconcile().await.unwrap();
    assert!(
        support::wait_until(|| reloads() == 1, Duration::from_secs(5)).await,
        "nginx reloads the renewed certificate"
    );
    e.sup.shutdown().await;
}

#[tokio::test]
async fn leftover_temp_files_in_certs_are_removed() {
    let e = env().await;
    e.site("blog");
    e.secure("blog", true);
    e.rec.reconcile().await.unwrap();
    for stray in ["blog.key.yard-tmp", "gone.pem.yard-tmp"] {
        std::fs::write(e.fx.paths.certs_dir().join(stray), "").unwrap();
    }
    e.rec.reconcile().await.unwrap();
    let left: Vec<String> = listing(&e.fx.paths.certs_dir())
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["blog.key", "blog.pem"]);
    e.sup.shutdown().await;
}

#[test]
fn free_ports_are_never_ones_the_os_gives_connections() {
    // Any process's outbound connection can take an ephemeral port between a test choosing it and
    // its unit binding it; that flaked nginx with PORT_IN_USE held by an invisible process.
    let out =
        std::process::Command::new("sysctl").args(["-n", "net.inet.ip.portrange.first"]).output().unwrap();
    let first: u16 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    for _ in 0..50 {
        let port = free_port();
        assert!(port < first, "{port} is in the ephemeral range from {first}");
    }
}
