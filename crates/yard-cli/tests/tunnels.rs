//! `yard tunnel`, `untunnel`, `tunnels` and `tunnels domain(s)` (M5).

mod support;

use serde_json::{Value, json};
use support::{MockDaemon, stderr, stdout};
use yard_proto::{SiteInfo, SiteKind, SiteSource};

fn sites() -> Value {
    json!([serde_json::to_value(SiteInfo {
        name: "blog".into(),
        host: "blog.yard".into(),
        url: "http://blog.yard:8080/".into(),
        source: SiteSource::Linked,
        kind: SiteKind::Php { path: "/tmp/blog".into(), docroot: "public".into(), php: "8.4".into() },
        secure: false,
        project_file: None,
        project_file_kind: None,
        ignored_managed_file: None,
        needs: vec![],
        project_error: None,
        node: None,
    })
    .unwrap()])
}

fn quick() -> Value {
    json!({
        "site": "blog", "provider": "cloudflared", "kind": "quick",
        "url": "https://example-words-here.trycloudflare.com", "host": null, "tunnel": null,
        "state": "ready", "unit": "tunnel-blog", "origin_port": 41000, "metrics_port": 41001,
        "last_error": null,
    })
}

fn named(host: &str) -> Value {
    json!({
        "site": "blog", "provider": "cloudflared", "kind": "named",
        "url": format!("https://{host}"), "host": host,
        "tunnel": {"name": "yard-blog", "id": "1b2c", "created_by_yard": true},
        "state": "ready", "unit": "tunnel-blog", "origin_port": 41000, "metrics_port": 41001,
        "last_error": null,
    })
}

fn domains(names: &[&str]) -> Value {
    names.iter().map(|d| json!({"provider": "cloudflared", "domain": d, "logged_in": true})).collect()
}

/// A job's events: its lines, then `job_finished` with `result`.
fn job(id: &str, lines: &[&str], result: Value) -> Vec<Value> {
    let mut events: Vec<Value> =
        lines.iter().map(|l| json!({"type": "job_progress", "job_id": id, "line": l})).collect();
    events.push(json!({"type": "job_finished", "job_id": id, "error": null, "result": result}));
    events
}

fn started(id: &str) -> Result<Value, yard_proto::YardError> {
    Ok(json!({"job_id": id}))
}

#[test]
fn tunnel_quick_prints_the_url() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("subscribe", vec![Ok(json!(null))]),
            ("sites.list", vec![Ok(sites())]),
            ("tunnels.domains", vec![Ok(json!([]))]),
            ("tunnels.add", vec![started("tunnel-add-1")]),
        ],
        &[("tunnels.add", job("tunnel-add-1", &["Starting a quick tunnel for blog…"], quick()))],
    );
    let out = d.yard().args(["tunnel", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Starting a quick tunnel for blog…\n\
         Shared blog → https://example-words-here.trycloudflare.com\n  \
         Anyone with this URL reaches blog until yardd stops or you run yard untunnel.\n  \
         For the app: APP_URL=https://example-words-here.trycloudflare.com\n  \
         Error pages and debug output are public too.\n"
    );
    assert_eq!(
        d.params("tunnels.add"),
        [json!({"site": "blog", "provider": null, "host": null, "tunnel": null, "overwrite_dns": false})]
    );
}

#[test]
fn tunnel_named_prints_the_app_url() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("subscribe", vec![Ok(json!(null))]),
            ("sites.list", vec![Ok(sites())]),
            ("tunnels.domains", vec![Ok(domains(&["example.com"]))]),
            ("tunnels.add", vec![started("tunnel-add-1")]),
        ],
        &[(
            "tunnels.add",
            job(
                "tunnel-add-1",
                &["Creating the Cloudflare tunnel yard-blog…", "Routing a.blog.example.com to it…"],
                named("a.blog.example.com"),
            ),
        )],
    );
    let out = d.yard().args(["tunnel", "blog", "--host", "a.blog.example.com"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Creating the Cloudflare tunnel yard-blog…\n\
         Routing a.blog.example.com to it…\n\
         Shared blog → https://a.blog.example.com (named tunnel yard-blog)\n  \
         It stays up whenever yardd runs, until yard untunnel.\n  \
         For the app: APP_URL=https://a.blog.example.com\n  \
         Error pages and debug output are public too.\n  \
         Cloudflare's free certificate covers example.com and *.example.com only; a.blog.example.com needs an advanced certificate.\n"
    );
    assert_eq!(d.params("tunnels.add")[0]["host"], "a.blog.example.com");
}

#[test]
fn tunnel_without_a_terminal_and_with_domains_exits_2() {
    let d =
        MockDaemon::start(&[("sites.list", Ok(sites())), ("tunnels.domains", Ok(domains(&["example.com"])))]);
    let out = d.yard().args(["tunnel", "blog"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("error: blog can be shared at a random URL or on a domain you set up; say which"),
        "{err}"
    );
    assert!(err.contains("hint: yard tunnel blog --quick, or --host <name>"), "{err}");
    assert!(!d.methods().iter().any(|m| m == "tunnels.add"), "{:?}", d.methods());
}

#[test]
fn tunnel_quick_and_host_conflict() {
    let d = MockDaemon::start(&[]);
    let out = d.yard().args(["tunnel", "blog", "--quick", "--host", "blog.example.com"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(d.methods().is_empty(), "{:?}", d.methods());
}

#[test]
fn untunnel_prints_the_record_to_delete() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("subscribe", vec![Ok(json!(null))]),
            ("tunnels.list", vec![Ok(json!([named("blog.example.com")]))]),
            ("tunnels.remove", vec![started("tunnel-remove-1")]),
        ],
        &[(
            "tunnels.remove",
            job(
                "tunnel-remove-1",
                &["Deleting the Cloudflare tunnel yard-blog…"],
                json!({"site": "blog", "deleted_tunnel": true,
                       "dns_left": [{"host": "blog.example.com", "target": "1b2c.cfargotunnel.com"}]}),
            ),
        )],
    );
    let out = d.yard().args(["untunnel", "blog"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Deleting the Cloudflare tunnel yard-blog…\n\
         Stopped the tunnel for blog and deleted the Cloudflare tunnel yard-blog.\n\
         The DNS record is still there. Delete the CNAME blog.example.com\n\
         (→ 1b2c.cfargotunnel.com) in the Cloudflare dashboard, under the zone's DNS records;\n\
         until then visitors get a Cloudflare error page. yard doctor reminds you until it is gone.\n"
    );
    assert_eq!(d.params("tunnels.remove"), [json!({"site": "blog", "keep_tunnel": false})]);
}

#[test]
fn untunnel_keeping_the_tunnel_says_so() {
    let d = MockDaemon::start_sequenced_with_events(
        &[
            ("subscribe", vec![Ok(json!(null))]),
            ("tunnels.list", vec![Ok(json!([named("blog.example.com")]))]),
            ("tunnels.remove", vec![started("tunnel-remove-1")]),
        ],
        &[(
            "tunnels.remove",
            job("tunnel-remove-1", &[], json!({"site": "blog", "deleted_tunnel": false, "dns_left": []})),
        )],
    );
    let out = d.yard().args(["untunnel", "blog", "--keep-tunnel"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Stopped the tunnel for blog and kept the Cloudflare tunnel yard-blog.\n");
    assert_eq!(d.params("tunnels.remove")[0]["keep_tunnel"], true);
}

#[test]
fn tunnels_lists_and_ends_with_agents_line() {
    let mut shop = quick();
    shop["site"] = json!("shop");
    shop["url"] = json!("https://example-words.trycloudflare.com");
    shop["state"] = json!("connecting");
    let d = MockDaemon::start(&[("tunnels.list", Ok(json!([named("blog.example.com"), shop])))]);
    let out = d.yard().args(["tunnels"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "SITE  URL                                      KIND   STATE       TUNNEL\n\
         blog  https://blog.example.com                 named  ready       yard-blog\n\
         shop  https://example-words.trycloudflare.com  quick  connecting  -\n\
         \n\
         For agents and scripts: yard tunnels --json\n"
    );
}

#[test]
fn tunnels_empty_says_what_to_do() {
    let d = MockDaemon::start(&[("tunnels.list", Ok(json!([])))]);
    let out = d.yard().args(["tunnels"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "No tunnels. Share a site with yard tunnel, run inside it.\n\nFor agents and scripts: yard tunnels --json\n"
    );
}

#[test]
fn tunnels_domains_lists() {
    let mut list = domains(&["example.com", "example.org"]);
    list[1]["logged_in"] = json!(false);
    let d = MockDaemon::start(&[("tunnels.domains", Ok(list))]);
    let out = d.yard().args(["tunnels", "domains"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "DOMAIN       PROVIDER     SIGNED IN\n\
         example.com  cloudflared  yes\n\
         example.org  cloudflared  no\n\
         \n\
         For agents and scripts: yard tunnels domains --json\n"
    );
    let d = MockDaemon::start(&[("tunnels.domains", Ok(json!([])))]);
    let out = d.yard().args(["tunnels", "domains"]).output().unwrap();
    assert!(
        stdout(&out).starts_with("No domains. Set one up with yard tunnels domain add <domain>.\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn domain_add_needs_a_terminal() {
    let d = MockDaemon::start(&[]);
    let out = d.yard().args(["tunnels", "domain", "add", "example.com"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("error: yard tunnels domain add opens a browser to sign in"), "{err}");
    assert!(err.contains("hint: run it in a terminal"), "{err}");
    assert!(d.methods().is_empty(), "{:?}", d.methods());
}

#[test]
fn domain_remove_says_what_stays() {
    let d = MockDaemon::start(&[("tunnels.domain_remove", Ok(json!([])))]);
    let out = d.yard().args(["tunnels", "domain", "remove", "example.com"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Forgot example.com. Tunnels on it keep running; yard tunnel --host can no longer pick it.\n"
    );
    assert_eq!(d.params("tunnels.domain_remove"), [json!({"domain": "example.com"})]);
}

#[test]
fn tunnels_json_is_the_proto_list() {
    let list = json!([named("blog.example.com"), quick()]);
    let d = MockDaemon::start(&[("tunnels.list", Ok(list.clone()))]);
    let out = d.yard().args(["tunnels", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let printed: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(printed, list);
}
