//! A push button.

use super::A11y;
use super::Pressable;
use super::disabled::Disabled;
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
    /// Read after the label: what the button does when its label alone does not say.
    description: Option<SharedString>,
    on_click: Option<Handler>,
}

impl Button {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Button {
        Button {
            id: id.into(),
            label: label.into(),
            style: ButtonStyle::Normal,
            disabled: false,
            description: None,
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

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y {
            disabled: self.disabled,
            description: self.description.clone(),
            ..A11y::new(self.id.clone(), Role::Button, self.label.clone())
        }
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
        let a11y = self.a11y();
        let button = div()
            .id(ElementId::Name(a11y.id.clone()))
            .debug_selector({
                let id = a11y.id.to_string();
                move || id
            })
            .role(a11y.role)
            .aria_label(a11y.label.clone())
            .when_some(a11y.description.clone(), |d, why| d.aria_description(why))
            .flex_none()
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
            .when_some(handler.filter(|_| !self.disabled), |d, h| d.on_press(move |e, w, cx| h(e, w, cx)))
            .child(self.label);
        if a11y.disabled { Disabled(button).into_any_element() } else { button.into_any_element() }
    }
}
