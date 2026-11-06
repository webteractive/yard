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
    detail_of(php_source, "project")
}

fn detail_of(php_source: &str, kind: &str) -> yard_proto::SiteDetail {
    serde_json::from_value(json!({
        "site": {
            "name": "blog", "host": "blog.yard", "url": "http://blog.yard:8080/", "source": "linked",
            "kind": "php", "path": "/p", "docroot": "/p/public", "php": "8.3", "project_file": "/p/.yard",
            "project_file_kind": kind
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

/// yardd refuses `isolate`, `secure` and `docroot` (and `init` finds the file there) for a site
/// whose project has its own `.yard`, whatever keys it sets: the PHP picker, the HTTPS switch,
/// Detect docroot and Save settings to .yard are locked, saying where (found by the checklist
/// script: only the picker was, and only when the file set `php`).
#[test]
fn a_committed_yard_locks_what_yardd_would_refuse() {
    let lock = |d: yard_proto::SiteDetail| super::detail::project_lock(&d.site);
    assert_eq!(lock(detail_of("project_file", "project")), Some("set in /p/.yard".to_string()));
    assert_eq!(
        lock(detail_of("default", "project")),
        Some("set in /p/.yard".to_string()),
        "even without php"
    );
    assert_eq!(lock(detail_of("managed_file", "managed")), None, "a managed file is edited for you");
    let mut none = detail_of("sites_toml", "project");
    (none.site.project_file, none.site.project_file_kind) = (None, None);
    assert_eq!(lock(none), None);
}

/// The detail says which Node a site runs and where that comes from, and offers Use default Node
/// only for a pin Yard made (`yard node use`, in sites.toml or the managed `.yard`); a `.nvmrc` or a
/// committed `.yard` is the project's own (found by the checklist script: no Node line at all).
#[test]
fn the_detail_shows_the_sites_node_and_offers_to_unpin_yards_own_pin() {
    let with_node = |node: serde_json::Value| {
        let mut d = detail_of("default", "managed");
        d.site.node = serde_json::from_value(node).unwrap();
        d
    };
    let pinned = with_node(json!({"spec": "22", "source": "sites_toml", "version": null}));
    let lines = super::detail::source_lines(&pinned);
    assert!(lines.contains(&"Node 22 — not installed, from sites.toml".to_string()), "{lines:?}");
    assert!(super::detail::node_pinned_by_yard(&pinned.site));

    let nvmrc =
        with_node(json!({"spec": "22", "source": "nvmrc", "file": "/p/.nvmrc", "version": "22.11.0"}));
    let lines = super::detail::source_lines(&nvmrc);
    assert!(lines.contains(&"Node 22.11.0 — from /p/.nvmrc".to_string()), "{lines:?}");
    assert!(!super::detail::node_pinned_by_yard(&nvmrc.site));

    assert!(!super::detail::source_lines(&detail("default")).iter().any(|l| l.starts_with("Node")));
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

/// Save settings to .yard asks where to write it (Yard's data root or the project) and, when
/// another directory's site has the name, whether to replace it (design §8.2, §11.3; M6 final
/// review: it always wrote the managed file and stopped at SITE_NAME_TAKEN).
#[gpui::test]
async fn save_settings_asks_where_and_whether_to_replace(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    let taken = yard_proto::YardError::new(yard_proto::ErrorCode::SiteNameTaken, "blog serves /old/blog");
    let written =
        json!({"file": "/p/blog/.yard", "text": "", "written": true, "site": site("blog", json!([]))});
    fake.answers("sites.init", vec![Err(taken), Ok(written)]);
    let (_v, cx) = cx.add_window_view(|_, _| Blank);
    cx.update(|window, cx| super::detail::save_settings(&env, "blog", "/p/blog", window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "asks where");
    cx.simulate_prompt_answer("Write it into the project");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "asks whether to replace");
    cx.simulate_prompt_answer("Replace");
    cx.run_until_parked();
    let inits: Vec<(bool, bool)> = fake
        .calls()
        .into_iter()
        .filter(|(m, _)| m == "sites.init")
        .map(|(_, p)| (p["local"].as_bool().unwrap(), p["replace"].as_bool().unwrap()))
        .collect();
    assert_eq!(inits, [(true, false), (true, true)]);
    let error = env
        .runner
        .read_with(cx, |r, _| r.errors.get(&crate::runner::ActionId("site:blog:init".into())).cloned());
    assert_eq!(error, None, "the replace went through");
}

/// Not now on a settle with a missing Node leaves `yard init`'s note beside the site, its install
/// command as the hint (design §11.1).
#[gpui::test]
async fn not_now_leaves_the_node_note_beside_the_site(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    let s: yard_proto::SiteInfo = serde_json::from_value(site("blog", json!([]))).unwrap();
    let mut s = s;
    s.node = serde_json::from_value(json!({"spec": "22", "source": "project", "version": null})).unwrap();
    let note = yard_app_model::settle::SettlePlan::new(&s).unwrap().not_now.unwrap();
    let id = crate::runner::ActionId("site:blog:node".into());
    cx.update(|cx| super::forms::not_now(&env, id.clone(), note, cx));
    let shown = env.runner.read_with(cx, |r, _| r.errors.get(&id).cloned()).unwrap();
    assert_eq!(shown.message, "blog runs Node 22, which is not installed");
    assert!(crate::sites::DETAIL_VERBS.contains(&"node"), "the Sites detail shows it");
}
