//! Tunnels in reconcile (M5): ports, the cloudflared yml, nginx's tunnel listener and the
//! `tunnel-<site>` unit, against the fake cloudflared.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
use support::{Fixture, fast, free_port, spawn_helper};
use tempfile::TempDir;
use yard_core::brew::Brew;
use yard_core::config::TomlFile;
use yard_core::ports::Registry;
use yard_core::tunnel::cloudflared::{Origin, named_yml, quick_yml};
use yard_core::tunnel::facts::{Facts, NamedFacts};
use yard_daemon::events::EventBus;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::supervisor::Supervisor;
use yard_proto::{PortMode, UnitInfo, UnitState};

struct Env {
    fx: Fixture,
    _resolver: TempDir,
    projects: TempDir,
    sup: Arc<Supervisor>,
    rec: Reconciler,
    /// The fake cloudflared's directory: its `calls` log.
    cloudflared: PathBuf,
}

async fn env() -> Env {
    let fx = Fixture::new();
    let cloudflared = fx.add_cloudflared();
    let resolver = tempfile::tempdir().unwrap();
    let helper = spawn_helper(&fx, resolver.path(), &[]).await;
    // Each env allocates `auto` ports from its own ten, below macOS's ephemeral range, so
    // parallel tests never hand two tunnels the same one.
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = 41000 + (std::process::id() % 50) as u16 * 100 + NEXT.fetch_add(1, Ordering::Relaxed) * 10;
    let ports = format!(
        "range = \"{start}-{}\"\n[ports]\nnginx-http = {{ port = {}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n",
        start + 9,
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
        bus,
        resolver.path().to_path_buf(),
    )
    .unwrap()
    .with_home(fx.paths.root().join("home"));
    let projects = tempfile::tempdir().unwrap();
    Env { fx, _resolver: resolver, projects, sup, rec, cloudflared }
}

impl Env {
    /// A project directory, linked as `name`, the way `yard link` run in it would.
    fn site(&self, name: &str) {
        let dir = self.projects.path().join(name);
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("public/index.php"), "<?php echo 'hi';\n").unwrap();
        self.set(&["sites", name, "path"], toml_edit::value(dir.display().to_string()));
    }

    fn set(&self, keys: &[&str], value: toml_edit::Item) {
        let mut file = TomlFile::load(&self.fx.paths.sites_toml()).unwrap();
        file.set(keys, value);
        file.save().unwrap();
    }

    /// What `yard tunnel <site>` will do through the RPC (Task 9): mark a quick tunnel and
    /// reconcile.
    async fn add_quick(&self, site: &str) {
        self.rec.tunnels().add_quick(site);
        self.rec.reconcile().await.unwrap();
    }

    fn entry(&self, name: &str) -> Option<(u16, PortMode)> {
        Registry::load(&self.fx.paths).unwrap().entries.get(name).map(|e| (e.port, e.mode))
    }

    fn origin(&self, site: &str) -> Origin {
        let (origin_port, _) = self.entry(&format!("nginx-tunnel-{site}")).expect("an origin entry");
        let (metrics_port, _) = self.entry(&format!("tunnel-{site}")).expect("a metrics entry");
        Origin { origin_port, metrics_port }
    }

    fn read(&self, path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

    fn has_unit(&self, name: &str) -> bool {
        self.sup.list().iter().any(|u| u.name == name)
    }
}

fn running(u: &UnitInfo) -> bool {
    u.state == UnitState::Running
}

#[tokio::test]
async fn a_quick_tunnel_gets_ports_a_yml_a_listener_and_a_unit() {
    let e = env().await;
    e.site("blog");
    e.rec.reconcile().await.unwrap();
    e.add_quick("blog").await;

    assert_eq!(e.entry("nginx-tunnel-blog").map(|(_, m)| m), Some(PortMode::Auto));
    assert_eq!(e.entry("tunnel-blog").map(|(_, m)| m), Some(PortMode::Auto));
    let o = e.origin("blog");
    assert_ne!(o.origin_port, o.metrics_port);
    assert_eq!(e.read(&e.fx.paths.tunnel_yml("blog")), quick_yml(o));
    let conf = e.read(&e.fx.paths.nginx_site("blog"));
    assert!(conf.contains(&format!("listen 127.0.0.1:{};", o.origin_port)), "{conf}");
    assert!(conf.contains(&e.fx.paths.tunnel_override("blog").display().to_string()), "{conf}");
    assert!(e.fx.paths.tunnel_override("blog").exists());
    e.unit("tunnel-blog", running).await;
    e.sup.shutdown().await;
}

#[tokio::test]
async fn ending_a_quick_tunnel_removes_all_of_it() {
    let e = env().await;
    e.site("blog");
    e.add_quick("blog").await;
    e.unit("tunnel-blog", running).await;
    let origin = e.origin("blog").origin_port;

    assert!(e.rec.tunnels().remove_quick("blog"));
    assert!(!e.rec.tunnels().remove_quick("blog"));
    e.rec.reconcile().await.unwrap();
    assert!(!e.has_unit("tunnel-blog"), "{:?}", e.sup.list());
    assert_eq!(e.entry("nginx-tunnel-blog"), None);
    assert_eq!(e.entry("tunnel-blog"), None);
    assert!(!e.fx.paths.tunnel_yml("blog").exists());
    let conf = e.read(&e.fx.paths.nginx_site("blog"));
    assert!(!conf.contains(&format!("listen 127.0.0.1:{origin};")), "{conf}");
    assert!(e.fx.paths.tunnel_override("blog").exists(), "the user's file stays");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn unrelated_changes_keep_the_tunnel_pid() {
    let e = env().await;
    e.fx.add_php("7.1");
    e.site("blog");
    e.add_quick("blog").await;
    let pid = e.unit("tunnel-blog", running).await.pid;
    assert!(pid.is_some());

    e.set(&["sites", "blog", "secure"], toml_edit::value(true));
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.unit("tunnel-blog", running).await.pid, pid, "after securing");
    e.set(&["sites", "blog", "php"], toml_edit::value("7.1"));
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.unit("tunnel-blog", running).await.pid, pid, "after isolating");
    e.site("shop");
    e.rec.reconcile().await.unwrap();
    assert_eq!(e.unit("tunnel-blog", running).await.pid, pid, "after linking another site");
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_dotted_site_name_tunnels() {
    let e = env().await;
    e.site("my.app");
    e.add_quick("my.app").await;
    e.unit("tunnel-my.app", running).await;
    assert_eq!(e.entry("nginx-tunnel-my.app").map(|(_, m)| m), Some(PortMode::Auto));
    assert_eq!(e.entry("tunnel-my.app").map(|(_, m)| m), Some(PortMode::Auto));
    assert!(e.fx.paths.tunnel_yml("my.app").exists());
    assert!(e.fx.paths.tunnel_override("my.app").exists());
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_named_tunnel_without_facts_is_a_warning() {
    let e = env().await;
    e.site("blog");
    e.set(&["sites", "blog", "tunnel", "host"], toml_edit::value("blog.example.com"));
    let report = e.rec.reconcile().await.unwrap();
    let want =
        "tunnel-blog: no Cloudflare tunnel is recorded for it; run yard tunnel blog --host blog.example.com";
    assert!(report.warnings.iter().any(|w| w == want), "{:?}", report.warnings);
    assert!(!e.has_unit("tunnel-blog"));
    assert!(!e.fx.paths.tunnel_yml("blog").exists());
    e.sup.shutdown().await;
}

#[tokio::test]
async fn a_named_tunnel_with_facts_runs() {
    let e = env().await;
    e.site("blog");
    e.set(&["sites", "blog", "tunnel", "host"], toml_edit::value("blog.example.com"));
    let credentials = e.fx.paths.root().join("home/.cloudflared/yard/example.com/yard-blog.json");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    std::fs::write(&credentials, "{}").unwrap();
    let mut facts = Facts::default();
    facts.named.insert(
        "blog".into(),
        NamedFacts {
            id: "1b2c".into(),
            name: "yard-blog".into(),
            created_by_yard: true,
            routed: vec!["blog.example.com".into()],
            credentials: credentials.clone(),
            domain: "example.com".into(),
        },
    );
    facts.save(&e.fx.paths.tunnels_state()).unwrap();

    let report = e.rec.reconcile().await.unwrap();
    assert!(!report.warnings.iter().any(|w| w.starts_with("tunnel-")), "{:?}", report.warnings);
    let o = e.origin("blog");
    assert_eq!(
        e.read(&e.fx.paths.tunnel_yml("blog")),
        named_yml(o, "1b2c", &credentials, "blog.example.com")
    );
    e.unit("tunnel-blog", running).await;
    let calls = e.read(&e.cloudflared.join("calls"));
    let yml = e.fx.paths.tunnel_yml("blog");
    assert_eq!(
        calls.lines().last(),
        Some(format!("tunnel --config {} run", yml.display()).as_str()),
        "{calls}"
    );
    e.sup.shutdown().await;
}
