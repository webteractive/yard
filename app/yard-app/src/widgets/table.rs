//! A table whose rows keep their ids across refreshes: a row's id comes from its key, so a list
//! that refetches does not make assistive technology announce every row again.
//!
//! Columns have widths of their own and a row's controls sit in a reserved trailing column, so
//! every row's cells line up with the header whatever buttons it carries (G1). Cells truncate
//! rather than paint into their neighbour (G2), and an empty value reads as a dash (G18).

use super::Pressable;
use super::pill::Pill;
use super::theme::{Theme, Typography, size, space};
use super::{A11y, text};
use gpui::{
    AnyElement, App, ElementId, IntoElement, ParentElement, Pixels, RenderOnce, Role, SharedString, Styled,
    Window, div, prelude::*, px,
};
use std::rc::Rc;

/// What an empty value shows, in every table.
pub const EMPTY: &str = "—";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    Fixed(Pixels),
    /// A share of what the fixed columns leave.
    Flex(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub title: SharedString,
    pub width: Width,
    pub align: Align,
    /// Its values are states, drawn as pills (spec §3.5).
    pub pill: bool,
}

impl Column {
    pub fn flex(title: impl Into<SharedString>) -> Column {
        Column { title: title.into(), width: Width::Flex(1.), align: Align::Start, pill: false }
    }

    pub fn fixed(title: impl Into<SharedString>, width: f32) -> Column {
        Column { title: title.into(), width: Width::Fixed(px(width)), align: Align::Start, pill: false }
    }

    /// A larger share of the room left (`flex` is 1).
    pub fn grow(mut self, share: f32) -> Column {
        self.width = Width::Flex(share);
        self
    }

    /// A state column: each value is a pill.
    pub fn pill(mut self) -> Column {
        self.pill = true;
        self
    }

    /// Right-aligned (numbers).
    pub fn end(mut self) -> Column {
        self.align = Align::End;
        self
    }
}

impl From<SharedString> for Column {
    fn from(title: SharedString) -> Column {
        Column::flex(title)
    }
}

impl From<&str> for Column {
    fn from(title: &str) -> Column {
        Column::flex(SharedString::from(title.to_string()))
    }
}

pub struct Row {
    pub key: SharedString,
    pub cells: Vec<SharedString>,
    /// Controls drawn in the actions column (a toggle, a button).
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

/// A cell's text: the value, or the dash for an empty one.
pub fn shown(value: &SharedString) -> SharedString {
    if value.trim().is_empty() { EMPTY.into() } else { value.clone() }
}

#[derive(IntoElement)]
pub struct Table {
    id: SharedString,
    columns: Vec<Column>,
    rows: Vec<Row>,
    selected: Option<SharedString>,
    on_select: Option<super::KeyCallback>,
    actions: Option<Pixels>,
    empty: Option<(SharedString, Option<AnyElement>)>,
}

impl Table {
    pub fn new<C: Into<Column>>(
        id: impl Into<SharedString>,
        columns: impl IntoIterator<Item = C>,
        rows: Vec<Row>,
    ) -> Table {
        Table {
            id: id.into(),
            columns: columns.into_iter().map(Into::into).collect(),
            rows,
            selected: None,
            on_select: None,
            actions: None,
            empty: None,
        }
    }

    pub fn selected(mut self, key: Option<SharedString>) -> Self {
        self.selected = key;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&SharedString, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Reserves `width` at the end of the header and of every row for the row's controls.
    pub fn actions(mut self, width: f32) -> Self {
        self.actions = Some(px(width));
        self
    }

    /// What the table says with no rows, and at most one button to fix that.
    pub fn empty(mut self, text: impl Into<SharedString>, button: Option<AnyElement>) -> Self {
        self.empty = Some((text.into(), button));
        self
    }

    pub fn a11y(&self) -> A11y {
        let titles: Vec<&str> = self.columns.iter().map(|c| c.title.as_ref()).collect();
        A11y::new(self.id.clone(), Role::List, titles.join(", ").into())
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

/// A cell `div` sized for `column`; it truncates whatever it holds.
fn cell(column: &Column, selector: String) -> gpui::Div {
    let d = div().debug_selector(move || selector).overflow_hidden().truncate().min_w(px(0.));
    let d = match column.width {
        Width::Fixed(w) => d.w(w).flex_none(),
        Width::Flex(share) => d.flex_basis(px(0.)).flex_grow(share).flex_shrink(1.),
    };
    match column.align {
        Align::Start => d,
        Align::End => d.flex().justify_end(),
    }
}

impl RenderOnce for Table {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let id = self.id.clone();
        let header = div()
            .flex()
            .items_center()
            .h(size::HEADER)
            .px(space::SM)
            .gap(space::SM)
            .border_b_1()
            .border_color(t.c.separator)
            .type_secondary()
            .text_color(t.c.text_secondary)
            .children(self.columns.iter().enumerate().map(|(i, c)| {
                cell(c, format!("{id}:column:{i}")).child(text(format!("column:{i}"), c.title.clone()))
            }))
            .when_some(self.actions, |d, w| d.child(div().w(w).flex_none()));
        let row_height = if self.actions.is_some() { size::ROW_ACTIONS } else { size::ROW };
        let mut rows = vec![];
        let ids: Vec<SharedString> = self.rows.iter().map(|r| self.row_id(&r.key)).collect();
        for (row, row_id) in self.rows.into_iter().zip(ids) {
            let selected = self.selected.as_ref() == Some(&row.key);
            let handler = self.on_select.clone();
            let key = row.key.clone();
            let selector = row_id.to_string();
            let cells = self.columns.iter().enumerate().map(|(i, c)| {
                let value = row.cells.get(i).map(shown).unwrap_or_else(|| EMPTY.into());
                let pill = c.pill && value.as_ref() != EMPTY;
                cell(c, format!("{selector}:cell:{i}")).when(i == 0, |d| d.type_body_strong()).map(|d| {
                    if pill {
                        d.child(Pill::of(format!("cell:{i}"), value))
                    } else {
                        d.child(text(format!("cell:{i}"), value))
                    }
                })
            });
            let cells: Vec<_> = cells.collect();
            let extra = row.extra;
            rows.push(
                div()
                    .id(ElementId::Name(row_id))
                    .debug_selector({
                        let s = selector.clone();
                        move || s
                    })
                    .role(Role::ListItem)
                    .aria_label(row.cells.first().cloned().unwrap_or_default())
                    .aria_selected(selected)
                    .flex()
                    .items_center()
                    .h(row_height)
                    .px(space::SM)
                    .gap(space::SM)
                    .border_b_1()
                    .border_color(t.c.separator)
                    .text_color(t.c.text)
                    .when(selected, |d| d.bg(t.c.row_selected))
                    .when(!selected, |d| d.hover(|s| s.bg(t.c.hover)))
                    .when_some(handler, |d, h| d.cursor_pointer().on_press(move |_, w, cx| h(&key, w, cx)))
                    .children(cells)
                    .map(|d| match self.actions {
                        Some(w) => d.child(
                            div()
                                .w(w)
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(space::XS)
                                .children(extra),
                        ),
                        None => d.children(extra),
                    }),
            );
        }
        let empty = if rows.is_empty() { self.empty } else { None };
        div()
            .id(ElementId::Name(a11y.id))
            .role(Role::List)
            .aria_label(a11y.label)
            .flex()
            .flex_col()
            .min_w(px(0.))
            .type_body()
            .child(header)
            .children(rows)
            .when_some(empty, |d, (words, button)| {
                let selector = format!("{id}:empty");
                d.child(
                    div()
                        .debug_selector(move || selector)
                        .h(px(120.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(space::SM)
                        .text_color(t.c.text_secondary)
                        .child(text(format!("{id}:empty"), words))
                        .children(button),
                )
            })
    }
}
