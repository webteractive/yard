//! A modal sheet over the window: a title, fields, Cancel and a default button.

use super::A11y;
use super::button::{Button, ButtonStyle};
use super::theme::Theme;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString,
    Styled, Window, div, prelude::*, px,
};
use std::rc::Rc;

type Handler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Form {
    id: SharedString,
    title: SharedString,
    fields: Vec<AnyElement>,
    submit: SharedString,
    submit_disabled: bool,
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
            submit_disabled: false,
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

    pub fn on_cancel(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }

    pub fn a11y(&self) -> A11y {
        A11y::new(self.id.clone(), Role::Dialog, self.title.clone())
    }
}

impl RenderOnce for Form {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let mut cancel = Button::new(format!("{}:cancel", self.id), "Cancel");
        if let Some(h) = self.on_cancel {
            cancel = cancel.on_click(move |e, w, cx| h(e, w, cx));
        }
        let mut submit = Button::new(format!("{}:submit", self.id), self.submit)
            .style(ButtonStyle::Primary)
            .disabled(self.submit_disabled);
        if let Some(h) = self.on_submit {
            submit = submit.on_click(move |e, w, cx| h(e, w, cx));
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::hsla(0., 0., 0., 0.35))
            .child(
                div()
                    .id(ElementId::Name(a11y.id))
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
                    .child(div().text_lg().text_color(t.text).child(self.title))
                    .children(self.fields)
                    .child(div().flex().justify_end().gap(px(8.)).child(cancel).child(submit)),
            )
    }
}
