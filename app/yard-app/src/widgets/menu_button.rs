//! A button that opens a short menu of actions (the site detail's Open in…), drawn as a picker
//! is but choosing nothing: each item runs its own action and keeps its own id and label.

use super::A11y;
use super::Pressable;
use super::picker::{chevron, popup, popup_row};
use super::theme::{Theme, Typography, radius, size};
use gpui::{
    App, Bounds, ElementId, IntoElement, ParentElement, Pixels, RenderOnce, Role, SharedString, Styled,
    Window, canvas, div, prelude::*, px,
};
use std::rc::Rc;

type Action = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct MenuButton {
    id: SharedString,
    label: SharedString,
    items: Vec<(SharedString, SharedString, Action)>,
}

impl MenuButton {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> MenuButton {
        MenuButton { id: id.into(), label: label.into(), items: vec![] }
    }

    /// An item: its id (as the button it replaces had), its label, and what it does.
    pub fn item(
        mut self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        f: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push((id.into(), label.into(), Rc::new(f)));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y::new(self.id.clone(), Role::Button, self.label.clone())
    }
}

impl RenderOnce for MenuButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let open =
            window.use_keyed_state(ElementId::Name(format!("{}:open", self.id).into()), cx, |_, _| false);
        let is_open = *open.read(cx);
        let button =
            window.use_keyed_state(ElementId::Name(format!("{}:bounds", self.id).into()), cx, |_, _| {
                Bounds::<Pixels>::default()
            });
        let a11y = self.a11y();
        let id = self.id.to_string();
        let toggle = open.clone();
        let mut root = div()
            .id(ElementId::Name(a11y.id.clone()))
            .debug_selector(|| id)
            .role(a11y.role)
            .aria_label(a11y.label)
            .aria_expanded(is_open)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(size::CONTROL)
            .px(px(10.))
            .rounded(radius::MD)
            .border_1()
            .border_color(t.c.control_border)
            .bg(t.c.control_bg)
            .type_body()
            .text_color(t.c.text)
            .relative()
            .cursor_pointer()
            .child(self.label.clone())
            .child(chevron(&t))
            .child({
                let button = button.clone();
                canvas(move |bounds, _, cx| button.update(cx, |b, _| *b = bounds), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
            })
            .on_press(move |_, _, cx| {
                toggle.update(cx, |o, cx| {
                    *o = !*o;
                    cx.notify();
                })
            });
        if is_open {
            let rows = self.items.len();
            let items: Vec<_> = self
                .items
                .into_iter()
                .map(|(id, label, f)| {
                    let close = open.clone();
                    popup_row(&t, id.to_string(), id.to_string(), Role::MenuItem)
                        .aria_label(label.clone())
                        .on_press(move |_, w, cx| {
                            close.update(cx, |o, cx| {
                                *o = false;
                                cx.notify();
                            });
                            f(w, cx);
                        })
                        .child(label)
                })
                .collect();
            root = root.child(popup(&self.id, *button.read(cx), rows, open, button, items, window, &t));
        }
        root
    }
}
