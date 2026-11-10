//! A group box: rows of a label and a value or control on a surface, the System Settings look
//! (spec §3.5). The Overview's facts, the site and service details and Settings use it.

use super::text;
use super::theme::{Theme, Typography, radius, space};
use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, prelude::*,
    px,
};

/// A row's minimum height.
pub const ROW_HEIGHT: f32 = 32.;

struct GroupRow {
    id: SharedString,
    label: SharedString,
    value: AnyElement,
    /// The value goes under the label (a long path, a note).
    below: bool,
}

#[derive(IntoElement)]
pub struct Group {
    id: SharedString,
    title: Option<SharedString>,
    rows: Vec<GroupRow>,
    footer: Option<AnyElement>,
}

impl Group {
    pub fn new(id: impl Into<SharedString>, title: Option<SharedString>) -> Group {
        Group { id: id.into(), title, rows: vec![], footer: None }
    }

    /// A row: `label` (read as text `<id>:label`) beside `value`.
    pub fn row(
        mut self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        value: impl IntoElement,
    ) -> Self {
        self.rows.push(GroupRow {
            id: id.into(),
            label: label.into(),
            value: value.into_any_element(),
            below: false,
        });
        self
    }

    /// A row whose value sits under its label.
    pub fn row_below(
        mut self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        value: impl IntoElement,
    ) -> Self {
        self.rows.push(GroupRow {
            id: id.into(),
            label: label.into(),
            value: value.into_any_element(),
            below: true,
        });
        self
    }

    /// Controls under the rows, inside the box.
    pub fn footer(mut self, element: impl IntoElement) -> Self {
        self.footer = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for Group {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let count = self.rows.len();
        let rows = self.rows.into_iter().enumerate().map(|(i, r)| {
            let selector = format!("{}:row:{}", self.id, r.id);
            let label = div()
                .w(px(120.))
                .flex_none()
                .text_color(t.c.text_secondary)
                .child(text(format!("{}:label", r.id), r.label));
            div()
                .debug_selector(move || selector)
                .min_h(px(ROW_HEIGHT))
                .px(space::MD)
                .py(px(5.))
                .flex()
                .min_w(px(0.))
                .when(i + 1 < count, |d| d.border_b_1().border_color(t.c.separator))
                .map(|d| {
                    if r.below {
                        d.flex_col().gap(space::XS).child(label).child(div().min_w(px(0.)).child(r.value))
                    } else {
                        d.items_center().gap(space::SM).child(label).child(
                            div().flex_1().min_w(px(0.)).flex().items_center().gap(space::SM).child(r.value),
                        )
                    }
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .min_w(px(0.))
            .when_some(self.title, |d, title| {
                d.child(div().type_section().child(text(format!("{}:title", self.id), title)))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .rounded(radius::LG)
                    .border_1()
                    .border_color(t.c.separator)
                    .bg(t.c.surface)
                    .type_body()
                    .children(rows)
                    .when_some(self.footer, |d, f| {
                        d.child(
                            div()
                                .px(space::MD)
                                .py(space::SM)
                                .border_t_1()
                                .border_color(t.c.separator)
                                .child(f),
                        )
                    }),
            )
    }
}
