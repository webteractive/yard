//! Smoke tests: each screen draws with sample data.

use crate::env::AppEnv;
use crate::testing::env;
use gpui::{AnyElement, App, Context, IntoElement, Render, TestAppContext, Window};
use serde_json::json;

type Screen = fn(&AppEnv, &mut Window, &mut App) -> AnyElement;

struct Host(AppEnv, Screen);

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.1)(&self.0, window, cx)
    }
}

fn sample(env: &AppEnv, cx: &mut TestAppContext) {
    env.state.update(cx, |s, _| {
        s.php.value = serde_json::from_value(json!([
            {"version": "7.0", "default": false, "installed": false, "sites": [], "pool": null},
            {"version": "8.4", "default": true, "installed": true, "sites": ["blog"], "pool": "running"}
        ]))
        .ok();
        s.extensions.entry("8.4".into()).or_default().value =
            serde_json::from_value(json!([{"name": "xdebug", "source": "pecl", "enabled": false}])).ok();
        s.node.value = serde_json::from_value(json!({"default": "22", "versions": [{"version": "22.18.0", "default": true, "sites": ["blog"]}]})).ok();
        s.tunnels.value = serde_json::from_value(json!([{
            "site": "blog", "provider": "cloudflare", "kind": "quick", "url": "https://x.trycloudflare.com", "host": null,
            "tunnel": null, "state": "ready", "unit": "tunnel-blog", "origin_port": 33001, "metrics_port": 33002, "last_error": null
        }]))
        .ok();
        s.tunnel_domains.value = serde_json::from_value(json!([{"provider": "cloudflare", "domain": "example.com", "logged_in": true}])).ok();
        s.ports.value = serde_json::from_value(json!([
            {"unit": "nginx-http", "port": 8080, "mode": "pinned", "preferred": null, "status": "yard", "holder": null},
            {"unit": "redis", "port": 6379, "mode": "pinned", "preferred": null, "status": "in_use",
             "holder": {"pid": 9, "name": "redis-server", "path": "/Users/Shared/DBngin/redis", "ppid": 1, "kind": "dbngin"}}
        ]))
        .ok();
        s.doctor.value = serde_json::from_value(json!({"checks": [
            {"id": "units", "status": "ok", "message": "no unit has failed"},
            {"id": "helper", "status": "fail", "message": "the helper is not installed", "hint": "sudo yard helper install"},
            {"id": "brew-ini:7.4/20-redis.ini", "status": "warn", "message": "a stale ini", "hint": "rm /opt/homebrew/etc/php/7.4/conf.d/20-redis.ini"}
        ]}))
        .ok();
        s.sources.value = serde_json::from_value(json!([{"tap": "shivammathur/php", "tapped": true, "trusted": true, "formulae": []}])).ok();
    });
}

async fn draws(cx: &mut TestAppContext, screen: Screen) {
    let (_fake, env) = env(cx);
    sample(&env, cx);
    let (_v, cx) = cx.add_window_view(|_, _| Host(env, screen));
    cx.run_until_parked();
}

#[gpui::test]
async fn php_renders(cx: &mut TestAppContext) {
    draws(cx, super::php::php).await;
}

#[gpui::test]
async fn node_renders(cx: &mut TestAppContext) {
    draws(cx, super::node::node).await;
}

#[gpui::test]
async fn tunnels_render(cx: &mut TestAppContext) {
    draws(cx, super::tunnels::tunnels).await;
}

#[gpui::test]
async fn ports_render(cx: &mut TestAppContext) {
    draws(cx, super::ports::ports).await;
}

#[gpui::test]
async fn logs_render(cx: &mut TestAppContext) {
    draws(cx, super::logs::logs).await;
}

#[gpui::test]
async fn doctor_renders(cx: &mut TestAppContext) {
    draws(cx, super::doctor::doctor).await;
}

#[gpui::test]
async fn sources_render(cx: &mut TestAppContext) {
    draws(cx, super::sources::sources).await;
}

#[test]
fn doctor_checks_group_by_status_with_hint_buttons() {
    let report: yard_proto::DoctorReport = serde_json::from_value(json!({"checks": [
        {"id": "units", "status": "ok", "message": "no unit has failed"},
        {"id": "helper", "status": "fail", "message": "the helper is not installed", "hint": "sudo yard helper install"},
        {"id": "ini", "status": "warn", "message": "a stale ini", "hint": "rm /x/20-redis.ini"},
        {"id": "future", "status": "skipped", "message": "a status this app does not know"}
    ]}))
    .unwrap();
    let groups = super::doctor::groups(&report);
    let names: Vec<&str> = groups.iter().map(|g| g.0).collect();
    // A check of a status a newer yardd added is shown under Other, not dropped (M6 final review).
    assert_eq!(names, ["Failing", "Warnings", "OK", "Other"]);
    assert_eq!(groups[0].1[0].1, ["Open in Terminal", "Copy"], "a sudo hint offers Terminal and Copy");
    assert_eq!(groups[1].1[0].1, ["Copy"], "an rm hint is only copied");
}

#[test]
fn a_job_line_with_a_url_is_a_link() {
    use crate::widgets::log_view::link_in;
    assert_eq!(
        link_in("Your quick tunnel: https://a-b.trycloudflare.com ready"),
        Some("https://a-b.trycloudflare.com")
    );
    assert_eq!(link_in("open http://blog.yard:8080/"), Some("http://blog.yard:8080/"));
    assert_eq!(link_in("==> Pouring php@8.4"), None);
}

/// Every column has a title for VoiceOver to announce (found by the checklist script: the PHP and
/// Node tables' default column was blank).
#[test]
fn every_table_column_has_a_title() {
    for columns in [
        super::php::columns(),
        super::node::columns(),
        super::ports::columns(),
        super::tunnels::columns(),
        super::sources::columns(),
    ] {
        assert!(columns.iter().all(|c| !c.title.trim().is_empty()), "{columns:?}");
    }
}

/// A failed `ext.list` is not asked again each time the PHP screen draws (M6 final review: with
/// yardd stopped the screen asked about six times a second).
#[gpui::test]
async fn a_failed_extension_list_is_not_refetched_by_drawing(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    fake.fail("ext.list", yard_proto::YardError::internal("yardd is not answering"));
    env.state.update(cx, |s, _| {
        s.php.value = serde_json::from_value(json!([
            {"version": "8.4", "default": true, "installed": true, "sites": [], "pool": "running"}
        ]))
        .ok();
    });
    let state = env.state.clone();
    let (_v, cx) = cx.add_window_view(|_, cx| {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Host(env, super::php::php)
    });
    for _ in 0..10 {
        cx.run_until_parked();
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
    }
    assert!(fake.count("ext.list") <= 1, "asked {} times", fake.count("ext.list"));
}

/// Add extension… takes a `.so` chosen in an open panel, sent absolute (design §8.2 PHP; M6 final
/// review: only a typed path worked).
#[gpui::test]
async fn choose_so_puts_the_chosen_path_in_the_add_field(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    cx.update(|cx| super::php::choose_so(&env, cx));
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths(), "an open panel");
    cx.simulate_path_prompt_response(|_| Some(vec![std::path::PathBuf::from("/opt/ext/custom.so")]));
    cx.run_until_parked();
    let field = cx.update(|cx| crate::env::Nav::field(&env, "php:ext:add", "Extension to add", "", cx));
    assert_eq!(cx.read(|cx| field.read(cx).value().to_string()), "/opt/ext/custom.so");
}

/// Sources trusts or untrusts a whole tap or one formula of it (design §8.2; M6 final review: only
/// taps).
#[test]
fn sources_offer_a_tap_and_each_formula() {
    let tap: yard_proto::SourceInfo = serde_json::from_value(json!({
        "tap": "minio/stable", "tapped": true, "trusted": false,
        "formulae": [{"name": "minio", "trusted": true, "installed": true}, {"name": "mc", "trusted": false, "installed": false}]
    }))
    .unwrap();
    let targets: Vec<(String, &str, String, bool)> = super::sources::targets(&tap)
        .into_iter()
        .map(|t| (t.key, t.verb, t.params.target, t.params.tap))
        .collect();
    assert_eq!(
        targets,
        [
            ("minio/stable".to_string(), "Trust", "minio/stable".to_string(), true),
            ("minio/stable/minio".to_string(), "Untrust", "minio/stable/minio".to_string(), false),
            ("minio/stable/mc".to_string(), "Trust", "minio/stable/mc".to_string(), false),
        ]
    );
}

/// Ports' Range… sets the auto range (`ports.range`; design §8.2; M6 final review: missing).
#[gpui::test]
async fn range_sets_the_auto_range(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    fake.answer("ports.range", json!(null));
    cx.update(|cx| super::ports::set_range(&env, "29000-29099".into(), cx));
    cx.run_until_parked();
    let asked: Vec<serde_json::Value> =
        fake.calls().into_iter().filter(|(m, _)| m == "ports.range").map(|(_, p)| p).collect();
    assert_eq!(asked, [json!({"range": "29000-29099"})]);
}

/// Stopping a named tunnel can leave its DNS record in Cloudflare: Tunnels says which to delete,
/// as `yard untunnel` does (design §8.2; M6 final review: `dns_left` was never shown).
#[gpui::test]
async fn stop_sharing_says_which_dns_records_to_delete(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    fake.answer("tunnels.remove", crate::testing::job("tunnel-remove-1"));
    cx.update(|cx| super::tunnels::stop_sharing(&env, "blog".into(), false, cx));
    cx.run_until_parked();
    let remove = fake.calls().into_iter().find(|(m, _)| m == "tunnels.remove").unwrap().1;
    assert_eq!(remove, json!({"site": "blog", "keep_tunnel": false}));
    fake.event(yard_proto::Event::JobFinished {
        job_id: "tunnel-remove-1".into(),
        error: None,
        result: Some(json!({"site": "blog", "deleted_tunnel": true,
            "dns_left": [{"host": "blog.example.com", "target": "abc.cfargotunnel.com"}]})),
    });
    cx.run_until_parked();
    let note = env
        .runner
        .read_with(cx, |r, _| r.errors.get(&crate::runner::ActionId("tunnel:blog:dns".into())).cloned())
        .expect("a note");
    assert!(
        note.message.contains("Delete the CNAME blog.example.com (→ abc.cfargotunnel.com)"),
        "{}",
        note.message
    );
}

/// A named share whose host already has a DNS record asks whether to overwrite it, then shares
/// again with `overwrite_dns` (design §8.2 Tunnels).
#[gpui::test]
async fn a_named_share_asks_to_overwrite_a_dns_record(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    fake.answers(
        "tunnels.add",
        vec![Ok(crate::testing::job("tunnel-add-1")), Ok(crate::testing::job("tunnel-add-2"))],
    );
    let (_v, cx) = cx.add_window_view(|_, _| Blank);
    cx.update(|window, cx| {
        super::tunnels::share(&env, "blog".into(), Some("blog.example.com".into()), false, window, cx)
    });
    cx.run_until_parked();
    let exists = yard_proto::YardError::new(
        yard_proto::ErrorCode::TunnelDnsExists,
        "blog.example.com has a DNS record",
    );
    fake.event(crate::testing::finished("tunnel-add-1", Some(exists)));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "asks whether to overwrite");
    cx.simulate_prompt_answer("Overwrite");
    cx.run_until_parked();
    let overwrites: Vec<bool> = fake
        .calls()
        .into_iter()
        .filter(|(m, _)| m == "tunnels.add")
        .map(|(_, p)| p["overwrite_dns"].as_bool().unwrap())
        .collect();
    assert_eq!(overwrites, [false, true]);
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

/// The files the screens open are named by yard-core's `Paths`, as yardd writes them (M6 design
/// §5.8; the final review found them spelled out by hand).
#[gpui::test]
async fn opened_files_are_yard_cores_paths(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    let paths = env.paths();
    let v84 = "8.4".parse().unwrap();
    assert_eq!(super::php::overrides_file(&env, "8.4"), Some(paths.php_overrides(v84)));
    assert_eq!(super::logs::log_file(&paths, Some("nginx")), paths.log("nginx"));
    assert_eq!(super::logs::log_file(&paths, None), paths.logs_dir());
}

/// Check now shows what the check found: its answer is the port list (M6 final review: the
/// answer was dropped and the list refetched before the check had run).
#[gpui::test]
async fn check_now_shows_the_checks_answer(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    fake.answer(
        "ports.check",
        json!([
            {"unit": "nginx-http", "port": 8080, "mode": "pinned", "preferred": null, "status": "in_use",
             "holder": {"pid": 9, "name": "nginx", "path": "/x/nginx", "ppid": 1, "kind": "herd"}}
        ]),
    );
    cx.update(|cx| super::ports::check_now(&env, cx));
    cx.run_until_parked();
    let status = env.state.read_with(cx, |s, _| s.ports.value.as_ref().map(|p| p[0].status));
    assert_eq!(status, Some(yard_proto::PortStatus::InUse));
    assert_eq!(fake.count("ports.list"), 0, "nothing refetched over it");
}

/// Make default and Uninstall… are the selected version's, not repeated on every row (spec §4,
/// PHP: 22 buttons, 11 of them red); a version that is not installed keeps Install on its row.
#[gpui::test]
async fn php_actions_live_in_the_selected_version(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| {
        s.php.value = serde_json::from_value(json!([
            {"version": "7.0", "default": false, "installed": false, "sites": [], "pool": null},
            {"version": "7.1", "default": false, "installed": true, "sites": [], "pool": null},
            {"version": "8.4", "default": true, "installed": true, "sites": ["blog"], "pool": "running"}
        ]))
        .ok();
    });
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone(), super::php::php));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("php:8.4:uninstall").is_some(), "the default version is shown first");
    assert!(vcx.debug_bounds("php:7.1:uninstall").is_none(), "7.1 is not selected");
    assert!(vcx.debug_bounds("php:7.0:install").is_some(), "Install stays on its row");
    vcx.update(|_, cx| {
        env.nav.update(cx, |n, cx| {
            n.php_version = Some("7.1".into());
            cx.notify();
        })
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("php:7.1:uninstall").is_some() && vcx.debug_bounds("php:7.1:default").is_some());
    assert!(vcx.debug_bounds("php:8.4:uninstall").is_none());
}

/// Before its first run, Doctor says how to run it, and while it runs, that it is checking,
/// rather than showing a blank page.
#[gpui::test]
async fn doctor_says_what_it_is_doing_before_a_report(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    let e = env.clone();
    let (v, cx) = cx.add_window_view(|_, _| Host(e, super::doctor::doctor));
    cx.run_until_parked();
    assert!(cx.debug_bounds("doctor:empty").is_some(), "says to run it");
    env.state.update(cx, |s, _| s.doctor.loading = true);
    v.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(cx.debug_bounds("doctor:checking").is_some(), "says it is checking");
}
