//! Smoke tests for the Sites screen.

use crate::env::AppEnv;
use crate::testing::{env, site};
use gpui::{Context, IntoElement, Render, TestAppContext, Window};
use serde_json::json;
use yard_app_model::hint::Screen;

struct Host(AppEnv);

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::sites(&self.0, window, cx)
    }
}

fn detail(php_source: &str) -> yard_proto::SiteDetail {
    serde_json::from_value(json!({
        "site": {
            "name": "blog", "host": "blog.yard", "url": "http://blog.yard:8080/", "source": "linked",
            "kind": "php", "path": "/p", "docroot": "/p/public", "php": "8.3", "project_file": "/p/.yard",
            "project_file_kind": "project"
        },
        "php_full": "8.3.21",
        "sources": {"php": php_source, "docroot": "default", "secure": "sites_toml"},
        "services": [{"engine": "mysql", "want": "8.4", "service": null}]
    }))
    .unwrap()
}

#[gpui::test]
async fn sites_list_renders(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| {
        s.sites.value = serde_json::from_value(json!([
            site("blog", json!([])),
            site("legacy", json!([{"kind": "php", "php": "7.1"}]))
        ]))
        .ok();
    });
    let (_v, cx) = cx.add_window_view(|_, _| Host(env));
    cx.run_until_parked();
}

#[gpui::test]
async fn site_detail_renders_sources(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| {
        s.sites.value = serde_json::from_value(json!([site("blog", json!([]))])).ok();
        s.site_detail.entry("blog".into()).or_default().value = Some(detail("managed_file"));
    });
    env.nav.update(cx, |n, _| n.screen = Screen::Sites(Some("blog".into())));
    let lines = cx.update(|cx| {
        super::detail::source_lines(&env.state.read(cx).site_detail["blog"].value.clone().unwrap())
    });
    assert!(lines.contains(&"PHP 8.3.21 — from the managed .yard".to_string()), "{lines:?}");
    assert!(lines.contains(&"HTTPS off — from sites.toml".to_string()), "{lines:?}");
    let (_v, cx) = cx.add_window_view(|_, _| Host(env));
    cx.run_until_parked();
}

#[test]
fn php_picker_is_disabled_for_a_committed_yard() {
    assert_eq!(super::detail::php_lock(&detail("project_file")), Some("set in /p/.yard".to_string()));
    assert_eq!(super::detail::php_lock(&detail("managed_file")), None, "a managed file is edited for you");
    assert_eq!(super::detail::php_lock(&detail("sites_toml")), None);
}
