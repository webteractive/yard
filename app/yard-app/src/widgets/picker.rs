//! A choice of one value from a short list, shown in a popup under it (above it near the
//! window's bottom).

use super::A11y;
use super::Pressable;
use super::disabled::Disabled;
use super::theme::{Theme, Typography, radius, size};
use gpui::{
    Anchor, AnchoredPositionMode, App, Bounds, ElementId, Entity, IntoElement, ParentElement, Pixels,
    RenderOnce, Role, SharedString, Styled, Window, anchored, canvas, deferred, div, point, prelude::*, px,
};
use std::rc::Rc;

/// An option's row in the popup; the popup adds its 4 pt padding and 1 pt border on each side.
const OPTION_HEIGHT: Pixels = px(24.);

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
        // Where the button is, so a click on it is left to its toggle rather than closing the
        // popup as a click elsewhere does.
        let button =
            window.use_keyed_state(ElementId::Name(format!("{}:bounds", self.id).into()), cx, |_, _| {
                Bounds::<Pixels>::default()
            });
        let current = self.selected.and_then(|i| self.options.get(i)).cloned().unwrap_or_else(|| "—".into());
        let disabled = self.disabled.clone();
        let toggle = open.clone();
        let a11y = self.a11y();
        let id = self.id.to_string();
        let mut root = div()
            .id(ElementId::Name(a11y.id.clone()))
            .debug_selector(|| id)
            .role(a11y.role)
            .aria_label(a11y.label)
            .aria_value(current.clone())
            .aria_expanded(is_open)
            .when_some(a11y.description.clone(), |d, why| d.aria_description(why))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .h(size::CONTROL)
            .min_w(size::PICKER_MIN)
            .px(px(8.))
            .rounded(radius::MD)
            .border_1()
            .border_color(t.c.control_border)
            .bg(t.c.control_bg)
            .type_body()
            .text_color(t.c.text)
            .relative()
            .child(div().min_w(px(0.)).truncate().child(current))
            .child(chevron(&t))
            .child({
                let button = button.clone();
                canvas(move |bounds, _, cx| button.update(cx, |b, _| *b = bounds), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
            });
        if let Some(why) = &disabled {
            root = root
                .opacity(0.6)
                .child(div().type_secondary().text_color(t.c.text_secondary).child(why.clone()));
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
                let selector = format!("{}:option:{i}", self.id);
                popup_row(&t, format!("{}:option:{o}", self.id), selector, Role::MenuItem)
                    .aria_label(o.clone())
                    .aria_selected(self.selected == Some(i))
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_color(t.c.accent)
                            .child(if self.selected == Some(i) { "✓" } else { "" }),
                    )
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
            let items: Vec<_> = items.collect();
            root = root.child(popup(
                &self.id,
                *button.read(cx),
                self.options.len(),
                open,
                button,
                items,
                window,
                &t,
            ));
        }
        root.into_any_element()
    }
}

/// An item's row in a popup: its id, the selector the tests find it by, and its hover.
pub(super) fn popup_row(t: &Theme, id: String, selector: String, role: Role) -> gpui::Stateful<gpui::Div> {
    div()
        .id(ElementId::Name(id.into()))
        .debug_selector(|| selector)
        .role(role)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(OPTION_HEIGHT)
        .px(px(8.))
        .rounded(radius::SM)
        .cursor_pointer()
        .hover(|s| s.bg(t.c.row_selected))
}

/// The popup of the control drawn at `inner` (its bounds inside its 1 px border), holding `rows`
/// rows: under the control when it fits there, else above it. It takes its own clicks, and a click
/// anywhere but the control closes it, as a macOS pop-up button does.
#[allow(clippy::too_many_arguments)]
pub(super) fn popup(
    id: &str,
    inner: Bounds<Pixels>,
    rows: usize,
    open: Entity<bool>,
    button: Entity<Bounds<Pixels>>,
    items: impl IntoIterator<Item = impl IntoElement>,
    window: &Window,
    t: &Theme,
) -> impl IntoElement {
    // The popup lines up with the control's border.
    let at = Bounds::new(
        point(inner.left() - px(1.), inner.top() - px(1.)),
        gpui::size(inner.size.width + px(2.), inner.size.height + px(2.)),
    );
    // Under the control when the popup fits there, else above it: anchored's own flip turns about
    // the popup's corner, which put it over its control near the window's bottom.
    let height = OPTION_HEIGHT * rows as f32 + px(10.);
    let fits_below = at.bottom() + px(2.) + height <= window.viewport_size().height;
    let placed = anchored().position_mode(AnchoredPositionMode::Window);
    let placed = if fits_below {
        placed.position(point(at.left(), at.bottom() + px(2.)))
    } else {
        placed.anchor(Anchor::BottomLeft).position(point(at.left(), at.top() - px(2.)))
    };
    let selector = format!("{id}:menu");
    deferred(
        placed.child(
            div()
                .id(ElementId::Name(selector.clone().into()))
                .debug_selector(move || selector)
                .role(Role::Menu)
                .occlude()
                .on_mouse_down_out(move |e, _, cx| {
                    if button.read(cx).contains(&e.position) {
                        return;
                    }
                    open.update(cx, |o, cx| {
                        *o = false;
                        cx.notify();
                    })
                })
                .min_w(at.size.width)
                .p(px(4.))
                .rounded(radius::XL)
                .border_1()
                .border_color(t.c.separator)
                .bg(t.c.surface)
                .type_body()
                .text_color(t.c.text)
                .shadow_md()
                .children(items),
        ),
    )
}

/// The ⌄ beside a picker's or a menu button's label. The glyph sits low in its line box, so it is
/// raised to centre on the label.
pub(super) fn chevron(t: &Theme) -> gpui::Div {
    div().relative().top(px(-3.)).text_color(t.c.text_secondary).child("⌄")
}
