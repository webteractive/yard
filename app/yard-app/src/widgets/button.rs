//! A push button.

use super::A11y;
use super::theme::Theme;
use gpui::{
    App, ClickEvent, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Window,
    div, prelude::*, px,
};
use std::rc::Rc;

type Handler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Normal,
    Primary,
    Destructive,
}

#[derive(IntoElement)]
pub struct Button {
    id: SharedString,
    label: SharedString,
    style: ButtonStyle,
    disabled: bool,
    on_click: Option<Handler>,
}

impl Button {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Button {
        Button {
            id: id.into(),
            label: label.into(),
            style: ButtonStyle::Normal,
            disabled: false,
            on_click: None,
        }
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y { id: self.id.clone(), role: Role::Button, label: self.label.clone() }
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let (bg, fg) = match self.style {
            ButtonStyle::Primary => (t.accent, t.on_accent),
            ButtonStyle::Destructive => (t.danger, t.on_accent),
            ButtonStyle::Normal => (t.surface, t.text),
        };
        let handler = self.on_click.clone();
        div()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Button)
            .aria_label(self.label.clone())
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(t.border)
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .when(self.disabled, |d| d.opacity(0.5))
            .when(!self.disabled, |d| d.cursor_pointer())
            .when_some(handler.filter(|_| !self.disabled), |d, h| d.on_click(move |e, w, cx| h(e, w, cx)))
            .child(self.label)
    }
}
