//! Doctor (M6 design §8.2): yardd's checks grouped by status, each hint as the button it can be.

use super::heading;
use crate::env::AppEnv;
use crate::platform::open::Opener;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::theme::Theme;
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use yard_app_model::hint::HintAction;
use yard_proto::{Check, CheckStatus};

/// What a hint becomes on a check: Terminal and Copy for the commands handed to Terminal, Copy
/// alone for anything the app will not run, else the hint itself and Copy.
fn buttons(hint: Option<&str>) -> Vec<&'static str> {
    match hint.map(HintAction::parse) {
        None => vec![],
        Some(HintAction::Copy(_)) => vec!["Copy"],
        Some(HintAction::Terminal(_)) => vec!["Open in Terminal", "Copy"],
        Some(_) => vec!["Fix", "Copy"],
    }
}

/// A check as the screen lists it: its message and the buttons its hint becomes.
#[cfg(test)]
pub type Line = (String, Vec<&'static str>);

/// `(group, lines)`: failing, then warnings, then ok, leaving out empty groups, as the tests read it.
#[cfg(test)]
pub fn groups(report: &yard_proto::DoctorReport) -> Vec<(&'static str, Vec<Line>)> {
    let of = |status: CheckStatus| -> Vec<&Check> {
        report.checks.iter().filter(|c| c.status == status).collect()
    };
    [
        ("Failing", CheckStatus::Fail),
        ("Warnings", CheckStatus::Warn),
        ("OK", CheckStatus::Ok),
        ("Other", CheckStatus::Other),
    ]
    .into_iter()
    .map(|(name, status)| {
        (
            name,
            of(status)
                .into_iter()
                .map(|c| (c.message.clone(), buttons(c.hint.as_deref())))
                .collect::<Vec<_>>(),
        )
    })
    .filter(|(_, checks)| !checks.is_empty())
    .collect()
}

pub fn doctor(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let s = env.state.read(cx);
    let report = s.doctor.value.clone();
    let running = s.doctor.loading;
    let run = env.clone();
    let mut page = div().flex().flex_col().gap(px(10.)).child(heading("Doctor")).child(
        Button::new("doctor:run", if running { "Running…" } else { "Run doctor" })
            .style(ButtonStyle::Primary)
            .disabled(running)
            .on_click(move |_, _, cx| run.state.update(cx, |s, cx| s.run_doctor(cx))),
    );
    if let Some(e) = &s.doctor.error {
        page = page.child(div().text_sm().text_color(t.danger).child(e.message.clone()));
    }
    let Some(report) = report else { return page.into_any_element() };
    for status in [CheckStatus::Fail, CheckStatus::Warn, CheckStatus::Ok] {
        let checks: Vec<Check> = report.checks.iter().filter(|c| c.status == status).cloned().collect();
        if checks.is_empty() {
            continue;
        }
        let (title, color) = match status {
            CheckStatus::Fail => ("Failing", t.danger),
            CheckStatus::Warn => ("Warnings", t.warning),
            _ => ("OK", t.muted),
        };
        page = page.child(div().text_lg().text_color(color).child(title));
        for c in checks {
            let mut row = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .child(div().text_sm().child(c.message.clone()));
            if let Some(hint) = c.hint.clone() {
                let action = HintAction::parse(&hint);
                let mut controls = div()
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_xs().text_color(t.muted).child(hint.clone()));
                for label in buttons(Some(&hint)) {
                    let (env, action, hint) = (env.clone(), action.clone(), hint.clone());
                    controls =
                        controls.child(Button::new(format!("doctor:{}:{label}", c.id), label).on_click(
                            move |_, _, cx| match label {
                                "Copy" => Opener::copy(cx, &hint),
                                _ => env.perform(action.clone(), cx),
                            },
                        ));
                }
                row = row.child(controls);
            }
            page = page.child(row);
        }
    }
    page.into_any_element()
}
