//! Node (M6 design §8.2): installed versions, the default, installing and removing.

use super::{errors, job, quick};
use crate::env::AppEnv;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::table::{Column, Row, Table};
use crate::widgets::theme::Theme;
use crate::widgets::theme::Typography;
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use yard_client::api;
use yard_proto::methods::{NodeListParams, NodeSpecParams, NodeVersionParams};

/// The table's column titles, each read out by VoiceOver.
/// The table's columns, with their widths.
pub fn columns() -> Vec<Column> {
    vec![Column::fixed("Version", 110.), Column::fixed("Default", 90.).pill(), Column::flex("Sites")]
}

pub fn node(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let list = env.state.read(cx).node.value.clone();
    let rows = list.iter().flat_map(|l| l.versions.clone()).map(|v| {
        let n = v.version.clone();
        let (e1, n1, e2, n2) = (env.clone(), n.clone(), env.clone(), n.clone());
        let mut actions = div().flex().gap(px(4.));
        if !v.default {
            actions = actions.child(
                Button::new(format!("node:{n}:default"), "Make default").small().on_click(move |_, _, cx| {
                    quick(
                        &e1,
                        format!("node:{n1}:default"),
                        api::NODE_DEFAULT,
                        NodeSpecParams { spec: n1.clone() },
                        cx,
                    )
                }),
            );
        }
        actions = actions.child(
            Button::new(format!("node:{n}:uninstall"), "Uninstall")
                .small()
                .style(ButtonStyle::Destructive)
                .on_click(move |_, _, cx| {
                    job(
                        &e2,
                        format!("node:{n2}:uninstall"),
                        api::NODE_UNINSTALL,
                        NodeVersionParams { version: n2.clone() },
                        format!("Uninstall Node {n2}"),
                        cx,
                    )
                }),
        );
        Row::new(
            n.clone(),
            vec![
                n.clone().into(),
                (if v.default { "default" } else { "" }).into(),
                v.sites.join(", ").into(),
            ],
        )
        .extra(actions)
    });
    let table = Table::new("node", columns(), rows.collect())
        .actions(200.)
        .empty("No Node versions installed.", None);
    let available = env.nav.read(cx).node_available.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .children(errors(env, "node:", cx))
        .child(div().type_body().text_color(t.c.text_secondary).child(crate::widgets::text(
            "node:default",
            format!("Default: {}", list.as_ref().map(|l| l.default.clone()).unwrap_or_default()),
        )))
        .child(table)
        .children(available.map(|a| {
            div()
                .type_secondary()
                .text_color(t.c.text_secondary)
                .child(crate::widgets::text("node:available:list", a))
        }))
        .into_any_element()
}

/// The Node toolbar's button.
pub fn actions(env: &AppEnv) -> Vec<AnyElement> {
    let e4 = env.clone();
    let env = env.clone();
    vec![
        Button::new("node:available", "Show available")
            .on_click(move |_, _, cx| show_available(&e4, cx))
            .into_any_element(),
        Button::new("node:install:open", "Install…")
            .style(ButtonStyle::Primary)
            .on_click(move |_, _, cx| super::forms::open(&env, super::forms::Inline::NodeInstall, cx))
            .into_any_element(),
    ]
}

/// Show available: the versions fnm can install, newest first, under the table.
fn show_available(e4: &AppEnv, cx: &mut App) {
    let (env, daemon) = (e4.clone(), e4.daemon.clone());
    let task = env.runner.update(cx, |r, cx| {
        r.perform(
            crate::runner::ActionId("node:available".into()),
            async move {
                crate::daemon::call(
                    &*daemon,
                    api::NODE_LIST_AVAILABLE,
                    NodeListParams { available: true, major: None },
                )
                .await
            },
            cx,
        )
    });
    cx.spawn(async move |cx| {
        if let Some(list) = task.await {
            let text = list
                .available
                .iter()
                .map(|r| match &r.lts {
                    Some(lts) => format!("{} ({lts})", r.version),
                    None => r.version.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            cx.update(|cx| {
                env.nav.update(cx, |n, cx| {
                    n.node_available = Some(text);
                    cx.notify();
                })
            });
        }
    })
    .detach();
}
