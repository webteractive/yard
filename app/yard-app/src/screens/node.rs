//! Node (M6 design §8.2): installed versions, the default, installing and removing.

use super::{errors, heading, job, quick, value};
use crate::env::{AppEnv, Nav};
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::table::{Column, Row, Table};
use crate::widgets::theme::Theme;
use crate::widgets::theme::Typography;
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use yard_client::api;
use yard_proto::methods::{NodeInstallParams, NodeListParams, NodeSpecParams, NodeVersionParams};

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
    let spec = Nav::field(env, "node:install:spec", "Node version to install", "22, 22.17, lts/*", cx);
    let (e3, s3) = (env.clone(), spec.clone());
    let e4 = env.clone();
    let available = env.nav.read(cx).node_available.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Node"))
        .children(errors(env, "node:", cx))
        .child(div().type_body().text_color(t.muted).child(crate::widgets::text(
            "node:default",
            format!("Default: {}", list.as_ref().map(|l| l.default.clone()).unwrap_or_default()),
        )))
        .child(table)
        .child(crate::widgets::bar().child(spec.clone()).child(
            Button::new("node:install", "Install").style(ButtonStyle::Primary).on_click(move |_, _, cx| {
                if let Some(spec) = value(&s3, cx) {
                    job(
                        &e3,
                        "node:install".into(),
                        api::NODE_INSTALL,
                        NodeInstallParams { spec: Some(spec.clone()), path: None },
                        format!("Install Node {spec}"),
                        cx,
                    );
                }
            }),
        ))
        .child(crate::widgets::bar().child(Button::new("node:available", "Show available").on_click(
            move |_, _, cx| {
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
            },
        )))
        .children(available.map(|a| {
            div().type_secondary().text_color(t.muted).child(crate::widgets::text("node:available:list", a))
        }))
        .into_any_element()
}
