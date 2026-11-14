//! Logs (M6 design §8.2): every unit's live lines with filters, on a connection that is open only
//! while the screen is shown.

use crate::env::{AppEnv, Nav};
use crate::platform::open::Opener;
use crate::services::logs::LogTail;
use crate::widgets::button::Button;
use crate::widgets::log_view::LogView;
use crate::widgets::picker::Picker;
use crate::widgets::toggle::Toggle;
use gpui::{AnyElement, App, AppContext, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_app_model::hint::Screen;

/// A unit's log file, or the logs folder for every unit.
pub fn log_file(paths: &yard_core::Paths, unit: Option<&str>) -> std::path::PathBuf {
    unit.map_or_else(|| paths.logs_dir(), |u| paths.log(u))
}

/// The screen's live tail, made the first time Logs is shown, starting on the unit it was opened for.
fn tail(env: &AppEnv, cx: &mut App) -> gpui::Entity<LogTail> {
    if let Some(t) = env.nav.read(cx).logs.clone() {
        return t;
    }
    let start_unit = match &env.nav.read(cx).screen {
        Screen::Logs(u) => u.clone(),
        _ => None,
    };
    let t = cx.new(|cx| {
        let mut tail = LogTail::new(env, None, None, cx);
        tail.buffer.unit_filter = start_unit;
        tail
    });
    env.nav.update(cx, |n, _| n.logs = Some(t.clone()));
    t
}

fn filter(env: &AppEnv, cx: &mut App) -> gpui::Entity<crate::widgets::text_field::TextField> {
    Nav::field(env, "logs:filter", "Filter the lines", "text to find", cx)
}

/// The toolbar's controls (spec §4, Logs): the unit, the filter, Pause and Show log file.
pub fn actions(env: &AppEnv, cx: &mut App) -> Vec<AnyElement> {
    let tail = tail(env, cx);
    let filter = filter(env, cx);
    let buffer = &tail.read(cx).buffer;
    let mut units: Vec<String> =
        env.state.read(cx).status.value.iter().flat_map(|s| s.units.iter().map(|u| u.name.clone())).collect();
    units.sort();
    let mut options: Vec<SharedString> = vec!["All units".into()];
    options.extend(units.iter().map(|u| SharedString::from(u.clone())));
    let selected =
        buffer.unit_filter.as_ref().and_then(|u| units.iter().position(|x| x == u)).map_or(0, |i| i + 1);
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
    let paths = env.paths();
    let open_file = Button::new("logs:file", "Show log file").on_click(move |_, _, cx| {
        Opener::reveal(cx, &log_file(&paths, file_unit.as_deref()));
    });
    vec![
        unit_picker.into_any_element(),
        div().w(px(180.)).child(filter).into_any_element(),
        pause.into_any_element(),
        open_file.into_any_element(),
    ]
}

pub fn logs(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let tail = tail(env, cx);
    let text = filter(env, cx).read(cx).value().to_string();
    tail.update(cx, |t, _| t.buffer.text_filter = text);
    let lines: Vec<SharedString> = tail
        .read(cx)
        .buffer
        .visible()
        .map(|l| SharedString::from(format!("{}  {}", l.unit, l.text)))
        .collect();
    // The log fills the page under the toolbar, lines or none (spec §4, Logs).
    let height = window.viewport_size().height - crate::widgets::theme::size::TOOLBAR - px(44.);
    div().flex().flex_col().child(LogView::new("logs:view", lines).min_height(height)).into_any_element()
}
