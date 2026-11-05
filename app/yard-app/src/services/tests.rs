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
