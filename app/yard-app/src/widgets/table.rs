//! A table whose rows keep their ids across refreshes: a row's id comes from its key, so a list
//! that refetches does not make assistive technology announce every row again.

use super::Pressable;
use super::theme::Theme;
use super::{A11y, text};
use gpui::{
    AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Window,
    div, prelude::*, px,
};
use std::rc::Rc;

pub struct Row {
    pub key: SharedString,
    pub cells: Vec<SharedString>,
    /// Controls drawn after the cells (a toggle, a button).
    pub extra: Vec<AnyElement>,
}

impl Row {
    pub fn new(key: impl Into<SharedString>, cells: Vec<SharedString>) -> Row {
        Row { key: key.into(), cells, extra: vec![] }
    }

    pub fn extra(mut self, element: impl IntoElement) -> Self {
        self.extra.push(element.into_any_element());
        self
    }
}

#[derive(IntoElement)]
pub struct Table {
    id: SharedString,
    columns: Vec<SharedString>,
    rows: Vec<Row>,
    selected: Option<SharedString>,
    on_select: Option<super::KeyCallback>,
}

impl Table {
    pub fn new(id: impl Into<SharedString>, columns: Vec<SharedString>, rows: Vec<Row>) -> Table {
        Table { id: id.into(), columns, rows, selected: None, on_select: None }
    }

    pub fn selected(mut self, key: Option<SharedString>) -> Self {
        self.selected = key;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&SharedString, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y::new(self.id.clone(), Role::List, self.columns.join(", ").into())
    }

    /// `(key, element id)` of each row.
    #[cfg(test)]
    pub fn row_ids(&self) -> Vec<(SharedString, SharedString)> {
        self.rows.iter().map(|r| (r.key.clone(), self.row_id(&r.key))).collect()
    }

    fn row_id(&self, key: &SharedString) -> SharedString {
        format!("{}:row:{key}", self.id).into()
    }
}

impl RenderOnce for Table {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let header = div()
            .flex()
            .px(px(8.))
            .py(px(4.))
            .border_b_1()
            .border_color(t.border)
            .text_xs()
            .text_color(t.muted)
            .children(
                self.columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| div().flex_1().child(text(format!("column:{i}"), c.clone()))),
            );
        let mut rows = vec![];
        let ids: Vec<SharedString> = self.rows.iter().map(|r| self.row_id(&r.key)).collect();
        for (row, row_id) in self.rows.into_iter().zip(ids) {
            let selected = self.selected.as_ref() == Some(&row.key);
            let handler = self.on_select.clone();
            let key = row.key.clone();
            rows.push(
                div()
                    .id(ElementId::Name(row_id))
                    .role(Role::ListItem)
                    .aria_label(row.cells.first().cloned().unwrap_or_default())
                    .aria_selected(selected)
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .py(px(5.))
                    .border_b_1()
                    .border_color(t.border)
                    .when(selected, |d| d.bg(Theme::wash(t.accent, 0.14)))
                    .when_some(handler, |d, h| d.cursor_pointer().on_press(move |_, w, cx| h(&key, w, cx)))
                    .children(row.cells.into_iter().enumerate().map(|(i, c)| {
                        div().flex_1().text_sm().text_color(t.text).child(text(format!("cell:{i}"), c))
                    }))
                    .children(row.extra),
            );
        }
        div()
            .id(ElementId::Name(a11y.id))
            .role(Role::List)
            .aria_label(a11y.label)
            .flex()
            .flex_col()
            .child(header)
            .children(rows)
    }
}
