//! The rest of the main window's screens (M6 design §8.2), and what they share.

pub mod doctor;
pub mod forms;
pub mod logs;
pub mod node;
pub mod php;
pub mod ports;
pub mod sources;
pub mod tunnels;

use crate::daemon::call;
use crate::env::AppEnv;
use crate::runner::ActionId;
use crate::widgets::error_view::ErrorView;
use crate::widgets::theme::Typography;
use gpui::{AnyElement, App, IntoElement, ParentElement, div};
use serde::Serialize;
use yard_app_model::hint::HintAction;
use yard_client::api::Method;
use yard_proto::JobStarted;

/// The screen's title.
pub fn heading(title: &str) -> AnyElement {
    div().type_title().child(crate::widgets::text("heading", title.to_string())).into_any_element()
}

/// Every action error whose id starts with `prefix`, each with its hint's action.
pub fn errors(env: &AppEnv, prefix: &str, cx: &App) -> Vec<AnyElement> {
    error_views(env, env.runner.read(cx).errors.matching(prefix))
}

/// An ErrorView per `(id, error)`, each with its hint's action.
pub fn error_views(
    env: &AppEnv,
    errors: Vec<(crate::runner::ActionId, yard_proto::YardError)>,
) -> Vec<AnyElement> {
    errors
        .into_iter()
        .map(|(id, e)| {
            let env = env.clone();
            ErrorView::new(format!("{}:error", id.0), &e, HintAction::parse(e.hint.as_deref().unwrap_or("")))
                .on_action(move |a, _, cx| env.perform(a, cx))
                .into_any_element()
        })
        .collect()
}

/// A quick call whose error stays beside `id`.
pub fn quick<P: Serialize + Send + 'static, R: serde::de::DeserializeOwned + 'static>(
    env: &AppEnv,
    id: String,
    m: Method<P, R>,
    params: P,
    cx: &mut App,
) {
    let daemon = env.daemon.clone();
    env.runner.update(cx, |r, cx| {
        r.perform(ActionId(id), async move { call(&*daemon, m, params).await.map(|_| ()) }, cx).detach()
    });
}

/// A job, followed to its end, titled for the Activity screen.
pub fn job<P: Serialize + Clone + 'static>(
    env: &AppEnv,
    id: String,
    m: Method<P, JobStarted>,
    params: P,
    title: String,
    cx: &mut App,
) {
    env.runner.update(cx, |r, cx| r.run_job(ActionId(id), m, params, title, cx).detach());
}

/// The value of a field, trimmed; `None` when empty.
pub fn value(field: &gpui::Entity<crate::widgets::text_field::TextField>, cx: &App) -> Option<String> {
    let v = field.read(cx).value().trim().to_string();
    (!v.is_empty()).then_some(v)
}

#[cfg(test)]
mod tests;
