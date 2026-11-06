//! PHP (M6 design §8.2): every supported version, installing, removing and choosing the default,
//! a version's extensions, and its php.ini overrides.

use super::{errors, heading, job, quick, value};
use crate::env::{AppEnv, Nav};
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::checkbox::Checkbox;
use crate::widgets::picker::Picker;
use crate::widgets::table::{Row, Table};
use gpui::{AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, px};
use std::path::PathBuf;
use yard_app_model::hint::HintAction;
use yard_app_model::refresh::ResourceKey;
use yard_client::api;
use yard_proto::methods::{ExtAddParams, ExtParams, PhpConfigParams, PhpParams};

/// The table's column titles, each read out by VoiceOver.
pub const COLUMNS: [&str; 4] = ["Version", "Default", "Sites", "Pool"];

pub fn php(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let versions = env.state.read(cx).php.value.clone().unwrap_or_default();
    let default = versions.iter().find(|v| v.default).map(|v| v.version.clone());
    let installed: Vec<String> = versions.iter().filter(|v| v.installed).map(|v| v.version.clone()).collect();
    let shown = env
        .nav
        .read(cx)
        .php_version
        .clone()
        .or(default.clone())
        .filter(|v| installed.contains(v))
        .or_else(|| installed.first().cloned());
    let rows = versions.iter().map(|v| {
        let n = v.version.clone();
        let sites = if v.sites.is_empty() { "-".to_string() } else { v.sites.join(", ") };
        let pool = match (v.installed, &v.pool) {
            (false, _) => "not installed".to_string(),
            (true, Some(p)) => p.clone(),
            (true, None) => "-".to_string(),
        };
        let mut actions = div().flex().gap(px(4.));
        if v.installed {
            if !v.default {
                let (e, n1) = (env.clone(), n.clone());
                actions = actions.child(Button::new(format!("php:{n}:default"), "Make default").on_click(
                    move |_, _, cx| {
                        quick(
                            &e,
                            format!("php:{n1}:default"),
                            api::PHP_DEFAULT,
                            PhpParams { php: n1.clone() },
                            cx,
                        )
                    },
                ));
            }
            let (e, n2) = (env.clone(), n.clone());
            actions = actions.child(
                Button::new(format!("php:{n}:uninstall"), "Uninstall…")
                    .style(ButtonStyle::Destructive)
                    .on_click(move |_, window, cx| {
                        let answer = window.prompt(
                            PromptLevel::Warning,
                            &format!("Uninstall PHP {n2}?"),
                            Some("brew removes its keg."),
                            &["Uninstall", "Cancel"],
                            cx,
                        );
                        let (env, v) = (e.clone(), n2.clone());
                        cx.spawn(async move |cx| {
                            if answer.await == Ok(0) {
                                cx.update(|cx| {
                                    job(
                                        &env,
                                        format!("php:{v}:uninstall"),
                                        api::PHP_UNINSTALL,
                                        PhpParams { php: v.clone() },
                                        format!("Uninstall PHP {v}"),
                                        cx,
                                    )
                                });
                            }
                        })
                        .detach();
                    }),
            );
        } else {
            let (e, n3) = (env.clone(), n.clone());
            actions = actions.child(
                Button::new(format!("php:{n}:install"), "Install").style(ButtonStyle::Primary).on_click(
                    move |_, _, cx| {
                        job(
                            &e,
                            format!("php:{n3}:install"),
                            api::PHP_INSTALL,
                            PhpParams { php: n3.clone() },
                            format!("Install PHP {n3}"),
                            cx,
                        )
                    },
                ),
            );
        }
        Row::new(
            n.clone(),
            vec![
                n.clone().into(),
                (if v.default { "default" } else { "" }).into(),
                sites.into(),
                pool.into(),
            ],
        )
        .extra(actions)
    });
    let table = Table::new("php", COLUMNS.iter().map(|c| SharedString::from(*c)).collect(), rows.collect());
    let mut page = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("PHP"))
        .children(errors(env, "php:", cx))
        .child(table);
    if let Some(v) = shown {
        page = page.child(extensions(env, &v, &installed, window, cx));
    }
    page.into_any_element()
}

fn extensions(env: &AppEnv, v: &str, installed: &[String], _window: &mut Window, cx: &mut App) -> AnyElement {
    let loaded = env.state.read(cx).extensions.get(v).and_then(|r| r.value.clone());
    if loaded.is_none() && !env.state.read(cx).extensions.get(v).is_some_and(|r| r.loading) {
        let key = ResourceKey::Extensions(v.to_string());
        env.state.update(cx, |s, cx| s.refresh([key], cx));
    }
    let options: Vec<SharedString> = installed.iter().map(|x| SharedString::from(x.clone())).collect();
    let (pick, all) = (env.clone(), installed.to_vec());
    let picker =
        Picker::new("php:extensions:version", "PHP version", options, installed.iter().position(|x| x == v))
            .on_select(move |i, _, cx| {
                let chosen = all[i].clone();
                pick.nav.update(cx, |n, cx| {
                    n.php_version = Some(chosen);
                    cx.notify();
                });
            });
    let rows = loaded.unwrap_or_default().into_iter().map(|x| {
        let (e1, e2, name, php) = (env.clone(), env.clone(), x.name.clone(), v.to_string());
        let (name2, php2) = (name.clone(), php.clone());
        Row::new(x.name.clone(), vec![x.name.clone().into(), x.source.clone().into()]).extra(
            div()
                .flex()
                .gap(px(6.))
                .child(
                    Checkbox::new(format!("ext:{php}:{name}:on"), format!("{name} on PHP {php}"), x.enabled)
                        .on_toggle(move |on, _, cx| {
                            let m = if on { api::EXT_ENABLE } else { api::EXT_DISABLE };
                            quick(
                                &e1,
                                format!("php:{php}:ext:{name}"),
                                m,
                                ExtParams { name: name.clone(), php: Some(php.clone()) },
                                cx,
                            );
                            let key = ResourceKey::Extensions(php.clone());
                            e1.state.update(cx, |s, cx| s.refresh([key], cx));
                        }),
                )
                .child(Button::new(format!("ext:{php2}:{name2}:remove"), "Remove").on_click(
                    move |_, _, cx| {
                        job(
                            &e2,
                            format!("php:{php2}:ext:{name2}"),
                            api::EXT_REMOVE,
                            ExtParams { name: name2.clone(), php: Some(php2.clone()) },
                            format!("Remove {name2} from PHP {php2}"),
                            cx,
                        )
                    },
                )),
        )
    });
    let table = Table::new(format!("ext:{v}"), vec!["Extension".into(), "From".into()], rows.collect());
    let add = Nav::field(
        env,
        "php:ext:add",
        "Extension to add",
        "redis, xdebug, xdebug-3.4.5, /path/to/ext.so",
        cx,
    );
    let (e3, add2, v3) = (env.clone(), add.clone(), v.to_string());
    let key = Nav::field(env, "php:config:key", "php.ini key", "memory_limit", cx);
    let val = Nav::field(env, "php:config:value", "Value", "512M", cx);
    let (e4, k4, val4, v4) = (env.clone(), key.clone(), val.clone(), v.to_string());
    let (e5, k5, v5) = (env.clone(), key.clone(), v.to_string());
    let (e6, root, v6) = (env.clone(), env.location.root.clone(), v.to_string());
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .items_center()
                .child(div().text_lg().child(crate::widgets::text("extensions:heading", "Extensions")))
                .child(picker),
        )
        .child(table)
        .child(div().flex().gap(px(6.)).child(add.clone()).child(
            Button::new("php:ext:add:go", "Add").on_click(move |_, _, cx| {
                if let Some(name) = value(&add2, cx) {
                    job(
                        &e3,
                        format!("php:{v3}:ext:{name}"),
                        api::EXT_ADD,
                        ExtAddParams { name: name.clone(), php: Some(v3.clone()) },
                        format!("Add {name} to PHP {v3}"),
                        cx,
                    );
                }
            }),
        ))
        .child(div().text_lg().child(crate::widgets::text("overrides:heading", "php.ini overrides")))
        .child(
            div()
                .flex()
                .gap(px(6.))
                .child(key.clone())
                .child(val.clone())
                .child(Button::new("php:config:set", "Set").on_click(move |_, _, cx| {
                    if let Some(k) = value(&k4, cx) {
                        quick(
                            &e4,
                            format!("php:{v4}:config"),
                            api::PHP_CONFIG_SET,
                            PhpConfigParams { php: Some(v4.clone()), key: k, value: value(&val4, cx) },
                            cx,
                        );
                    }
                }))
                .child(Button::new("php:config:unset", "Unset").on_click(move |_, _, cx| {
                    if let Some(k) = value(&k5, cx) {
                        quick(
                            &e5,
                            format!("php:{v5}:config"),
                            api::PHP_CONFIG_UNSET,
                            PhpConfigParams { php: Some(v5.clone()), key: k, value: None },
                            cx,
                        );
                    }
                })),
        )
        .child(Button::new("php:overrides:open", "Open 99-overrides.ini").on_click(move |_, _, cx| {
            e6.perform(
                HintAction::OpenFile(
                    PathBuf::from(&root).join("php").join(&v6).join("conf.d/99-overrides.ini"),
                ),
                cx,
            )
        }))
        .into_any_element()
}
