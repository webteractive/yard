//! Sources (M6 design §8.2): the brew taps Yard installs from, and whether brew trusts each tap
//! and each formula in it.

use super::{errors, heading, job};
use crate::env::AppEnv;
use crate::widgets::button::Button;
use crate::widgets::table::{Column, Row, Table};
use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_client::api;
use yard_proto::SourceInfo;
use yard_proto::methods::SourceParams;

/// The table's column titles, each read out by VoiceOver.
/// The table's columns, with their widths.
pub fn columns() -> Vec<Column> {
    vec![
        Column::flex("Tap or formula").grow(2.),
        Column::fixed("Tapped or installed", 150.).pill(),
        Column::fixed("Trusted by brew", 130.).pill(),
    ]
}

/// A row's trust action: the tap's, or one of its formulae's (`<tap>/<name>`).
pub struct Target {
    pub key: String,
    pub cells: [String; 3],
    pub verb: &'static str,
    pub params: SourceParams,
}

/// The tap, then each of its formulae, with Trust or Untrust for each.
pub fn targets(s: &SourceInfo) -> Vec<Target> {
    let verb = |trusted: bool| if trusted { "Untrust" } else { "Trust" };
    let trust = |trusted: bool| (if trusted { "trusted" } else { "not trusted" }).to_string();
    let mut out = vec![Target {
        key: s.tap.clone(),
        cells: [s.tap.clone(), (if s.tapped { "tapped" } else { "not tapped" }).into(), trust(s.trusted)],
        verb: verb(s.trusted),
        params: SourceParams { target: s.tap.clone(), tap: true },
    }];
    out.extend(s.formulae.iter().map(|f| {
        let full = format!("{}/{}", s.tap, f.name);
        Target {
            key: full.clone(),
            cells: [
                format!("  {}", f.name),
                (if f.installed { "installed" } else { "not installed" }).into(),
                trust(f.trusted),
            ],
            verb: verb(f.trusted),
            params: SourceParams { target: full, tap: false },
        }
    }));
    out
}

pub fn sources(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let sources = env.state.read(cx).sources.value.clone().unwrap_or_default();
    let rows = sources.iter().flat_map(targets).map(|t| {
        let e = env.clone();
        let Target { key, cells, verb, params } = t;
        Row::new(key.clone(), cells.into_iter().map(SharedString::from).collect()).extra(
            Button::new(format!("source:{key}:{}", verb.to_lowercase()), verb).small().on_click(
                move |_, _, cx| {
                    let m = if verb == "Untrust" { api::SOURCES_UNTRUST } else { api::SOURCES_TRUST };
                    job(&e, format!("source:{key}"), m, params.clone(), format!("{verb} {key}"), cx)
                },
            ),
        )
    });
    let table =
        Table::new("sources", columns(), rows.collect()).actions(90.).empty("No third-party taps.", None);
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Sources"))
        .children(errors(env, "source:", cx))
        .child(table)
        .into_any_element()
}
