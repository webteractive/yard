//! An error beside its control: the message, the hint as a button when the app can act on it,
//! else Copy, and the output that came with it under a disclosure (M6 design §10).

use super::A11y;
use super::button::Button;
use super::log_view::LogView;
use super::theme::{Theme, Typography, radius, space};
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
    /// Dismiss: the action whose error this is, and what clears it.
    dismiss: Option<(SharedString, DismissHandler)>,
    /// One line, for the issues strip.
    compact: bool,
}

type DismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

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
            dismiss: None,
            compact: false,
        }
    }

    /// Offers Dismiss (id `dismiss:<action>`), which runs `f`.
    pub fn dismissible(
        mut self,
        action: impl Into<SharedString>,
        f: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.dismiss = Some((action.into(), Rc::new(f)));
        self
    }

    /// One line: the message and its buttons, for the issues strip.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
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
        let copy = self.copy_button().small();
        let open =
            window.use_keyed_state(ElementId::Name(format!("{}:details", self.id).into()), cx, |_, _| false);
        let show = *open.read(cx);
        let mut row = div().flex().flex_none().gap(space::XS).items_center();
        if buttons.len() == 2 {
            let action = self.action.clone();
            let handler = self.on_action.clone();
            row = row.child(Button::new(format!("{}:act", self.id), buttons[0].clone()).small().on_click(
                move |_, w, cx| {
                    if let Some(h) = &handler {
                        h(action.clone(), w, cx);
                    }
                },
            ));
        }
        row = row.child(copy);
        if !self.details.is_empty() && !self.compact {
            row = row.child(
                Button::new(format!("{}:more", self.id), if show { "Hide details" } else { "Details" })
                    .small()
                    .on_click(move |_, _, cx| {
                        open.update(cx, |o, cx| {
                            *o = !*o;
                            cx.notify();
                        })
                    }),
            );
        }
        if let Some((action, f)) = self.dismiss.clone() {
            row = row.child(
                Button::new(format!("dismiss:{action}"), "Dismiss")
                    .small()
                    .description(format!("dismisses {}", self.message))
                    .on_click(move |_, w, cx| f(w, cx)),
            );
        }
        let message = div()
            .min_w(px(0.))
            .type_body()
            .text_color(t.c.danger_text)
            .when(self.compact, |d| d.flex_1().truncate())
            .child(super::text(format!("{}:message", a11y.id), self.message));
        let body = if self.compact {
            div().flex().items_center().gap(space::SM).min_w(px(0.)).child(message).child(row)
        } else {
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .min_w(px(0.))
                .child(message)
                .when_some(self.hint.clone(), |d, h| {
                    d.child(
                        div()
                            .type_mono()
                            .text_color(t.c.text_secondary)
                            .child(super::text(format!("{}:hint", a11y.id), h)),
                    )
                })
                .child(row)
                .when(show, |d| d.child(LogView::new(format!("{}:log", a11y.id), self.details)))
        };
        div()
            .id(ElementId::Name(a11y.id.clone()))
            .role(Role::Alert)
            .aria_label(a11y.label)
            .min_w(px(0.))
            .px(space::SM)
            .py(if self.compact { px(5.) } else { space::SM })
            .rounded(radius::MD)
            .border_l(px(3.))
            .border_color(t.c.danger)
            .bg(Theme::wash(t.c.danger, 0.10))
            .child(body)
    }
}
