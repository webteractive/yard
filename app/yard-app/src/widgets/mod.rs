//! The app's widgets (M6 design §8.5): each sets an id stable across frames, a role and a label,
//! so assistive technology can find, name and operate it.

pub mod banner;
pub mod button;
pub mod checkbox;
pub mod disabled;
pub mod error_view;
pub mod form;
pub mod log_view;
pub mod picker;
pub mod table;
pub mod text_field;
pub mod theme;
pub mod toggle;

use gpui::{App, ElementId, Role, SharedString, Window};
use std::rc::Rc;

/// Text that assistive technology can read. GPUI puts a `Text` with an id in the accessibility
/// tree and never a plain string, so every visible line that is not a control's own label goes
/// through here. `id` must be unique among the elements under the nearest ancestor with an id, and
/// stable across frames, so a refresh does not announce unchanged text again.
pub fn text(id: impl Into<ElementId>, s: impl Into<SharedString>) -> gpui::Text {
    gpui::Text::new(id.into(), s.into())
}

/// A control's press, by mouse or by assistive technology. GPUI turns an accessibility press into
/// a mouse click at the element's centre, which lands nowhere when the element is scrolled out of
/// view, so VoiceOver could not press a control below the fold (found by the checklist script: a
/// long site detail's Copy .env). `on_press` also answers the accessibility press directly.
pub trait Pressable: gpui::StatefulInteractiveElement + Sized {
    fn on_press(self, f: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        let f = Rc::new(f);
        let g = f.clone();
        self.on_click(move |e, w, cx| f(e, w, cx))
            .on_a11y_action(gpui::accesskit::Action::Click, move |_, w, cx| {
                g(&gpui::ClickEvent::default(), w, cx)
            })
    }
}

impl<E: gpui::StatefulInteractiveElement> Pressable for E {}

/// A widget's handler, called with a value (the new state, the chosen index).
pub type Callback<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// A table's handler, called with the chosen row's key.
pub type KeyCallback = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// What a widget tells assistive technology about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct A11y {
    pub id: SharedString,
    pub role: Role,
    pub label: SharedString,
    /// Reported unavailable ([`disabled::Disabled`]): it is greyed out and does nothing.
    pub disabled: bool,
    /// Read after the label and value, such as why a control is disabled.
    pub description: Option<SharedString>,
}

impl A11y {
    pub fn new(id: SharedString, role: Role, label: SharedString) -> A11y {
        A11y { id, role, label, disabled: false, description: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::banner::{Banner, BannerKind};
    use crate::widgets::button::Button;
    use crate::widgets::checkbox::Checkbox;
    use crate::widgets::error_view::ErrorView;
    use crate::widgets::form::Form;
    use crate::widgets::log_view::LogView;
    use crate::widgets::picker::Picker;
    use crate::widgets::table::{Row, Table};
    use crate::widgets::text_field::TextField;
    use crate::widgets::toggle::Toggle;
    use gpui::{Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div};
    use yard_app_model::hint::HintAction;
    use yard_proto::{ErrorCode, YardError};

    fn rows(keys: &[&str]) -> Vec<Row> {
        keys.iter().map(|k| Row::new(*k, vec![SharedString::from(*k), "x".into()])).collect()
    }

    fn widgets() -> Vec<A11y> {
        let error = YardError::new(ErrorCode::Internal, "boom").with_hint("yard doctor");
        vec![
            Button::new("site:blog:open", "Open blog").a11y(),
            Toggle::new("site:blog:secure", "HTTPS for blog", true).a11y(),
            Checkbox::new("service:redis:on", "Redis", false).a11y(),
            Table::new("sites", vec!["Name".into(), "PHP".into()], rows(&["blog"])).a11y(),
            Picker::new("site:blog:php", "PHP for blog", vec!["8.3".into(), "8.4".into()], Some(1)).a11y(),
            Banner::new("banner:stale", BannerKind::Warning, "yardd is older than this app").a11y(),
            ErrorView::new("site:blog:error", &error, HintAction::parse("yard doctor")).a11y(),
            Form::new("form:link", "Link a folder").a11y(),
            LogView::new("logs", vec!["GET /".into()]).a11y(),
        ]
    }

    #[test]
    fn every_widget_exposes_a_role_and_label() {
        for w in widgets() {
            assert!(!w.id.is_empty(), "{w:?}");
            assert!(!w.label.is_empty(), "{w:?}");
            assert!(!matches!(w.role, Role::Unknown | Role::GenericContainer), "{w:?}");
        }
    }

    /// A greyed-out control is reported unavailable, with the reason as its description, so
    /// VoiceOver does not offer a control that does nothing (found by the checklist script: a PHP
    /// picker a committed `.yard` locks read as enabled, its reason as no text at all).
    #[test]
    fn a_disabled_control_says_so_and_why() {
        let locked = Picker::new("site:blog:php", "PHP for blog", vec!["8.4".into()], Some(0))
            .disabled_because(Some("set in /p/.yard".into()))
            .a11y();
        assert!(locked.disabled);
        assert_eq!(locked.description.as_deref(), Some("set in /p/.yard"));
        assert!(Button::new("b", "Install").disabled(true).a11y().disabled);
        let switch =
            Toggle::new("t", "HTTPS for blog", false).disabled_because(Some("set in /p/.yard".into())).a11y();
        assert!(switch.disabled);
        assert_eq!(switch.description.as_deref(), Some("set in /p/.yard"));
        for w in widgets() {
            assert!(!w.disabled && w.description.is_none(), "{w:?}");
        }
    }

    struct Gallery;

    impl Render for Gallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let error = YardError::new(ErrorCode::Internal, "boom").with_hint("rm -rf ~/x");
            div()
                .size_full()
                .child(Button::new("b", "Open"))
                .child(Toggle::new("t", "HTTPS", true))
                .child(Checkbox::new("c", "Redis", false))
                .child(Table::new("tbl", vec!["Name".into()], rows(&["a", "b"])))
                .child(Banner::new("bn", BannerKind::Error, "broken"))
                .child(ErrorView::new("e", &error, HintAction::parse("rm -rf ~/x")))
                .child(LogView::new("l", vec!["see https://example.com".into()]))
        }
    }

    #[gpui::test]
    async fn every_widget_draws(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| Gallery);
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn text_field_edits_and_reports_its_value(cx: &mut TestAppContext) {
        cx.update(TextField::bind_keys);
        let (field, cx) = cx.add_window_view(|_, cx| TextField::new("name", "Site name", "blog", cx));
        cx.update(|window, cx| {
            let handle = gpui::Focusable::focus_handle(field.read(cx), cx);
            window.focus(&handle, cx);
        });
        cx.simulate_input("shopx");
        cx.simulate_keystrokes("backspace");
        assert_eq!(field.read_with(cx, |f, _| f.value().to_string()), "shop");
        let a11y = field.read_with(cx, |f, _| f.a11y());
        assert_eq!((a11y.role, a11y.label.as_ref()), (Role::TextInput, "Site name"));
        field.update(cx, |f, cx| f.set_value("blog", cx));
        assert_eq!(field.read_with(cx, |f, _| f.value().to_string()), "blog");
    }

    #[test]
    fn table_keeps_row_ids_across_refreshes() {
        let before = Table::new("sites", vec!["Name".into()], rows(&["blog", "shop"]));
        let after = Table::new("sites", vec!["Name".into()], rows(&["api", "shop", "blog"]));
        let id_of = |t: &Table, key: &str| t.row_ids().into_iter().find(|(k, _)| k == key).map(|(_, id)| id);
        assert_eq!(
            id_of(&before, "blog"),
            id_of(&after, "blog"),
            "a row's id comes from its key, not its place"
        );
        assert_ne!(id_of(&after, "blog"), id_of(&after, "shop"));
    }

    #[test]
    fn error_view_offers_copy_for_an_unknown_hint() {
        let e = YardError::new(ErrorCode::Internal, "x").with_hint("rm -rf ~/x");
        assert_eq!(ErrorView::new("e", &e, HintAction::parse("rm -rf ~/x")).buttons(), ["Copy"]);
        let start =
            YardError::new(ErrorCode::DaemonNotRunning, "yardd is not running").with_hint("yard start");
        assert_eq!(
            ErrorView::new("e", &start, HintAction::parse("yard start")).buttons(),
            ["yard start", "Copy"]
        );
    }
}
