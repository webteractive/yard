//! Logs (M6 design §8.2): every unit's live lines with filters, on a connection that is open only
//! while the screen is shown.

use super::heading;
use crate::env::{AppEnv, Nav};
use crate::platform::open::Opener;
use crate::services::logs::LogTail;
use crate::widgets::button::Button;
use crate::widgets::log_view::LogView;
use crate::widgets::picker::Picker;
use crate::widgets::toggle::Toggle;
use gpui::{AnyElement, App, AppContext, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_app_model::hint::Screen;

pub fn logs(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let start_unit = match &env.nav.read(cx).screen {
        Screen::Logs(u) => u.clone(),
        _ => None,
    };
    let tail = match env.nav.read(cx).logs.clone() {
        Some(t) => t,
        None => {
            let t = cx.new(|cx| {
                let mut tail = LogTail::new(env, None, None, cx);
                tail.buffer.unit_filter = start_unit.clone();
                tail
            });
            env.nav.update(cx, |n, _| n.logs = Some(t.clone()));
            t
        }
    };
    let filter = Nav::field(env, "logs:filter", "Filter the lines", "text to find", cx);
    let text = filter.read(cx).value().to_string();
    tail.update(cx, |t, _| t.buffer.text_filter = text);
    let buffer = &tail.read(cx).buffer;
    let mut units: Vec<String> =
        env.state.read(cx).status.value.iter().flat_map(|s| s.units.iter().map(|u| u.name.clone())).collect();
    units.sort();
    let mut options: Vec<SharedString> = vec!["All units".into()];
    options.extend(units.iter().map(|u| SharedString::from(u.clone())));
    let selected =
        buffer.unit_filter.as_ref().and_then(|u| units.iter().position(|x| x == u)).map_or(0, |i| i + 1);
    let lines: Vec<SharedString> =
        buffer.visible().map(|l| SharedString::from(format!("{}  {}", l.unit, l.text))).collect();
    let paused = buffer.paused();
    let (t1, t2, all) = (tail.clone(), tail.clone(), units.clone());
    let unit_picker = Picker::new("logs:unit", "Unit", options, Some(selected)).on_select(move |i, _, cx| {
        let unit = if i == 0 { None } else { all.get(i - 1).cloned() };
        t1.update(cx, |t, cx| {
            t.buffer.unit_filter = unit;
            cx.notify();
        });
    });
    let pause = Toggle::new("logs:pause", "Pause", paused).on_toggle(move |on, _, cx| {
        t2.update(cx, |t, cx| {
            t.buffer.set_paused(on);
            cx.notify();
        })
    });
    let file_unit = buffer.unit_filter.clone();
    let root = env.location.root.clone();
    let open_file = Button::new("logs:file", "Show log file").on_click(move |_, _, cx| {
        let file = match &file_unit {
            Some(u) => root.join("logs").join(format!("{u}.log")),
            None => root.join("logs"),
        };
        Opener::reveal(cx, &file);
    });
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading("Logs"))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .items_center()
                .child(unit_picker)
                .child(filter.clone())
                .child(pause)
                .child(open_file),
        )
        .child(LogView::new("logs:view", lines))
        .into_any_element()
}
