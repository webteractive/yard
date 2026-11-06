//! Services (M6 design §8.2): the instances, their row actions, adding one, and a live log.

pub mod forms;
pub mod logs;
pub mod table;

use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::error_view::ErrorView;
use crate::widgets::log_view::LogView;
use crate::widgets::text_field::TextField;
use crate::widgets::theme::Theme;
use gpui::{AnyElement, App, AppContext, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_app_model::hint::HintAction;

pub fn services(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let add = env.clone();
    let header = div()
        .flex()
        .justify_between()
        .items_center()
        .child(div().text_xl().child(crate::widgets::text("heading", "Services")))
        .child(Button::new("service:add", "Add service…").style(ButtonStyle::Primary).on_click(
            move |_, _, cx| {
                let line = cx.new(|cx| {
                    TextField::new(
                        "sheet:add-service:line",
                        "Line (optional), such as 8.4",
                        "the engine's default",
                        cx,
                    )
                });
                let port = cx.new(|cx| {
                    TextField::new(
                        "sheet:add-service:port",
                        "Port (optional)",
                        "the usual port, else the next free one",
                        cx,
                    )
                });
                add.nav.update(cx, |n, cx| {
                    n.sheet = Some(Sheet::AddService { engine: 0, line, port, lines: vec![] });
                    cx.notify();
                });
            },
        ));
    let runner = env.runner.read(cx);
    let names: Vec<String> =
        env.state.read(cx).services.value.iter().flatten().map(|s| s.name.clone()).collect();
    let mut errors = vec![];
    for n in names.iter().map(String::as_str).chain(["add"]) {
        for verb in ["start", "stop", "restart", "remove", "port", "sleep", "env", "add"] {
            let id = if n == "add" { "service:add".to_string() } else { format!("service:{n}:{verb}") };
            if let Some(e) = runner.errors.get(&ActionId(id.clone())) {
                let env = env.clone();
                errors.push(
                    ErrorView::new(
                        format!("{id}:error"),
                        e,
                        HintAction::parse(e.hint.as_deref().unwrap_or("")),
                    )
                    .on_action(move |a, _, cx| env.perform(a, cx)),
                );
            }
            if n == "add" {
                break;
            }
        }
    }
    let log = env.nav.read(cx).service_log.as_ref().map(|(name, tail)| {
        let lines: Vec<SharedString> =
            tail.read(cx).buffer.visible().map(|l| SharedString::from(l.text.clone())).collect();
        let close = env.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.muted)
                            .child(crate::widgets::text("service:log:title", format!("{name}'s log"))),
                    )
                    .child(Button::new("service:log:close", "Close log").on_click(move |_, _, cx| {
                        close.nav.update(cx, |n, cx| {
                            n.service_log = None;
                            cx.notify();
                        })
                    })),
            )
            .child(LogView::new("service:log", lines))
    });
    let failed = failures(env, cx).into_iter().map(|(name, e)| {
        let env = env.clone();
        ErrorView::new(
            format!("service:{name}:failed"),
            &e,
            HintAction::parse(e.hint.as_deref().unwrap_or("")),
        )
        .on_action(move |a, _, cx| env.perform(a, cx))
    });
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(header)
        .children(errors)
        .child(table::table(env, window, cx))
        .children(failed)
        .children(log)
        .into_any_element()
}

/// Why each failed instance failed, with the command that fixes it (`PORT_IN_USE` names the
/// holder and offers Port…), as `yard services` prints under its table.
pub fn failures(env: &AppEnv, cx: &App) -> Vec<(String, yard_proto::YardError)> {
    let services = env.state.read(cx).services.value.clone().unwrap_or_default();
    services.into_iter().filter_map(|s| s.state.failure(&s.name).map(|e| (s.name, e))).collect()
}

#[cfg(test)]
mod tests;
