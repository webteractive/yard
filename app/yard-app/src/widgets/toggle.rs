//! A switch: on or off.

use super::A11y;
use super::Pressable;
use super::disabled::Disabled;
use super::theme::Theme;
use super::theme::Typography;
use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Toggled, Window, div,
    prelude::*, px,
};
use std::rc::Rc;

#[derive(IntoElement)]
pub struct Toggle {
    id: SharedString,
    label: SharedString,
    on: bool,
    disabled: Option<SharedString>,
    on_toggle: Option<super::Callback<bool>>,
    /// Only read aloud: the row's own label already says what it is.
    label_hidden: bool,
}

impl Toggle {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, on: bool) -> Toggle {
        Toggle {
            id: id.into(),
            label: label.into(),
            on,
            disabled: None,
            on_toggle: None,
            label_hidden: false,
        }
    }

    /// Greys the switch out and ignores clicks, `why` read as its description.
    pub fn disabled_because(mut self, why: Option<SharedString>) -> Self {
        self.disabled = why;
        self
    }

    /// Shows no label beside it, for a row whose label already names it; VoiceOver still reads
    /// the label.
    pub fn label_hidden(mut self) -> Self {
        self.label_hidden = true;
        self
    }

    /// Called with the new value.
    pub fn on_toggle(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y {
            disabled: self.disabled.is_some(),
            description: self.disabled.clone(),
            ..A11y::new(self.id.clone(), Role::Switch, self.label.clone())
        }
    }
}

/// The switch's track colour.
pub fn track(on: bool, t: &Theme) -> gpui::Hsla {
    if on { t.c.accent } else { t.c.switch_off }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let on = self.on;
        let a11y = self.a11y();
        let handler = self.on_toggle.filter(|_| !a11y.disabled);
        let selector = a11y.id.to_string();
        let switch = div()
            .id(ElementId::Name(a11y.id.clone()))
            .debug_selector(move || selector)
            .role(a11y.role)
            .aria_label(a11y.label.clone())
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .when_some(a11y.description.clone(), |d, why| d.aria_description(why))
            .when(a11y.disabled, |d| d.opacity(0.6))
            .flex()
            .items_center()
            .gap(px(6.))
            .when_some(handler, |d, h| d.cursor_pointer().on_press(move |_, w, cx| h(!on, w, cx)))
            .child(
                div()
                    .flex_none()
                    .w(px(32.))
                    .h(px(18.))
                    .rounded_full()
                    .bg(track(on, &t))
                    .flex()
                    .when(on, |d| d.justify_end())
                    .p(px(2.))
                    .child(div().size(px(14.)).rounded_full().bg(gpui::white()).shadow_sm()),
            )
            .when(!self.label_hidden, |d| d.child(div().type_body().text_color(t.c.text).child(self.label)));
        if a11y.disabled { Disabled(switch).into_any_element() } else { switch.into_any_element() }
    }
}
