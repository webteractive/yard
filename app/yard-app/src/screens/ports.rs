//! Ports (M6 design §8.2): the registry, who holds each port, pinning, moving, and the profile.

use super::{errors, heading, quick};
use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::widgets::button::Button;
use crate::widgets::picker::Picker;
use crate::widgets::table::{Row, Table};
use crate::widgets::text_field::TextField;
use gpui::{AnyElement, App, AppContext, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_client::api;
use yard_core::ports::mode_str;
use yard_core::present::holder_kind_name;
use yard_proto::methods::{ProfileParams, RangeParams, UnitParams};
use yard_proto::{PortMode, PortStatus, Profile};

/// The table's column titles, each read out by VoiceOver.
pub const COLUMNS: [&str; 5] = ["Unit", "Port", "Mode", "Status", "Held by"];

fn status(s: PortStatus) -> &'static str {
    match s {
        PortStatus::Free => "free",
        PortStatus::Yard => "yard",
        PortStatus::InUse => "in use",
        PortStatus::ExternalListening => "listening",
        PortStatus::ExternalNotListening => "not listening",
        PortStatus::Other => "unknown",
    }
}

pub fn ports(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let s = env.state.read(cx);
    let ports = s.ports.value.clone().unwrap_or_default();
    let profile = s.status.value.as_ref().map(|st| st.profile).unwrap_or_default();
    let rows = ports.iter().map(|p| {
        let holder = p
            .holder
            .as_ref()
            .map(|h| format!("{} (pid {}, {})", h.name, h.pid, holder_kind_name(h.kind)))
            .unwrap_or_else(|| "-".into());
        let unit = p.unit.clone();
        let pinned = p.mode == PortMode::Pinned;
        let (e1, u1, e2, u2) = (env.clone(), unit.clone(), env.clone(), unit.clone());
        let actions = div()
            .flex()
            .gap(px(4.))
            .child(Button::new(format!("port:{unit}:pin"), if pinned { "Unpin" } else { "Pin" }).on_click(
                move |_, _, cx| {
                    let m = if pinned { api::PORTS_UNPIN } else { api::PORTS_PIN };
                    quick(&e1, format!("port:{u1}:pin"), m, UnitParams { unit: u1.clone() }, cx)
                },
            ))
            .child(Button::new(format!("port:{unit}:set"), "Set…").on_click(move |_, _, cx| {
                let unit = u2.clone();
                let port = cx.new(|cx| {
                    TextField::new(
                        format!("sheet:unit-port:{unit}"),
                        format!("Port for {unit}"),
                        "a free port",
                        cx,
                    )
                });
                e2.nav.update(cx, |n, cx| {
                    n.sheet = Some(Sheet::UnitPort { unit, port });
                    cx.notify();
                })
            }));
        Row::new(
            p.unit.clone(),
            vec![
                p.unit.clone().into(),
                p.port.to_string().into(),
                mode_str(p.mode).into(),
                status(p.status).into(),
                holder.into(),
            ],
        )
        .extra(actions)
    });
    let table = Table::new("ports", COLUMNS.iter().map(|c| SharedString::from(*c)).collect(), rows.collect());
    let pick = env.clone();
    let profile_picker = Picker::new(
        "ports:profile",
        "Ports profile",
        vec!["standard (8080, 8443)".into(), "privileged (80, 443)".into()],
        Some(if profile == Profile::Privileged { 1 } else { 0 }),
    )
    .on_select(move |i, _, cx| {
        quick(
            &pick,
            "port:profile".into(),
            api::PORTS_PROFILE,
            ProfileParams { profile: if i == 1 { Profile::Privileged } else { Profile::Standard } },
            cx,
        )
    });
    let check = env.clone();
    let range = crate::env::Nav::field(env, "port:range", "Auto range", "29000-29099", cx);
    let (set, field) = (env.clone(), range.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Ports"))
        .children(errors(env, "port:", cx))
        .child(
            crate::widgets::bar().child(profile_picker).child(
                Button::new("ports:check", "Check now").on_click(move |_, _, cx| check_now(&check, cx)),
            ),
        )
        .child(table)
        .child(crate::widgets::bar().child(range).child(Button::new("port:range:set", "Set range").on_click(
            move |_, _, cx| {
                if let Some(r) = super::value(&field, cx) {
                    set_range(&set, r, cx);
                }
            },
        )))
        .into_any_element()
}

/// Check now: `ports.check` probes every port and answers the list as it found it, which the
/// screen then shows.
pub fn check_now(env: &AppEnv, cx: &mut App) {
    let (daemon, state) = (env.daemon.clone(), env.state.clone());
    let checked = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId("port:check".into()),
            async move { call(&*daemon, api::PORTS_CHECK, ()).await },
            cx,
        )
    });
    cx.spawn(async move |cx| {
        if let Some(ports) = checked.await {
            state.update(cx, |s, cx| {
                s.set_ports(ports);
                cx.notify();
            });
        }
    })
    .detach();
}

/// Range…: the ports Yard picks automatically come from `range` (`ports.range`, `29000-29099`).
pub fn set_range(env: &AppEnv, range: String, cx: &mut App) {
    quick(env, "port:range".into(), api::PORTS_RANGE, RangeParams { range }, cx);
}
