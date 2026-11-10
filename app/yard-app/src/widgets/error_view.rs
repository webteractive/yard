//! An error beside its control: the message, the hint as a button when the app can act on it,
//! else Copy, and the output that came with it under a disclosure (M6 design §10).

use super::A11y;
use super::button::Button;
use super::log_view::LogView;
use super::theme::Theme;
use crate::platform::open::Opener;
use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Window, div,
    prelude::*, px,
};
use std::rc::Rc;
use yard_app_model::hint::HintAction;
use yard_proto::YardError;

#[derive(IntoElement)]
pub struct ErrorView {
    id: SharedString,
    message: SharedString,
    hint: Option<SharedString>,
    action: HintAction,
    details: Vec<SharedString>,
    on_action: Option<super::Callback<HintAction>>,
}

impl ErrorView {
    pub fn new(id: impl Into<SharedString>, e: &YardError, action: HintAction) -> ErrorView {
        let details = e
            .data
            .as_ref()
            .and_then(|d| d.get("tail").or_else(|| d.get("output")))
            .and_then(|v| v.as_array())
            .map(|lines| {
                lines.iter().filter_map(|l| l.as_str()).map(|l| SharedString::from(l.to_string())).collect()
            })
            .unwrap_or_default();
        ErrorView {
            id: id.into(),
            message: e.message.clone().into(),
            hint: e.hint.clone().map(Into::into),
            action,
            details,
            on_action: None,
        }
    }

    /// What runs the hint's action; the shell passes its dispatcher.
    pub fn on_action(mut self, f: impl Fn(HintAction, &mut Window, &mut App) + 'static) -> Self {
        self.on_action = Some(Rc::new(f));
        self
    }

    /// The buttons it offers: the hint when the app can act on it, and Copy.
    pub fn buttons(&self) -> Vec<SharedString> {
        match (&self.action, &self.hint) {
            (HintAction::Copy(_), _) | (_, None) => vec!["Copy".into()],
            (_, Some(hint)) => vec![hint.clone(), "Copy".into()],
        }
    }

    pub fn a11y(&self) -> A11y {
        A11y::new(self.id.clone(), Role::Alert, self.message.clone())
    }

    /// Copy: the hint, else the message, saying which for VoiceOver.
    pub fn copy_button(&self) -> Button {
        let text: String = self.hint.clone().unwrap_or_else(|| self.message.clone()).to_string();
        Button::new(format!("{}:copy", self.id), "Copy")
            .description(format!("copies {text}"))
            .on_click(move |_, _, cx| Opener::copy(cx, &text))
    }
}

impl RenderOnce for ErrorView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let buttons = self.buttons();
        let copy = self.copy_button();
        let open =
            window.use_keyed_state(ElementId::Name(format!("{}:details", self.id).into()), cx, |_, _| false);
        let show = *open.read(cx);
        let mut row = div().flex().gap(px(6.)).items_center();
        if buttons.len() == 2 {
            let action = self.action.clone();
            let handler = self.on_action.clone();
            row = row.child(Button::new(format!("{}:act", self.id), buttons[0].clone()).on_click(
                move |_, w, cx| {
                    if let Some(h) = &handler {
                        h(action.clone(), w, cx);
                    }
                },
            ));
        }
        row = row.child(copy);
        if !self.details.is_empty() {
            row = row.child(
                Button::new(format!("{}:more", self.id), if show { "Hide details" } else { "Details" })
                    .on_click(move |_, _, cx| {
                        open.update(cx, |o, cx| {
                            *o = !*o;
                            cx.notify();
                        })
                    }),
            );
        }
        div()
            .id(ElementId::Name(a11y.id.clone()))
            .role(Role::Alert)
            .aria_label(a11y.label)
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(8.))
            .rounded(px(6.))
            .bg(Theme::wash(t.danger, 0.14))
            .child(
                div()
                    .text_sm()
                    .text_color(t.danger)
                    .child(super::text(format!("{}:message", a11y.id), self.message)),
            )
            .when_some(self.hint.clone(), |d, h| {
                d.child(
                    div().text_xs().text_color(t.muted).child(super::text(format!("{}:hint", a11y.id), h)),
                )
            })
            .child(row)
            .when(show, |d| d.child(LogView::new(format!("{}:log", a11y.id), self.details)))
    }
}
