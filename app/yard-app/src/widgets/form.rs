//! A modal sheet over the window: a title, fields, Cancel and a default button.

use super::A11y;
use super::button::{Button, ButtonStyle};
use super::theme::Theme;
use super::theme::Typography;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, IntoElement, KeyBinding, ParentElement, RenderOnce,
    Role, SharedString, Styled, Window, actions, div, prelude::*, px,
};
use std::rc::Rc;

actions!(form, [Submit, Cancel]);

/// The key context a sheet's keys are bound in.
const CONTEXT: &str = "Form";

type Handler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Form {
    id: SharedString,
    title: SharedString,
    fields: Vec<AnyElement>,
    submit: SharedString,
    cancel: SharedString,
    submit_disabled: bool,
    /// The last error of what the sheet does, shown above its buttons.
    error: Option<SharedString>,
    on_submit: Option<Handler>,
    on_cancel: Option<Handler>,
}

impl Form {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Form {
        Form {
            id: id.into(),
            title: title.into(),
            fields: vec![],
            submit: "OK".into(),
            cancel: "Cancel".into(),
            submit_disabled: false,
            error: None,
            on_submit: None,
            on_cancel: None,
        }
    }

    pub fn field(mut self, element: impl IntoElement) -> Self {
        self.fields.push(element.into_any_element());
        self
    }

    pub fn submit(
        mut self,
        label: impl Into<SharedString>,
        disabled: bool,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.submit = label.into();
        self.submit_disabled = disabled;
        self.on_submit = Some(Rc::new(f));
        self
    }

    pub fn error(mut self, error: Option<SharedString>) -> Self {
        self.error = error;
        self
    }

    /// The cancel button's label (`Not now` where cancelling leaves something undone).
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel = label.into();
        self
    }

    pub fn on_cancel(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y::new(self.id.clone(), Role::Dialog, self.title.clone())
    }

    /// Enter presses the default button and Escape cancels, as in a macOS sheet.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("enter", Submit, Some(CONTEXT)),
            KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        ]);
    }
}

impl RenderOnce for Form {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let mut cancel = Button::new(format!("{}:cancel", self.id), self.cancel);
        if let Some(h) = self.on_cancel.clone() {
            cancel = cancel.on_click(move |e, w, cx| h(e, w, cx));
        }
        let mut submit = Button::new(format!("{}:submit", self.id), self.submit)
            .style(ButtonStyle::Primary)
            .disabled(self.submit_disabled);
        let on_submit = self.on_submit.clone().filter(|_| !self.submit_disabled);
        if let Some(h) = self.on_submit {
            submit = submit.on_click(move |e, w, cx| h(e, w, cx));
        }
        // The sheet holds the keyboard while it is open, so Enter and Escape reach it; a field
        // inside it keeps the focus it was given.
        let focus =
            window.use_keyed_state(ElementId::Name(format!("{}:focus", a11y.id).into()), cx, |_, cx| {
                cx.focus_handle()
            });
        let focus: FocusHandle = focus.read(cx).clone();
        if !focus.contains_focused(window, cx) {
            window.focus(&focus, cx);
        }
        let on_cancel = self.on_cancel;
        // Modal: occluded, so no click on the backdrop or the sheet reaches what is under it.
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::hsla(0., 0., 0., 0.35))
            .child(
                div()
                    .id(ElementId::Name(a11y.id.clone()))
                    .track_focus(&focus)
                    .key_context(CONTEXT)
                    .on_action(move |_: &Submit, w, cx| {
                        if let Some(h) = &on_submit {
                            h(&ClickEvent::default(), w, cx);
                        }
                    })
                    .on_action(move |_: &Cancel, w, cx| {
                        if let Some(h) = &on_cancel {
                            h(&ClickEvent::default(), w, cx);
                        }
                    })
                    .debug_selector(|| a11y.id.to_string())
                    .role(Role::Dialog)
                    .aria_label(a11y.label)
                    .w(px(440.))
                    .p(px(16.))
                    .gap(px(10.))
                    .flex()
                    .flex_col()
                    .rounded(px(10.))
                    .bg(t.surface)
                    .shadow_lg()
                    .child(div().type_section().text_color(t.text).child(self.title))
                    .children(self.fields)
                    .children(self.error.map(|e| {
                        div()
                            .type_body()
                            .text_color(t.danger)
                            .child(super::text(format!("{}:error", a11y.id), e))
                    }))
                    .child(div().flex().justify_end().gap(px(8.)).child(cancel).child(submit)),
            )
    }
}
