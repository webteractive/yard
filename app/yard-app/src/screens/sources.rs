//! Sources (M6 design §8.2): the brew taps Yard installs from, and whether brew trusts them.

use super::{errors, heading, job};
use crate::env::AppEnv;
use crate::widgets::button::Button;
use crate::widgets::table::{Row, Table};
use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_client::api;
use yard_proto::methods::SourceParams;

pub fn sources(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let sources = env.state.read(cx).sources.value.clone().unwrap_or_default();
    let rows = sources.iter().map(|s| {
        let (e, tap, trusted) = (env.clone(), s.tap.clone(), s.trusted);
        let label = if trusted { "Untrust" } else { "Trust" };
        Row::new(
            s.tap.clone(),
            vec![
                s.tap.clone().into(),
                (if s.tapped { "tapped" } else { "not tapped" }).into(),
                (if trusted { "trusted" } else { "not trusted" }).into(),
            ],
        )
        .extra(Button::new(format!("source:{}:{}", s.tap, label.to_lowercase()), label).on_click(
            move |_, _, cx| {
                let m = if trusted { api::SOURCES_UNTRUST } else { api::SOURCES_TRUST };
                let title = format!("{} {tap}", if trusted { "Untrust" } else { "Trust" });
                job(
                    &e,
                    format!("source:{tap}"),
                    m,
                    SourceParams { target: tap.clone(), tap: true },
                    title,
                    cx,
                )
            },
        ))
    });
    let table = Table::new(
        "sources",
        ["Tap", "", "brew"].iter().map(|c| SharedString::from(*c)).collect(),
        rows.collect(),
    );
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Sources"))
        .children(errors(env, "source:", cx))
        .child(table)
        .into_any_element()
}
