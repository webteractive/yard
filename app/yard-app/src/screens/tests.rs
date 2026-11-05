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
        {"id": "ini", "status": "warn", "message": "a stale ini", "hint": "rm /x/20-redis.ini"}
    ]}))
    .unwrap();
    let groups = super::doctor::groups(&report);
    let names: Vec<&str> = groups.iter().map(|g| g.0).collect();
    assert_eq!(names, ["Failing", "Warnings", "OK"]);
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
