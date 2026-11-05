//! A switch: on or off.

use super::A11y;
use super::theme::Theme;
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
    disabled: bool,
    on_toggle: Option<super::Callback<bool>>,
}

impl Toggle {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, on: bool) -> Toggle {
        Toggle { id: id.into(), label: label.into(), on, disabled: false, on_toggle: None }
    }

    #[expect(dead_code, reason = "the site detail (M6 Task 14)")]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new value.
    #[expect(dead_code, reason = "the site detail (M6 Task 14)")]
    pub fn on_toggle(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y { id: self.id.clone(), role: Role::Switch, label: self.label.clone() }
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let on = self.on;
        let handler = self.on_toggle.filter(|_| !self.disabled);
        div()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Switch)
            .aria_label(self.label.clone())
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .flex()
            .items_center()
            .gap(px(6.))
            .when(self.disabled, |d| d.opacity(0.5))
            .when_some(handler, |d, h| d.cursor_pointer().on_click(move |_, w, cx| h(!on, w, cx)))
            .child(
                div()
                    .w(px(28.))
                    .h(px(16.))
                    .rounded(px(8.))
                    .bg(if on { t.accent } else { t.border })
                    .flex()
                    .when(on, |d| d.justify_end())
                    .p(px(2.))
                    .child(div().size(px(12.)).rounded(px(6.)).bg(t.surface)),
            )
            .child(div().text_sm().text_color(t.text).child(self.label))
    }
}
