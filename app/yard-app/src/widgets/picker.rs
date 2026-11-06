//! A choice of one value from a short list, shown in a popup under it.

use super::A11y;
use super::Pressable;
use super::disabled::Disabled;
use super::theme::Theme;
use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Window, anchored,
    deferred, div, prelude::*, px,
};
use std::rc::Rc;

#[derive(IntoElement)]
pub struct Picker {
    id: SharedString,
    label: SharedString,
    options: Vec<SharedString>,
    selected: Option<usize>,
    disabled: Option<SharedString>,
    on_select: Option<super::Callback<usize>>,
}

impl Picker {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        options: Vec<SharedString>,
        selected: Option<usize>,
    ) -> Picker {
        Picker { id: id.into(), label: label.into(), options, selected, disabled: None, on_select: None }
    }

    /// Disabled, saying why (`set in /p/.yard`).
    pub fn disabled_because(mut self, why: Option<SharedString>) -> Self {
        self.disabled = why;
        self
    }

    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y {
            disabled: self.disabled.is_some(),
            description: self.disabled.clone(),
            ..A11y::new(self.id.clone(), Role::ComboBox, self.label.clone())
        }
    }
}

impl RenderOnce for Picker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let open =
            window.use_keyed_state(ElementId::Name(format!("{}:open", self.id).into()), cx, |_, _| false);
        let is_open = *open.read(cx);
        let current = self.selected.and_then(|i| self.options.get(i)).cloned().unwrap_or_else(|| "—".into());
        let disabled = self.disabled.clone();
        let toggle = open.clone();
        let a11y = self.a11y();
        let mut root = div()
            .id(ElementId::Name(a11y.id.clone()))
            .role(a11y.role)
            .aria_label(a11y.label)
            .aria_value(current.clone())
            .aria_expanded(is_open)
            .when_some(a11y.description.clone(), |d, why| d.aria_description(why))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .text_sm()
            .text_color(t.text)
            .child(current)
            .child(div().text_color(t.muted).child("▾"));
        if let Some(why) = &disabled {
            root = root.opacity(0.6).child(div().text_xs().text_color(t.muted).child(why.clone()));
            return Disabled(root).into_any_element();
        } else {
            root = root.cursor_pointer().on_press(move |_, _, cx| {
                toggle.update(cx, |o, cx| {
                    *o = !*o;
                    cx.notify();
                })
            });
        }
        if is_open && disabled.is_none() {
            let items = self.options.iter().enumerate().map(|(i, o)| {
                let close = open.clone();
                let handler = self.on_select.clone();
                div()
                    .id(ElementId::Name(format!("{}:option:{o}", self.id).into()))
                    .role(Role::MenuItem)
                    .aria_label(o.clone())
                    .aria_selected(self.selected == Some(i))
                    .px(px(10.))
                    .py(px(4.))
                    .cursor_pointer()
                    .when(self.selected == Some(i), |d| d.bg(Theme::wash(t.accent)))
                    .on_press(move |_, w, cx| {
                        close.update(cx, |o, cx| {
                            *o = false;
                            cx.notify();
                        });
                        if let Some(h) = &handler {
                            h(i, w, cx);
                        }
                    })
                    .child(o.clone())
            });
            root = root.child(deferred(
                anchored().child(
                    div()
                        .id(ElementId::Name(format!("{}:menu", self.id).into()))
                        .role(Role::Menu)
                        .mt(px(26.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.border)
                        .bg(t.surface)
                        .shadow_md()
                        .children(items),
                ),
            ));
        }
        root.into_any_element()
    }
}
