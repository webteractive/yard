//! Output lines in a monospaced block, with `http(s)://` addresses as links.

use super::A11y;
use super::Pressable;
use super::button::Button;
use super::theme::{Theme, Typography, radius, space};
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
        A11y::new(self.id.clone(), Role::Log, "Output".into())
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
        let count = self.lines.len();
        // A plain line is text assistive technology reads (a role and a label alone left VoiceOver
        // with nothing to read: found by the checklist script); a line with an address is a link.
        let rows = self.lines.into_iter().enumerate().map(|(i, line)| {
            let id = format!("{}:line:{i}", self.id);
            let selector = id.clone();
            match link_in(&line).map(str::to_string) {
                Some(url) => div()
                    .id(ElementId::Name(id.into()))
                    .debug_selector(move || selector)
                    .role(Role::Link)
                    .aria_label(line.clone())
                    .min_w(px(0.))
                    .text_color(t.c.accent)
                    .cursor_pointer()
                    .on_press(move |_, _, cx| Opener::open_url(cx, &url))
                    .child(line)
                    .into_any_element(),
                None => div()
                    .debug_selector(move || selector)
                    .min_w(px(0.))
                    .child(super::text(id, line))
                    .into_any_element(),
            }
        });
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .px(space::SM)
            .py(space::XS)
            .border_b_1()
            .border_color(t.c.separator)
            .type_secondary()
            .text_color(t.c.text_secondary)
            .child(super::text(
                format!("{}:count", a11y.id),
                if count == 1 { "1 line".to_string() } else { format!("{count} lines") },
            ))
            .child(
                Button::new(format!("{}:copy", a11y.id), "Copy all")
                    .small()
                    .disabled(count == 0)
                    .on_click(move |_, _, cx| Opener::copy(cx, &all)),
            );
        let id = a11y.id.to_string();
        div()
            .id(ElementId::Name(a11y.id.clone()))
            .debug_selector(move || id)
            .role(Role::Log)
            .aria_label(a11y.label)
            .flex()
            .flex_col()
            .min_w(px(0.))
            .overflow_hidden()
            .rounded(radius::LG)
            .border_1()
            .border_color(t.c.separator)
            .bg(t.c.surface_sunken)
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .p(space::SM)
                    .type_mono()
                    .text_color(t.c.text)
                    .whitespace_normal()
                    .when(count == 0, |d| {
                        let selector = format!("{}:empty", self.id);
                        d.child(
                            div()
                                .debug_selector(move || selector)
                                .type_body()
                                .text_color(t.c.text_secondary)
                                .child(super::text(format!("{}:empty", self.id), "No log lines yet")),
                        )
                    })
                    .children(rows),
            )
    }
}
