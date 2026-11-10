//! A push button.

use super::A11y;
use super::Pressable;
use super::disabled::Disabled;
use super::theme::{Theme, Typography, radius, size};
use gpui::{
    App, BoxShadow, ClickEvent, ElementId, Hsla, IntoElement, ParentElement, RenderOnce, Role, SharedString,
    Styled, Window, div, hsla, point, prelude::*, px,
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
    /// The row size: 20 pt high with a 12 px label.
    small: bool,
}

/// A style's background and label colour. Destructive is a normal button with a red label: the
/// prompt that follows carries the red (spec §3.5).
pub fn colors(style: ButtonStyle, t: &Theme) -> (Hsla, Hsla) {
    match style {
        ButtonStyle::Primary => (t.c.accent_fill, t.c.on_accent),
        ButtonStyle::Destructive => (t.c.control_bg, t.c.danger_text),
        ButtonStyle::Normal => (t.c.control_bg, t.c.text),
    }
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
            small: false,
        }
    }

    /// The row size (`size::CONTROL_SMALL`).
    pub fn small(mut self) -> Self {
        self.small = true;
        self
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
        let (bg, fg) = colors(self.style, &t);
        let bordered = self.style != ButtonStyle::Primary;
        let shadow = bordered && !t.dark;
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
            .flex()
            .items_center()
            .h(if self.small { size::CONTROL_SMALL } else { size::CONTROL })
            .px(if self.small { px(8.) } else { px(10.) })
            .rounded(radius::MD)
            .border_1()
            .border_color(if bordered { t.c.control_border } else { gpui::transparent_black() })
            .when(shadow, |d| {
                d.shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.05),
                    offset: point(px(0.), px(1.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                    inset: false,
                }])
            })
            .bg(bg)
            .text_color(fg)
            .map(|d| if self.small { d.type_style(12., 16., 500.) } else { d.type_body_strong() })
            .when(self.disabled, |d| d.opacity(0.45))
            .when(!self.disabled, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(Theme::darker(bg, 0.04)))
                    .active(move |s| s.bg(Theme::darker(bg, 0.08)))
            })
            .when_some(handler.filter(|_| !self.disabled), |d, h| d.on_press(move |e, w, cx| h(e, w, cx)))
            .child(self.label);
        if a11y.disabled { Disabled(button).into_any_element() } else { button.into_any_element() }
    }
}
