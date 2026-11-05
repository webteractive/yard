//! A checkbox: a choice that is ticked or not.

use super::A11y;
use super::theme::Theme;
use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Toggled, Window, div,
    prelude::*, px,
};
use std::rc::Rc;

#[derive(IntoElement)]
pub struct Checkbox {
    id: SharedString,
    label: SharedString,
    checked: bool,
    on_toggle: Option<super::Callback<bool>>,
}

impl Checkbox {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, checked: bool) -> Checkbox {
        Checkbox { id: id.into(), label: label.into(), checked, on_toggle: None }
    }

    pub fn on_toggle(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y { id: self.id.clone(), role: Role::CheckBox, label: self.label.clone() }
    }
}

impl RenderOnce for Checkbox {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let checked = self.checked;
        div()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::CheckBox)
            .aria_label(self.label.clone())
            .aria_toggled(if checked { Toggled::True } else { Toggled::False })
            .flex()
            .items_center()
            .gap(px(6.))
            .when_some(self.on_toggle, |d, h| d.cursor_pointer().on_click(move |_, w, cx| h(!checked, w, cx)))
            .child(
                div()
                    .size(px(14.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(t.border)
                    .bg(if checked { t.accent } else { t.surface })
                    .text_color(t.on_accent)
                    .text_xs()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked, |d| d.child("✓")),
            )
            .child(div().text_sm().text_color(t.text).child(self.label))
    }
}
