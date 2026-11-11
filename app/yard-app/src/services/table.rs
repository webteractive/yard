//! The services table and its row actions (M6 design §8.2 Services).

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::services::logs::LogTail;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::group::Group;
use crate::widgets::table::{Column, Row, Table};
use crate::widgets::text;
use crate::widgets::text_field::TextField;
use gpui::{
    AnyElement, App, AppContext, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div,
    prelude::*, px,
};
use time::OffsetDateTime;
use yard_app_model::hint::Screen;
use yard_client::api;
use yard_core::present::{sleep_text, state_text};
use yard_proto::methods::{EnvParams, ServiceParams, ServiceRemoveParams};
use yard_proto::{ErrorCode, ServiceInfo, ServiceManager, ServiceState, YardError, engine_display};

/// Remove's question and its buttons (design §11.3).
pub fn remove_question(name: &str) -> (String, [&'static str; 3]) {
    (format!("Delete {name} and its data?"), ["Delete", "Remove, keep its data", "Cancel"])
}

/// The answer to `remove_question` as `services.remove`'s `keep_data`; `None` is Cancel.
pub fn keep_data(answer: usize) -> Option<bool> {
    match answer {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

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

/// An instance's controls, keyed by verb (`toggle`, `restart`, `logs`, `env`, `console`, `port`,
/// `sleep`, `remove`), with their ids and labels as before. The row keeps one; the rest are in the
/// selected instance's detail.
fn controls(env: &AppEnv, s: &ServiceInfo, cx: &mut App) -> Vec<(&'static str, AnyElement)> {
    let _ = &cx;
    let n = s.name.clone();
    let mut out: Vec<(&'static str, AnyElement)> = vec![];
    if s.managed_by == ServiceManager::Yard {
        let running = s.state == ServiceState::Running;
        let (e1, n1) = (env.clone(), n.clone());
        out.push((
            "toggle",
            Button::new(format!("service:{n}:toggle"), if running { "Stop" } else { "Start" })
                .on_click(move |_, _, cx| {
                    let m = if running { api::SERVICES_STOP } else { api::SERVICES_START };
                    simple(
                        &e1,
                        format!("service:{n1}:{}", if running { "stop" } else { "start" }),
                        m,
                        n1.clone(),
                        cx,
                    )
                })
                .small()
                .into_any_element(),
        ));
        let (e2, n2) = (env.clone(), n.clone());
        out.push((
            "restart",
            Button::new(format!("service:{n}:restart"), "Restart")
                .on_click(move |_, _, cx| {
                    simple(&e2, format!("service:{n2}:restart"), api::SERVICES_RESTART, n2.clone(), cx)
                })
                .small()
                .into_any_element(),
        ));
    }
    let (e3, n3) = (env.clone(), n.clone());
    out.push((
        "logs",
        Button::new(format!("service:{n}:logs"), "Logs")
            .on_click(move |_, _, cx| {
                let (env, name) = (e3.clone(), n3.clone());
                let tail = cx.new(|cx| LogTail::new(&env, Some(name.clone()), Some(name.clone()), cx));
                env.nav.update(cx, |nav, cx| {
                    nav.service_log = Some((name, tail));
                    cx.notify();
                });
            })
            .small()
            .into_any_element(),
    ));
    let (e4, n4) = (env.clone(), n.clone());
    out.push((
        "env",
        Button::new(format!("service:{n}:env"), "Copy .env")
            .on_click(move |_, _, cx| {
                copy_env(
                    &e4,
                    EnvParams { path: None, site: None, service: Some(n4.clone()) },
                    format!("service:{n4}:env"),
                    cx,
                )
            })
            .small()
            .into_any_element(),
    ));
    if let Some(console) = s.console.clone() {
        out.push((
            "console",
            Button::new(format!("service:{n}:console"), "Open console")
                .on_click(move |_, _, cx| cx.open_url(&console))
                .small()
                .into_any_element(),
        ));
    }
    if s.managed_by == ServiceManager::Yard {
        let (e5, n5) = (env.clone(), n.clone());
        out.push((
            "port",
            Button::new(format!("service:{n}:port"), "Port…")
                .on_click(move |_, _, cx| {
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
                })
                .small()
                .into_any_element(),
        ));
        let (e6, n6) = (env.clone(), n.clone());
        out.push((
            "sleep",
            Button::new(format!("service:{n}:sleep"), "Sleep after…")
                .on_click(move |_, _, cx| {
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
                })
                .small()
                .into_any_element(),
        ));
        let (e7, n7) = (env.clone(), n.clone());
        out.push((
            "remove",
            Button::new(format!("service:{n}:remove"), "Remove…")
                .style(ButtonStyle::Destructive)
                .on_click(move |_, window, cx| {
                    if refuse_in_use(&e7, &n7, cx) {
                        return;
                    }
                    let (title, buttons) = remove_question(&n7);
                    let answer = window.prompt(PromptLevel::Critical, &title, None, &buttons, cx);
                    let (env, name) = (e7.clone(), n7.clone());
                    cx.spawn(async move |cx| {
                        if let Some(keep_data) = answer.await.ok().and_then(keep_data) {
                            cx.update(|cx| remove(&env, name, keep_data, cx));
                        }
                    })
                    .detach();
                })
                .small()
                .into_any_element(),
        ));
    }
    out
}

/// Removes `name` (`keep_data` keeps its data directory); its error stays beside Remove….
pub fn remove(env: &AppEnv, name: String, keep_data: bool, cx: &mut App) {
    let (daemon, params) = (env.daemon.clone(), ServiceRemoveParams { name: name.clone(), keep_data });
    let removal = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(format!("service:{name}:remove")),
            async move { call(&*daemon, api::SERVICES_REMOVE, params).await.map(|_| ()) },
            cx,
        )
    });
    crate::window::main::forget_after(env, name, removal, cx);
}

/// The selected instance's detail: its port and sleep with the sheets that change them, and its
/// other controls (spec §4, Services). `None` when `name` is no longer an instance.
pub fn detail(env: &AppEnv, name: &str, cx: &mut App) -> Option<AnyElement> {
    let now = OffsetDateTime::now_utc();
    let s = env.state.read(cx).services.value.iter().flatten().find(|s| s.name == name).cloned()?;
    let cells = cells(&s, now);
    let mut controls = controls(env, &s, cx);
    let mut take = |key: &str| controls.iter().position(|(k, _)| *k == key).map(|i| controls.remove(i).1);
    let (port, sleep, remove) = (take("port"), take("sleep"), take("remove"));
    // The row keeps Start or Stop, so the detail leaves it out.
    let _ = take("toggle");
    let rest: Vec<AnyElement> = controls.into_iter().map(|(_, e)| e).collect();
    let cell = |i: usize| {
        crate::widgets::table::shown(&SharedString::from(cells.get(i).cloned().unwrap_or_default()))
    };
    let id = format!("service:{name}:detail");
    let selector = id.clone();
    Some(
        // An accessibility group named for the instance, so its controls are read (and found by
        // the checklist's `ax press <label> <instance>`) as that instance's, as they were in its row.
        div()
            .id(gpui::ElementId::Name(id.clone().into()))
            .role(gpui::Role::Group)
            .aria_label(SharedString::from(name.to_string()))
            .debug_selector(move || selector)
            .child(
                Group::new(id, Some(name.to_string().into()))
                    .row(
                        "ports",
                        "Ports",
                        crate::widgets::bar()
                            .child(text(format!("service:{name}:ports"), cell(3)))
                            .children(port),
                    )
                    .row(
                        "sleep",
                        "Sleeps after",
                        crate::widgets::bar()
                            .child(text(format!("service:{name}:sleep-after"), cell(4)))
                            .children(sleep),
                    )
                    .row("run-by", "Run by", text(format!("service:{name}:run-by"), cell(5)))
                    .footer(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(crate::widgets::bar().children(rest))
                            .children(remove),
                    ),
            )
            .into_any_element(),
    )
}

pub fn table(env: &AppEnv, selected: Option<String>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let now = OffsetDateTime::now_utc();
    let services: Vec<ServiceInfo> = env.state.read(cx).services.value.clone().unwrap_or_default();
    let rows = services.iter().map(|s| {
        let n = s.name.clone();
        let mut row = Row::new(s.name.clone(), cells(s, now).into_iter().map(SharedString::from).collect());
        // The row keeps Start or Stop (Logs for an instance Yard does not run); the rest is in
        // the selected instance's detail.
        let mut controls = controls(env, s, cx);
        let keep = if controls.iter().any(|(k, _)| *k == "toggle") { "toggle" } else { "logs" };
        if let Some(i) = controls.iter().position(|(k, _)| *k == keep) {
            row = row.extra(controls.remove(i).1);
        }
        let _ = n;
        row
    });
    Table::new(
        "services",
        [
            Column::flex("Instance"),
            Column::flex("Engine"),
            Column::fixed("State", 110.).pill(),
            Column::fixed("Ports", 90.),
            Column::fixed("Sleep", 70.),
            Column::fixed("Run by", 70.),
            Column::flex("Used by"),
        ],
        rows.collect(),
    )
    .actions(90.)
    .empty("No services. Add one to run MySQL, PostgreSQL, Redis or MinIO for your sites.", None)
    .selected(selected.map(SharedString::from))
    .on_select({
        let env = env.clone();
        move |key, _, cx| crate::window::main::open(&env, Screen::Services(Some(key.to_string())), cx)
    })
    .into_any_element()
}
