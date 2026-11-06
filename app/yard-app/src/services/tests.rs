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
