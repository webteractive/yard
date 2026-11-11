//! Smoke tests for the Services screen.

use crate::env::AppEnv;
use crate::runner::ActionId;
use crate::testing::{env, services};
use gpui::{Context, IntoElement, Render, TestAppContext, Window};
use yard_proto::ErrorCode;

struct Host(AppEnv);

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::services(&self.0, window, cx)
    }
}

#[gpui::test]
async fn services_table_renders_every_state(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let rows = cx.update(|cx| super::table::row_texts(&env, cx));
    assert_eq!(rows[0], ["redis", "Redis", "running", "6380", "15m", "yard", "blog"]);
    assert_eq!(rows[1][2], "failed");
    assert_eq!(rows[2][5], "LaunchAgent");
    let (_v, cx) = cx.add_window_view(|_, _| Host(env));
    cx.run_until_parked();
}

#[gpui::test]
async fn remove_is_refused_locally_while_in_use(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let refused = cx.update(|cx| super::table::refuse_in_use(&env, "redis", cx));
    assert!(refused);
    let e = env
        .runner
        .read_with(cx, |r, _| r.errors.get(&ActionId("service:redis:remove".into())).cloned())
        .unwrap();
    assert_eq!(e.code, ErrorCode::ServiceInUse);
    assert_eq!(e.message, "redis is used by blog");
    assert_eq!(fake.count("services.remove"), 0);
    assert!(!cx.update(|cx| super::table::refuse_in_use(&env, "mysql-8.4", cx)), "nobody uses it");
}

/// Remove asks whether to delete the data too (design §11.3; M6 final review: it always deleted).
#[test]
fn remove_asks_whether_to_keep_the_data() {
    let (title, buttons) = super::table::remove_question("postgres-17");
    assert_eq!(title, "Delete postgres-17 and its data?");
    assert_eq!(buttons, ["Delete", "Remove, keep its data", "Cancel"]);
    assert_eq!(super::table::keep_data(0), Some(false));
    assert_eq!(super::table::keep_data(1), Some(true));
    assert_eq!(super::table::keep_data(2), None);
}

/// Copy .env copies what `yard env` prints: an instance that is not added is the one note that
/// says so and how to add it (found by the checklist script: the app copied a bare `# postgres-17`).
#[test]
fn copy_env_matches_yard_env() {
    let blocks: Vec<yard_proto::EnvBlock> = serde_json::from_value(serde_json::json!([
        {"instance": "postgres-17", "lines": [], "note": "postgres-17 is not added: yard service add postgres --version 17"},
        {"instance": "redis", "lines": ["REDIS_HOST=127.0.0.1", "REDIS_PORT=6380"], "note": null}
    ]))
    .unwrap();
    assert_eq!(
        super::table::env_text(&blocks),
        "# postgres-17 is not added: yard service add postgres --version 17\n\n# redis\nREDIS_HOST=127.0.0.1\nREDIS_PORT=6380\n"
    );
}

/// A failed instance says why on the Services screen, holder and fix included, as `yard services`
/// does under its table (found by the checklist script: the row said only `failed`).
#[gpui::test]
async fn a_failed_instance_says_why(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let failures = cx.update(|cx| super::failures(&env, cx));
    assert_eq!(failures.len(), 1);
    let (name, e) = &failures[0];
    assert_eq!(name, "mysql-8.4");
    assert!(e.message.contains("held by mysqld (pid 9)"), "{}", e.message);
    assert_eq!(e.hint.as_deref(), Some("yard service port mysql-8.4 <port>"));
}

/// A port field that is not a port says so beside the sheet, which stays open, and nothing is
/// sent (M6 final review: Move did nothing, and Add dropped the port and took an auto one).
#[gpui::test]
async fn a_port_that_is_not_a_port_says_so(cx: &mut TestAppContext) {
    use crate::env::Sheet;
    use crate::widgets::text_field::TextField;
    use gpui::AppContext;
    let (fake, env) = env(cx);
    let field = |text: &str, cx: &mut TestAppContext| {
        let f = cx.new(|cx| TextField::new("f", "Port", "", cx));
        let text = text.to_string();
        f.update(cx, |f, cx| f.set_value(text, cx));
        f
    };
    let error = |id: &str, cx: &mut TestAppContext| {
        env.runner.read_with(cx, |r, _| r.errors.get(&ActionId(id.into())).map(|e| e.message.clone()))
    };
    let port = field("abc", cx);
    env.nav.update(cx, |n, _| n.sheet = Some(Sheet::Port { name: "redis".into(), port: port.clone() }));
    cx.update(|cx| super::forms::move_service(&env, "redis", &port, cx));
    assert_eq!(
        error("service:redis:port", cx).as_deref(),
        Some("abc is not a port: give a number from 1 to 65535")
    );
    assert!(env.nav.read_with(cx, |n, _| n.sheet.is_some()), "the sheet stays open");

    let port = field("0", cx);
    cx.update(|cx| super::forms::move_unit(&env, "nginx-http", &port, cx));
    assert_eq!(
        error("port:nginx-http:set", cx).as_deref(),
        Some("0 is not a port: give a number from 1 to 65535")
    );

    let (line, port) = (field("", cx), field("33o7", cx));
    cx.update(|cx| super::forms::add_service(&env, 2, &line, &port, cx));
    assert_eq!(
        error("service:add", cx).as_deref(),
        Some("33o7 is not a port: give a number from 1 to 65535")
    );
    cx.run_until_parked();
    assert_eq!(fake.count("services.port") + fake.count("ports.set") + fake.count("services.add"), 0);
}

fn services_on(env: &AppEnv, selected: Option<&str>, cx: &mut TestAppContext) {
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let screen = yard_app_model::hint::Screen::Services(selected.map(str::to_string));
    env.nav.update(cx, |n, _| n.screen = screen);
}

/// A row keeps one control; Port…, Sleep after…, Remove… and the rest are in the selected
/// instance's detail, with their ids as before (spec §4: 8 buttons on one row).
#[gpui::test]
async fn service_actions_live_in_the_detail(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    services_on(&env, None, cx);
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("service:redis:toggle").is_some(), "the row keeps Stop");
    assert!(vcx.debug_bounds("service:redis:port").is_none(), "Port… waits for a selection");
    vcx.update(|_, cx| {
        env.nav.update(cx, |n, cx| {
            n.screen = yard_app_model::hint::Screen::Services(Some("redis".into()));
            cx.notify();
        })
    });
    vcx.run_until_parked();
    for id in ["service:redis:port", "service:redis:sleep", "service:redis:remove", "service:redis:restart"] {
        assert!(vcx.debug_bounds(id).is_some(), "{id} is in the detail");
    }
    assert!(vcx.debug_bounds("service:mysql-8.4:port").is_none(), "only the selected instance's");
}

/// A selected instance that is gone leaves no detail acting on nothing.
#[gpui::test]
async fn a_removed_selection_falls_back_to_none(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    services_on(&env, Some("gone"), cx);
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("service:gone:detail").is_none());
    assert!(vcx.debug_bounds("service:redis:toggle").is_some(), "the table still draws");
}

/// A removed instance is no longer selected, so one added later under its name is not either
/// (final review).
#[gpui::test]
async fn a_removed_instance_is_no_longer_selected(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    services_on(&env, Some("redis"), cx);
    fake.answer("services.remove", serde_json::json!({}));
    cx.update(|cx| super::table::remove(&env, "redis".into(), false, cx));
    cx.run_until_parked();
    let screen = env.nav.read_with(cx, |n, _| n.screen.clone());
    assert_eq!(screen, yard_app_model::hint::Screen::Services(None));
}

/// A remove that fails keeps the selection, beside its error.
#[gpui::test]
async fn a_failed_remove_keeps_the_selection(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    services_on(&env, Some("redis"), cx);
    cx.update(|cx| super::table::remove(&env, "redis".into(), false, cx));
    cx.run_until_parked();
    let screen = env.nav.read_with(cx, |n, _| n.screen.clone());
    assert_eq!(screen, yard_app_model::hint::Screen::Services(Some("redis".into())));
}
