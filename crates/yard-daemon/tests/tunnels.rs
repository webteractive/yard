//! Tunnels in reconcile (M5): ports, the cloudflared yml, nginx's tunnel listener and the
//! `tunnel-<site>` unit, against the fake cloudflared.

mod support;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::daemon::{Client, Daemon, prepared, run_job_result};
use support::{Fixture, auto_range, fast, free_port, spawn_helper};
use tempfile::TempDir;
use yard_core::brew::Brew;
use yard_core::config::TomlFile;
use yard_core::ports::Registry;
use yard_core::tunnel::cloudflared::{
    Origin, cert_path, created_credentials, default_credentials, named_yml, quick_yml,
};
use yard_core::tunnel::facts::{Facts, NamedFacts};
use yard_daemon::events::EventBus;
use yard_daemon::privileged::PrivilegedSockets;
use yard_daemon::reconcile::Reconciler;
use yard_daemon::supervisor::Supervisor;
use yard_proto::{ErrorCode, PortMode, UnitInfo, UnitState, YardError};

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
    let ports = format!(
        "{}[ports]\nnginx-http = {{ port = {}, mode = \"pinned\" }}\nnginx-https = {{ port = {}, mode = \"pinned\" }}\ndnsmasq = {{ port = {}, mode = \"pinned\" }}\n",
        auto_range(),
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

/// A started daemon with the fake cloudflared and `blog` linked, its `auto` ports from a range
/// of its own.
async fn tunnel_daemon() -> Daemon {
    let d = prepared("").await;
    d.fx.add_cloudflared();
    let ports = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    std::fs::write(d.fx.paths.ports_toml(), format!("{}{ports}", auto_range())).unwrap();
    d.project("blog");
    d.start().await
}

/// `tunnels.add` for a quick tunnel: the job's lines, error and result.
async fn add_quick(d: &Daemon, site: &str) -> (Vec<String>, Option<YardError>, Value) {
    let mut c = d.client().await;
    run_job_result(&mut c, "tunnels.add", json!({"site": site})).await
}

/// A daemon whose `blog` has a quick tunnel `tunnels.add` started.
async fn quick_daemon() -> Daemon {
    let d = tunnel_daemon().await;
    let (_, error, _) = add_quick(&d, "blog").await;
    assert_eq!(error, None);
    d
}

fn tunnel_entries(d: &Daemon) -> Vec<String> {
    let registry = Registry::load(&d.fx.paths).unwrap();
    registry
        .entries
        .into_keys()
        .filter(|k| k.starts_with("tunnel-") || k.starts_with("nginx-tunnel-"))
        .collect()
}

/// `blog`'s entry in `tunnels.list` once `done` holds for it.
async fn tunnel_where(c: &mut Client, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let list = c.call("tunnels.list", Value::Null).await.unwrap();
        let blog = list.as_array().unwrap().iter().find(|t| t["site"] == "blog").cloned();
        if let Some(t) = blog.clone().filter(|t| done(t)) {
            return t;
        }
        assert!(Instant::now() < deadline, "blog's tunnel: timed out; last seen {blog:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn unit_pid(c: &mut Client, name: &str) -> Value {
    let units = c.call("status", Value::Null).await.unwrap()["units"].clone();
    units.as_array().unwrap().iter().find(|u| u["name"] == name).map(|u| u["pid"].clone()).unwrap_or_default()
}

#[tokio::test]
async fn tunnels_list_shows_a_quick_url_once_known() {
    let d = quick_daemon().await;
    let mut c = d.client().await;
    let t = tunnel_where(&mut c, |t| t["state"] == "ready" && !t["url"].is_null()).await;
    let origin = t["origin_port"].as_u64().unwrap();
    assert_eq!(t["url"], format!("https://quick-{origin}.trycloudflare.com"));
    assert_eq!(t["kind"], "quick");
    assert_eq!(t["unit"], "tunnel-blog");
    assert_eq!(t["provider"], "cloudflared");
    assert!(t["host"].is_null() && t["tunnel"].is_null(), "{t}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn quick_tunnel_survives_losing_the_edge() {
    let d = quick_daemon().await;
    let cloudflared = Brew::new(d.fx.brew.path()).cloudflared_bin().parent().unwrap().to_path_buf();
    let mut c = d.client().await;
    let ready = tunnel_where(&mut c, |t| t["state"] == "ready" && !t["url"].is_null()).await;
    let pid = unit_pid(&mut c, "tunnel-blog").await;
    let mut events = d.client().await;
    events.call("subscribe", json!({"topics": ["tunnels"]})).await.unwrap();

    std::fs::write(cloudflared.join("not-ready"), "").unwrap();
    let changed = events
        .event_where(Duration::from_secs(2), |e| {
            e["type"] == "tunnel_changed" && e["tunnel"]["state"] == "connecting"
        })
        .await;
    assert!(changed.is_some(), "a tunnel_changed for connecting");
    tunnel_where(&mut c, |t| t["state"] == "connecting").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(unit_pid(&mut c, "tunnel-blog").await, pid, "the unit is not restarted");

    std::fs::remove_file(cloudflared.join("not-ready")).unwrap();
    let again = tunnel_where(&mut c, |t| t["state"] == "ready").await;
    assert_eq!(again["url"], ready["url"]);
    assert_eq!(unit_pid(&mut c, "tunnel-blog").await, pid);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn quick_tunnel_add_and_remove_by_rpc() {
    let d = tunnel_daemon().await;
    let (lines, error, result) = add_quick(&d, "blog").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "Starting a quick tunnel for blog…"), "{lines:?}");
    assert_eq!(result["state"], "ready", "{result}");
    let origin = result["origin_port"].as_u64().unwrap();
    assert_eq!(result["url"], format!("https://quick-{origin}.trycloudflare.com"));
    assert_eq!(tunnel_entries(&d).len(), 2);

    let mut c = d.client().await;
    let (_, error, removed) = run_job_result(&mut c, "tunnels.remove", json!({"site": "blog"})).await;
    assert_eq!(error, None);
    assert_eq!(removed, json!({"site": "blog", "deleted_tunnel": false, "dns_left": []}));
    assert_eq!(c.call("tunnels.list", Value::Null).await.unwrap(), json!([]));
    assert_eq!(unit_pid(&mut c, "tunnel-blog").await, Value::Null);
    assert!(tunnel_entries(&d).is_empty(), "{:?}", tunnel_entries(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn env_prints_the_tunnel_app_url() {
    let d = tunnel_daemon().await;
    let (_, error, result) = add_quick(&d, "blog").await;
    assert_eq!(error, None);
    let origin = result["origin_port"].as_u64().unwrap();
    let mut c = d.client().await;
    let blocks = c.call("env.print", json!({"site": "blog"})).await.unwrap();
    let tunnel = blocks.as_array().unwrap().last().cloned();
    assert_eq!(
        tunnel,
        Some(json!({
            "instance": "tunnel",
            "lines": [format!("APP_URL=https://quick-{origin}.trycloudflare.com")],
            "note": "while blog is shared through a tunnel; Yard does not write .env",
        })),
        "{blocks}"
    );
    let (_, error, _) = run_job_result(&mut c, "tunnels.remove", json!({"site": "blog"})).await;
    assert_eq!(error, None);
    let blocks = c.call("env.print", json!({"site": "blog"})).await.unwrap();
    assert!(!blocks.as_array().unwrap().iter().any(|b| b["instance"] == "tunnel"), "{blocks}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_quick_tunnel_that_never_connects_leaves_nothing() {
    let d = tunnel_daemon().await;
    let cloudflared = Brew::new(d.fx.brew.path()).cloudflared_bin().parent().unwrap().to_path_buf();
    std::fs::write(cloudflared.join("not-ready"), "").unwrap();
    let (_, error, result) = add_quick(&d, "blog").await;
    let error = error.expect("the job fails");
    assert_eq!(error.code, ErrorCode::TunnelNotReady, "{error:?}");
    assert_eq!(error.message, "tunnel-blog did not connect to Cloudflare within 2 s");
    let log = d.fx.paths.logs_dir().join("tunnel-blog.log");
    assert_eq!(error.hint.as_deref(), Some(format!("tail -n 50 {}", log.display()).as_str()));
    assert!(error.data.as_ref().is_some_and(|d| d["tail"].is_array()), "{error:?}");
    assert_eq!(result, Value::Null);
    let mut c = d.client().await;
    assert_eq!(c.call("tunnels.list", Value::Null).await.unwrap(), json!([]));
    assert!(tunnel_entries(&d).is_empty(), "{:?}", tunnel_entries(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn quick_tunnels_end_with_yardd() {
    let mut d = quick_daemon().await;
    d.stop().await.unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    assert_eq!(c.call("tunnels.list", Value::Null).await.unwrap(), json!([]));
    assert!(tunnel_entries(&d).is_empty(), "{:?}", tunnel_entries(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn untunnel_without_a_tunnel_is_not_found() {
    let d = tunnel_daemon().await;
    let mut c = d.client().await;
    let e = c.call("tunnels.remove", json!({"site": "blog"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::TunnelNotFound);
    assert_eq!(e.hint.as_deref(), Some("yard tunnel blog"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn add_refuses_a_second_tunnel_and_an_unknown_provider() {
    let d = quick_daemon().await;
    let mut c = d.client().await;
    let e = c.call("tunnels.add", json!({"site": "blog"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    assert_eq!(e.hint.as_deref(), Some("yard untunnel blog"));
    let e = c.call("tunnels.add", json!({"site": "blog", "provider": "ngrok"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    assert_eq!(e.message, "no tunnel provider ngrok; the providers are: cloudflared");
    let e = c.call("tunnels.add", json!({"site": "nope"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::SiteNotFound);
    d.project("shop");
    // Not installed: no keg at all (an empty keg is `brew reinstall`).
    std::fs::remove_dir_all(d.fx.brew.path().join("opt/cloudflared")).unwrap();
    // `shop` resolves once the watcher has reconciled its link.
    let deadline = Instant::now() + Duration::from_secs(10);
    let e = loop {
        let e = c.call("tunnels.add", json!({"site": "shop"})).await.unwrap_err();
        if e.code != ErrorCode::SiteNotFound || Instant::now() > deadline {
            break e;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(e.code, ErrorCode::FormulaMissing, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some("brew install cloudflared"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unlink_ends_a_quick_tunnel() {
    let d = quick_daemon().await;
    let mut c = d.client().await;
    c.call("sites.unlink", json!({"name": "blog"})).await.unwrap();
    assert_eq!(c.call("tunnels.list", Value::Null).await.unwrap(), json!([]));
    assert!(tunnel_entries(&d).is_empty(), "{:?}", tunnel_entries(&d));
    // Linking it again does not bring the tunnel back.
    d.project("blog");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(c.call("tunnels.list", Value::Null).await.unwrap(), json!([]));
    d.shutdown().await.unwrap();
}

fn cloudflared_dir(d: &Daemon) -> PathBuf {
    Brew::new(d.fx.brew.path()).cloudflared_bin().parent().unwrap().to_path_buf()
}

fn calls(d: &Daemon) -> String {
    std::fs::read_to_string(cloudflared_dir(d).join("calls")).unwrap_or_default()
}

#[tokio::test]
async fn domain_add_signs_in_and_records_the_domain() {
    let d = tunnel_daemon().await;
    let mut c = d.client().await;
    let (lines, error, result) =
        run_job_result(&mut c, "tunnels.domain_add", json!({"domain": "example.com"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "In the browser, pick the zone example.com."), "{lines:?}");
    assert!(calls(&d).lines().any(|l| l == "tunnel login"), "{}", calls(&d));
    assert!(cert_path(&d.opts.home, "example.com").exists());
    assert!(d.opts.home.join(".cloudflared/yard/example.com/.cloudflared/cert.pem").exists());
    let yard = std::fs::read_to_string(d.fx.paths.yard_toml()).unwrap();
    assert!(yard.contains("[tunnels.cloudflared]") && yard.contains("\"example.com\""), "{yard}");
    let want = json!({"provider": "cloudflared", "domain": "example.com", "logged_in": true});
    assert_eq!(result, want);
    assert_eq!(c.call("tunnels.domains", Value::Null).await.unwrap(), json!([want]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn domain_add_with_a_certificate_skips_login() {
    let d = tunnel_daemon().await;
    let cert = cert_path(&d.opts.home, "example.com");
    std::fs::create_dir_all(cert.parent().unwrap()).unwrap();
    std::fs::write(&cert, "mine").unwrap();
    let mut c = d.client().await;
    let (lines, error, _) =
        run_job_result(&mut c, "tunnels.domain_add", json!({"domain": "example.com"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "Already signed in for example.com."), "{lines:?}");
    assert!(!calls(&d).contains("login"), "{}", calls(&d));
    assert_eq!(std::fs::read_to_string(&cert).unwrap(), "mine");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_login_records_nothing() {
    let d = tunnel_daemon().await;
    std::fs::write(cloudflared_dir(&d).join("login-fails"), "").unwrap();
    let mut c = d.client().await;
    let (_, error, result) =
        run_job_result(&mut c, "tunnels.domain_add", json!({"domain": "example.com"})).await;
    assert!(error.is_some());
    assert_eq!(result, Value::Null);
    let yard = std::fs::read_to_string(d.fx.paths.yard_toml()).unwrap_or_default();
    assert!(!yard.contains("example.com"), "{yard}");
    assert_eq!(c.call("tunnels.domains", Value::Null).await.unwrap(), json!([]));
    let e = c.call("tunnels.domain_add", json!({"domain": "Not A Domain"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    let e = c
        .call("tunnels.domain_add", json!({"domain": "example.com", "provider": "ngrok"}))
        .await
        .unwrap_err();
    assert_eq!(e.message, "no tunnel provider ngrok; the providers are: cloudflared");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn domain_remove_forgets_only_the_domain() {
    let d = tunnel_daemon().await;
    std::fs::write(
        d.fx.paths.yard_toml(),
        "[tunnels.cloudflared]\n# the zones I own\ndomains = [\"example.com\", \"example.org\"]\n",
    )
    .unwrap();
    for domain in ["example.com", "example.org"] {
        let cert = cert_path(&d.opts.home, domain);
        std::fs::create_dir_all(cert.parent().unwrap()).unwrap();
        std::fs::write(&cert, "x").unwrap();
    }
    let mut c = d.client().await;
    let left = c.call("tunnels.domain_remove", json!({"domain": "example.com"})).await.unwrap();
    assert_eq!(left, json!([{"provider": "cloudflared", "domain": "example.org", "logged_in": true}]));
    let yard = std::fs::read_to_string(d.fx.paths.yard_toml()).unwrap();
    assert!(yard.contains("# the zones I own"), "{yard}");
    assert!(!yard.contains("example.com"), "{yard}");
    assert!(cert_path(&d.opts.home, "example.com").exists(), "the certificate stays");
    let e = c.call("tunnels.domain_remove", json!({"domain": "example.com"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    assert_eq!(e.hint.as_deref(), Some("yard tunnels domains"));
    d.shutdown().await.unwrap();
}

/// A started daemon with `blog` and `shop` linked and `example.com` set up and signed in.
async fn named_daemon() -> Daemon {
    let d = prepared("").await;
    d.fx.add_cloudflared();
    let ports = std::fs::read_to_string(d.fx.paths.ports_toml()).unwrap();
    std::fs::write(d.fx.paths.ports_toml(), format!("{}{ports}", auto_range())).unwrap();
    std::fs::write(d.fx.paths.yard_toml(), "[tunnels.cloudflared]\ndomains = [\"example.com\"]\n").unwrap();
    let cert = cert_path(&d.opts.home, "example.com");
    std::fs::create_dir_all(cert.parent().unwrap()).unwrap();
    std::fs::write(&cert, "fake").unwrap();
    d.project("blog");
    d.project("shop");
    d.start().await
}

async fn add_named(d: &Daemon, params: Value) -> (Vec<String>, Option<YardError>, Value) {
    let mut c = d.client().await;
    run_job_result(&mut c, "tunnels.add", params).await
}

async fn untunnel(d: &Daemon, site: &str) -> (Vec<String>, Option<YardError>, Value) {
    let mut c = d.client().await;
    run_job_result(&mut c, "tunnels.remove", json!({"site": site})).await
}

/// The fake's management calls, without `tunnel`, `--origincert <cert>` or the unit's runs.
fn managed_calls(d: &Daemon) -> Vec<String> {
    calls(d)
        .lines()
        .filter(|l| !l.starts_with("tunnel --config"))
        .map(|l| {
            let words: Vec<&str> = l.split(' ').collect();
            let mut kept = vec![];
            let mut skip = false;
            for w in words.into_iter().skip(1) {
                if skip {
                    skip = false;
                } else if w == "--origincert" {
                    skip = true;
                } else {
                    kept.push(w);
                }
            }
            kept.join(" ")
        })
        .collect()
}

fn facts(d: &Daemon) -> Facts {
    Facts::load(&d.fx.paths.tunnels_state()).unwrap()
}

fn sites_toml(d: &Daemon) -> String {
    std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap()
}

#[tokio::test]
async fn named_tunnel_creates_routes_and_runs() {
    let d = named_daemon().await;
    let (lines, error, result) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "Creating the Cloudflare tunnel yard-blog…"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "Routing blog.example.com to it…"), "{lines:?}");
    let credentials = created_credentials(&d.opts.home, "example.com", "yard-blog");
    assert_eq!(
        managed_calls(&d),
        [
            "list --name yard-blog -o json".to_string(),
            format!("create --credentials-file {} yard-blog", credentials.display()),
            "list --name yard-blog -o json".to_string(),
            "route dns id-yard-blog blog.example.com".to_string(),
        ]
    );
    assert_eq!(result["url"], "https://blog.example.com");
    assert_eq!(result["state"], "ready");
    assert_eq!(result["tunnel"], json!({"name": "yard-blog", "id": "id-yard-blog", "created_by_yard": true}));
    assert!(
        sites_toml(&d).contains("[sites.blog.tunnel]\nhost = \"blog.example.com\""),
        "{}",
        sites_toml(&d)
    );
    let f = facts(&d).named["blog"].clone();
    assert_eq!(
        f,
        NamedFacts {
            id: "id-yard-blog".into(),
            name: "yard-blog".into(),
            created_by_yard: true,
            routed: vec!["blog.example.com".into()],
            credentials,
            domain: "example.com".into(),
        }
    );
    // Another site cannot take the same host.
    let mut c = d.client().await;
    let e = c.call("tunnels.add", json!({"site": "shop", "host": "blog.example.com"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument, "{e:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn named_tunnel_refuses_a_foreign_yard_tunnel() {
    let d = named_daemon().await;
    std::fs::write(cloudflared_dir(&d).join("tunnels.json"), r#"[{"id":"x1","name":"yard-blog"}]"#).unwrap();
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    let e = error.expect("refused");
    assert_eq!(e.code, ErrorCode::TunnelExists, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some("yard tunnel blog --host blog.example.com --tunnel yard-blog"));
    assert!(!sites_toml(&d).contains("tunnel"), "{}", sites_toml(&d));
    assert!(!d.fx.paths.tunnels_state().exists());
    assert!(!managed_calls(&d).iter().any(|c| c.starts_with("create") || c.starts_with("delete")));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn adopting_with_tunnel_never_deletes() {
    let d = named_daemon().await;
    std::fs::write(cloudflared_dir(&d).join("tunnels.json"), r#"[{"id":"shared-id","name":"blog-shared"}]"#)
        .unwrap();
    let credentials = default_credentials(&d.opts.home, "shared-id");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    std::fs::write(&credentials, "{}").unwrap();
    let (lines, error, _) =
        add_named(&d, json!({"site": "blog", "host": "blog.example.com", "tunnel": "blog-shared"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(sites_toml(&d).contains("tunnel = \"blog-shared\""), "{}", sites_toml(&d));
    let f = facts(&d).named["blog"].clone();
    assert!(!f.created_by_yard);
    assert_eq!(f.credentials, credentials);

    let (_, error, removed) = untunnel(&d, "blog").await;
    assert_eq!(error, None);
    assert!(!managed_calls(&d).iter().any(|c| c.starts_with("delete") || c.starts_with("cleanup")));
    assert_eq!(
        removed,
        json!({"site": "blog", "deleted_tunnel": false,
               "dns_left": [{"host": "blog.example.com", "target": "shared-id.cfargotunnel.com"}]})
    );
    assert!(!facts(&d).named.contains_key("blog"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_existing_record_is_dns_exists_and_overwrite_passes_f() {
    let d = named_daemon().await;
    std::fs::write(cloudflared_dir(&d).join("dns-records"), "blog.example.com\n").unwrap();
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    let e = error.expect("refused");
    assert_eq!(e.code, ErrorCode::TunnelDnsExists, "{e:?}");
    assert_eq!(e.hint.as_deref(), Some("yard tunnel blog --host blog.example.com --overwrite-dns"));
    assert!(managed_calls(&d).contains(&"delete id-yard-blog".to_string()), "{:?}", managed_calls(&d));
    assert!(!facts(&d).named.contains_key("blog"));
    assert!(facts(&d).dns_left.is_empty());
    assert!(!sites_toml(&d).contains("tunnel"));

    let (lines, error, _) =
        add_named(&d, json!({"site": "blog", "host": "blog.example.com", "overwrite_dns": true})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(managed_calls(&d).contains(&"route dns -f id-yard-blog blog.example.com".to_string()));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_foreign_zone_deletes_the_new_tunnel() {
    let d = named_daemon().await;
    std::fs::write(cloudflared_dir(&d).join("foreign-zones"), "example.com\n").unwrap();
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    let e = error.expect("refused");
    assert_eq!(e.code, ErrorCode::TunnelDnsFailed, "{e:?}");
    assert_eq!(
        e.hint.as_deref(),
        Some("check that example.com is in the Cloudflare account you signed in to")
    );
    assert!(
        e.data.as_ref().is_some_and(|d| d["output"].as_str().is_some_and(|o| o.contains("zone not found")))
    );
    assert!(managed_calls(&d).contains(&"delete id-yard-blog".to_string()));
    assert!(!facts(&d).named.contains_key("blog"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn untunnel_deletes_and_leaves_the_record() {
    let d = named_daemon().await;
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    assert_eq!(error, None);
    let (_, error, removed) = untunnel(&d, "blog").await;
    assert_eq!(error, None);
    assert_eq!(
        removed,
        json!({"site": "blog", "deleted_tunnel": true,
               "dns_left": [{"host": "blog.example.com", "target": "id-yard-blog.cfargotunnel.com"}]})
    );
    let calls = managed_calls(&d);
    let cleanup = calls.iter().position(|c| c == "cleanup id-yard-blog").expect("cleanup");
    let delete = calls.iter().position(|c| c == "delete id-yard-blog").expect("delete");
    assert!(cleanup < delete);
    let f = facts(&d);
    assert!(!f.named.contains_key("blog"));
    assert_eq!(f.dns_left.len(), 1);
    assert_eq!(f.dns_left[0].host, "blog.example.com");
    assert_eq!(f.dns_left[0].since.len(), 10, "{:?}", f.dns_left[0]);
    assert!(!sites_toml(&d).contains("tunnel"));
    let mut c = d.client().await;
    assert_eq!(c.call("tunnels.list", Value::Null).await.unwrap(), json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_delete_keeps_the_id() {
    let d = named_daemon().await;
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    assert_eq!(error, None);
    std::fs::write(cloudflared_dir(&d).join("delete-fails"), "").unwrap();
    let (lines, error, removed) = untunnel(&d, "blog").await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("warning:")), "{lines:?}");
    assert_eq!(removed["deleted_tunnel"], false);
    let f = facts(&d);
    assert_eq!(f.named["blog"].id, "id-yard-blog");
    assert!(f.named["blog"].routed.is_empty());
    assert_eq!(f.dns_left.len(), 1);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn named_tunnel_returns_after_restart() {
    let mut d = named_daemon().await;
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    assert_eq!(error, None);
    d.stop().await.unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let t = tunnel_where(&mut c, |t| t["state"] == "ready").await;
    assert_eq!(t["kind"], "named");
    assert_eq!(t["url"], "https://blog.example.com");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unlink_refuses_a_named_tunnel() {
    let d = named_daemon().await;
    let (_, error, _) = add_named(&d, json!({"site": "blog", "host": "blog.example.com"})).await;
    assert_eq!(error, None);
    let mut c = d.client().await;
    let e = c.call("sites.unlink", json!({"name": "blog"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::SiteTunnelled);
    assert_eq!(e.message, "blog is shared through a named tunnel");
    assert_eq!(e.hint.as_deref(), Some("yard untunnel blog"));
    assert!(sites_toml(&d).contains("[sites.blog]"));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_outside_every_domain_is_not_set_up() {
    let d = named_daemon().await;
    let mut c = d.client().await;
    let e = c.call("tunnels.add", json!({"site": "blog", "host": "blog.other.net"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::TunnelDomainNotSetUp);
    assert_eq!(e.hint.as_deref(), Some("yard tunnels domain add other.net"));
    let e = c.call("tunnels.add", json!({"site": "blog", "host": "Not A Host"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_certificate_is_not_logged_in() {
    let d = named_daemon().await;
    std::fs::remove_file(cert_path(&d.opts.home, "example.com")).unwrap();
    let mut c = d.client().await;
    let e = c.call("tunnels.add", json!({"site": "blog", "host": "blog.example.com"})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::TunnelNotLoggedIn);
    assert_eq!(e.hint.as_deref(), Some("yard tunnels domain add example.com"));
    d.shutdown().await.unwrap();
}
