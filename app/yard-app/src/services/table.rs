//! The services table and its row actions (M6 design §8.2 Services).

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::services::logs::LogTail;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::table::{Row, Table};
use crate::widgets::text_field::TextField;
use gpui::{
    AnyElement, App, AppContext, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div,
    px,
};
use time::OffsetDateTime;
use yard_client::api;
use yard_core::present::{sleep_text, state_text};
use yard_proto::methods::{EnvParams, ServiceParams, ServiceRemoveParams};
use yard_proto::{ErrorCode, ServiceInfo, ServiceManager, ServiceState, YardError, engine_display};

/// Each row's cells as shown: instance, engine and line, state, ports, sleep, managed by, used by.
#[cfg(test)]
pub fn row_texts(env: &AppEnv, cx: &App) -> Vec<Vec<String>> {
    let now = OffsetDateTime::now_utc();
    env.state.read(cx).services.value.iter().flatten().map(|s| cells(s, now)).collect()
}

fn cells(s: &ServiceInfo, now: OffsetDateTime) -> Vec<String> {
    let engine = match &s.version {
        Some(v) if s.engine != "emailulator" => format!("{} {v}", engine_display(&s.engine)),
        _ => engine_display(&s.engine).to_string(),
    };
    let ports = s.ports.iter().map(|p| p.port.to_string()).collect::<Vec<_>>().join(", ");
    let managed = match s.managed_by {
        ServiceManager::Yard => "yard",
        ServiceManager::LaunchAgent { .. } => "LaunchAgent",
        ServiceManager::Other => "unknown",
    };
    let used = if s.used_by.is_empty() { "-".to_string() } else { s.used_by.join(", ") };
    vec![s.name.clone(), engine, state_text(s, now), ports, sleep_text(s), managed.into(), used]
}

/// Refuses to remove an instance a `.yard` still lists, without asking yardd (design §11.3).
pub fn refuse_in_use(env: &AppEnv, name: &str, cx: &mut App) -> bool {
    let used: Vec<String> = env
        .state
        .read(cx)
        .services
        .value
        .iter()
        .flatten()
        .find(|s| s.name == name)
        .map(|s| s.used_by.clone())
        .unwrap_or_default();
    if used.is_empty() {
        return false;
    }
    let e = YardError::new(ErrorCode::ServiceInUse, format!("{name} is used by {}", used.join(", ")))
        .with_hint("remove it from those projects' .yard first");
    env.runner.update(cx, |r, cx| {
        r.errors.record(ActionId(format!("service:{name}:remove")), e);
        cx.notify();
    });
    true
}

fn simple(
    env: &AppEnv,
    id: String,
    m: yard_client::api::Method<ServiceParams, ServiceInfo>,
    name: String,
    cx: &mut App,
) {
    let daemon = env.daemon.clone();
    env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(id),
            async move { call(&*daemon, m, ServiceParams { name }).await.map(|_| ()) },
            cx,
        )
        .detach()
    });
}

/// What Copy .env copies: what `yard env` prints, without emailulator's warning.
pub fn env_text(blocks: &[yard_proto::EnvBlock]) -> String {
    yard_core::present::env_text(&yard_core::present::split_env_warnings(blocks.to_vec()).0)
}

pub fn copy_env(env: &AppEnv, params: EnvParams, id: String, cx: &mut App) {
    let daemon = env.daemon.clone();
    env.runner.update(cx, |r, cx| {
        let task = r.perform(ActionId(id), async move { call(&*daemon, api::ENV_PRINT, params).await }, cx);
        cx.spawn(async move |_, cx| {
            if let Some(blocks) = task.await {
                cx.update(|cx| crate::platform::open::Opener::copy(cx, &env_text(&blocks)));
            }
        })
        .detach();
    });
}

fn open_sheet(env: &AppEnv, sheet: impl FnOnce(&mut App) -> Sheet, cx: &mut App) {
    let sheet = sheet(cx);
    env.nav.update(cx, |n, cx| {
        n.sheet = Some(sheet);
        cx.notify();
    });
}

pub fn table(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let now = OffsetDateTime::now_utc();
    let services: Vec<ServiceInfo> = env.state.read(cx).services.value.clone().unwrap_or_default();
    let rows = services.iter().map(|s| {
        let n = s.name.clone();
        let mut row = Row::new(s.name.clone(), cells(s, now).into_iter().map(SharedString::from).collect());
        let mut actions = div().flex().gap(px(4.));
        if s.managed_by == ServiceManager::Yard {
            let running = s.state == ServiceState::Running;
            let (e1, n1) = (env.clone(), n.clone());
            actions = actions.child(
                Button::new(format!("service:{n}:toggle"), if running { "Stop" } else { "Start" }).on_click(
                    move |_, _, cx| {
                        let m = if running { api::SERVICES_STOP } else { api::SERVICES_START };
                        simple(
                            &e1,
                            format!("service:{n1}:{}", if running { "stop" } else { "start" }),
                            m,
                            n1.clone(),
                            cx,
                        )
                    },
                ),
            );
            let (e2, n2) = (env.clone(), n.clone());
            actions = actions.child(Button::new(format!("service:{n}:restart"), "Restart").on_click(
                move |_, _, cx| {
                    simple(&e2, format!("service:{n2}:restart"), api::SERVICES_RESTART, n2.clone(), cx)
                },
            ));
        }
        let (e3, n3) = (env.clone(), n.clone());
        actions =
            actions.child(Button::new(format!("service:{n}:logs"), "Logs").on_click(move |_, _, cx| {
                let (env, name) = (e3.clone(), n3.clone());
                let tail = cx.new(|cx| LogTail::new(&env, Some(name.clone()), Some(name.clone()), cx));
                env.nav.update(cx, |nav, cx| {
                    nav.service_log = Some((name, tail));
                    cx.notify();
                });
            }));
        let (e4, n4) = (env.clone(), n.clone());
        actions =
            actions.child(Button::new(format!("service:{n}:env"), "Copy .env").on_click(move |_, _, cx| {
                copy_env(
                    &e4,
                    EnvParams { path: None, site: None, service: Some(n4.clone()) },
                    format!("service:{n4}:env"),
                    cx,
                )
            }));
        if let Some(console) = s.console.clone() {
            actions = actions.child(
                Button::new(format!("service:{n}:console"), "Open console")
                    .on_click(move |_, _, cx| cx.open_url(&console)),
            );
        }
        if s.managed_by == ServiceManager::Yard {
            let (e5, n5) = (env.clone(), n.clone());
            actions =
                actions.child(Button::new(format!("service:{n}:port"), "Port…").on_click(move |_, _, cx| {
                    let name = n5.clone();
                    open_sheet(
                        &e5,
                        |cx| Sheet::Port {
                            port: cx.new(|cx| {
                                TextField::new(
                                    format!("sheet:port:{name}"),
                                    format!("Port for {name}"),
                                    "a free port",
                                    cx,
                                )
                            }),
                            name,
                        },
                        cx,
                    )
                }));
            let (e6, n6) = (env.clone(), n.clone());
            actions = actions.child(Button::new(format!("service:{n}:sleep"), "Sleep after…").on_click(
                move |_, _, cx| {
                    let name = n6.clone();
                    open_sheet(
                        &e6,
                        |cx| Sheet::Sleep {
                            after: cx.new(|cx| {
                                TextField::new(
                                    format!("sheet:sleep:{name}"),
                                    format!("Sleep after for {name}"),
                                    "15m, 1h, off or default",
                                    cx,
                                )
                            }),
                            name,
                        },
                        cx,
                    )
                },
            ));
            let (e7, n7) = (env.clone(), n.clone());
            actions = actions.child(
                Button::new(format!("service:{n}:remove"), "Remove…")
                    .style(ButtonStyle::Destructive)
                    .on_click(move |_, window, cx| {
                        if refuse_in_use(&e7, &n7, cx) {
                            return;
                        }
                        let answer = window.prompt(
                            PromptLevel::Critical,
                            &format!("Remove {}?", n7),
                            Some("Its data is deleted."),
                            &["Remove", "Cancel"],
                            cx,
                        );
                        let (env, name) = (e7.clone(), n7.clone());
                        cx.spawn(async move |cx| {
                            if answer.await == Ok(0) {
                                cx.update(|cx| {
                                    let daemon = env.daemon.clone();
                                    env.runner.update(cx, |r, cx| {
                                        r.perform(
                                            ActionId(format!("service:{name}:remove")),
                                            async move {
                                                call(
                                                    &*daemon,
                                                    api::SERVICES_REMOVE,
                                                    ServiceRemoveParams { name, keep_data: false },
                                                )
                                                .await
                                                .map(|_| ())
                                            },
                                            cx,
                                        )
                                        .detach()
                                    })
                                });
                            }
                        })
                        .detach();
                    }),
            );
        }
        row = row.extra(actions);
        row
    });
    Table::new(
        "services",
        ["Instance", "Engine", "State", "Ports", "Sleep", "Run by", "Used by"]
            .iter()
            .map(|c| SharedString::from(*c))
            .collect(),
        rows.collect(),
    )
    .into_any_element()
}
