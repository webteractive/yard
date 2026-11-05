//! Output lines in a monospaced block, with `http(s)://` addresses as links.

use super::A11y;
use super::theme::Theme;
use crate::platform::open::Opener;
use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Window, div,
    prelude::*, px,
};

#[derive(IntoElement)]
pub struct LogView {
    id: SharedString,
    lines: Vec<SharedString>,
}

impl LogView {
    pub fn new(id: impl Into<SharedString>, lines: Vec<SharedString>) -> LogView {
        LogView { id: id.into(), lines }
    }

    pub fn a11y(&self) -> A11y {
        A11y { id: self.id.clone(), role: Role::Log, label: "Output".into() }
    }
}

/// The first `http://` or `https://` address in `line`, up to the next space.
pub fn link_in(line: &str) -> Option<&str> {
    let at = line.find("https://").or_else(|| line.find("http://"))?;
    let rest = &line[at..];
    Some(rest.split_whitespace().next().unwrap_or(rest))
}

impl RenderOnce for LogView {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let all = self.lines.join("\n");
        let rows = self.lines.into_iter().enumerate().map(|(i, line)| {
            let row = div()
                .id(ElementId::Name(format!("{}:line:{i}", self.id).into()))
                .role(Role::Label)
                .aria_label(line.clone());
            match link_in(&line).map(str::to_string) {
                Some(url) => row
                    .role(Role::Link)
                    .text_color(t.accent)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| Opener::open_url(cx, &url))
                    .child(line),
                None => row.child(line),
            }
        });
        div()
            .id(ElementId::Name(a11y.id.clone()))
            .role(Role::Log)
            .aria_label(a11y.label)
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(px(6.))
            .bg(t.background)
            .font_family("Menlo")
            .text_xs()
            .text_color(t.text)
            .child(
                div()
                    .id(ElementId::Name(format!("{}:copy", a11y.id).into()))
                    .role(Role::Button)
                    .aria_label("Copy all")
                    .self_end()
                    .text_color(t.muted)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| Opener::copy(cx, &all))
                    .child("Copy all"),
            )
            .children(rows)
    }
}
