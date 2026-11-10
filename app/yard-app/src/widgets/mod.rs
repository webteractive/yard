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
/// A row of controls: centred, spaced, wrapping, and never stretching them across the pane (G7).
pub fn bar() -> gpui::Div {
    use gpui::Styled;
    gpui::div().flex().flex_row().flex_wrap().items_center().gap(gpui::px(8.))
}

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
    use gpui::{
        Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
        TestAppContext, Window, div,
    };
    use yard_app_model::hint::HintAction;
    use yard_proto::{ErrorCode, YardError};

    /// Something clickable that covers the window, under whatever the test draws on top.
    struct Covered<F: Fn() -> gpui::AnyElement + 'static> {
        under: std::rc::Rc<std::cell::Cell<u32>>,
        top: F,
    }

    impl<F: Fn() -> gpui::AnyElement + 'static> Render for Covered<F> {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let under = self.under.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .id("under")
                        .absolute()
                        .inset_0()
                        .on_click(move |_, _, _| under.set(under.get() + 1)),
                )
                .child(div().absolute().top_0().left_0().size_full().child((self.top)()))
        }
    }

    fn click(cx: &mut gpui::VisualTestContext, at: gpui::Point<gpui::Pixels>) {
        cx.simulate_click(at, gpui::Modifiers::none());
        cx.run_until_parked();
    }

    /// A sheet is modal: a click on its backdrop or its blank space never reaches the controls
    /// under it (M6 final review: a click in the Port sheet could press Stop on the row beneath).
    #[gpui::test]
    async fn a_sheet_keeps_clicks_from_what_is_under_it(cx: &mut TestAppContext) {
        let under = std::rc::Rc::new(std::cell::Cell::new(0));
        let u = under.clone();
        let (_v, cx) = cx.add_window_view(move |_, _| Covered {
            under: u.clone(),
            top: || Form::new("sheet", "A sheet").into_any_element(),
        });
        cx.run_until_parked();
        click(cx, gpui::point(gpui::px(4.), gpui::px(4.)));
        let sheet = cx.debug_bounds("sheet").expect("the sheet is drawn");
        click(cx, sheet.origin + gpui::point(gpui::px(8.), gpui::px(40.)));
        assert_eq!(under.get(), 0, "a click reached the control under the sheet");
    }

    /// Escape cancels a sheet and Enter presses its default button, as in a macOS sheet (M6 final
    /// review: only the mouse closed one).
    #[gpui::test]
    async fn escape_cancels_a_sheet_and_enter_submits_it(cx: &mut TestAppContext) {
        cx.update(Form::bind_keys);
        let (submitted, cancelled) =
            (std::rc::Rc::new(std::cell::Cell::new(0)), std::rc::Rc::new(std::cell::Cell::new(0)));
        let (s, c) = (submitted.clone(), cancelled.clone());
        let (_v, cx) = cx.add_window_view(move |_, _| {
            let (s, c) = (s.clone(), c.clone());
            Covered {
                under: std::rc::Rc::new(std::cell::Cell::new(0)),
                top: move || {
                    let (s, c) = (s.clone(), c.clone());
                    Form::new("sheet", "A sheet")
                        .submit("OK", false, move |_, _, _| s.set(s.get() + 1))
                        .on_cancel(move |_, _, _| c.set(c.get() + 1))
                        .into_any_element()
                },
            }
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!((submitted.get(), cancelled.get()), (1, 0));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!((submitted.get(), cancelled.get()), (1, 1));
    }

    /// A click anywhere outside an open picker closes its popup, as a macOS pop-up button does.
    #[gpui::test]
    async fn a_click_outside_a_picker_closes_it(cx: &mut TestAppContext) {
        let (_v, cx) = cx.add_window_view(move |_, _| Covered {
            under: std::rc::Rc::new(std::cell::Cell::new(0)),
            top: || {
                div()
                    .p(gpui::px(200.))
                    .child(Picker::new("pick", "PHP", vec!["7.1".into(), "8.4".into()], Some(0)))
                    .into_any_element()
            },
        });
        cx.run_until_parked();
        let picker = cx.debug_bounds("pick").expect("the picker is drawn");
        click(cx, picker.center());
        assert!(cx.debug_bounds("pick:option:1").is_some(), "the popup is open");
        click(cx, gpui::point(gpui::px(4.), gpui::px(4.)));
        assert!(cx.debug_bounds("pick:option:1").is_none(), "the popup is still open");
        // Its own button still opens and closes it.
        click(cx, picker.center());
        assert!(cx.debug_bounds("pick:option:1").is_some(), "the button opens it again");
        click(cx, picker.center());
        assert!(cx.debug_bounds("pick:option:1").is_none(), "the button closes it");
    }

    /// Choosing from a picker's popup chooses, and presses nothing under the popup (M6 final
    /// review: picking a site's PHP could press Detect docroot below it).
    #[gpui::test]
    async fn a_picker_option_keeps_its_click(cx: &mut TestAppContext) {
        let under = std::rc::Rc::new(std::cell::Cell::new(0));
        let chosen = std::rc::Rc::new(std::cell::Cell::new(None));
        let (u, c) = (under.clone(), chosen.clone());
        let (_v, cx) = cx.add_window_view(move |_, _| {
            let c = c.clone();
            Covered {
                under: u.clone(),
                top: move || {
                    let c = c.clone();
                    Picker::new("pick", "PHP", vec!["7.1".into(), "8.4".into()], Some(0))
                        .on_select(move |i, _, _| c.set(Some(i)))
                        .into_any_element()
                },
            }
        });
        cx.run_until_parked();
        let picker = cx.debug_bounds("pick").expect("the picker is drawn");
        click(cx, picker.center());
        let option = cx.debug_bounds("pick:option:1").expect("the popup is open");
        let before = under.get();
        click(cx, option.center());
        assert_eq!(chosen.get(), Some(1));
        assert_eq!(under.get(), before, "the option's click reached the control under it");
    }

    /// A table is a list VoiceOver can walk: AccessKit gives a macOS table no rows, so VoiceOver
    /// read every table as "empty" and could not reach a row (found with VoiceOver, 2026-11-07).
    #[test]
    fn a_table_is_a_list_voiceover_can_walk() {
        let t = Table::new("sites", vec!["Name".into(), "URL".into()], rows(&["blog"]));
        assert_eq!(t.a11y().role, Role::List);
        assert_eq!(t.a11y().label.as_ref(), "Name, URL");
    }

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

    /// VoiceOver hears what Copy copies: the hint when there is one, else the message (M6 final
    /// review: the hint was plain text, so Copy said nothing of what it took).
    #[test]
    fn copy_says_what_it_copies() {
        let with_hint = YardError::new(ErrorCode::Internal, "boom").with_hint("yard trust");
        let copy = ErrorView::new("e", &with_hint, HintAction::parse("yard trust")).copy_button().a11y();
        assert_eq!(copy.description.as_deref(), Some("copies yard trust"));
        let bare = YardError::new(ErrorCode::Internal, "boom");
        let copy = ErrorView::new("e", &bare, HintAction::Copy("boom".into())).copy_button().a11y();
        assert_eq!(copy.description.as_deref(), Some("copies boom"));
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

    /// A field in a row of controls beside a button.
    struct FieldRow {
        field: gpui::Entity<TextField>,
    }

    impl Render for FieldRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(super::bar().child(self.field.clone()).child(Button::new("after", "Install")))
        }
    }

    fn field_row<'a>(cx: &'a mut TestAppContext, value: &str) -> &'a mut gpui::VisualTestContext {
        let value = value.to_string();
        let (_view, vcx) = cx.add_window_view(move |_, cx| {
            let field = gpui::AppContext::new(cx, |cx| {
                let mut f = TextField::new("spec", "Node version", "22", cx);
                f.set_value(value.clone(), cx);
                f
            });
            FieldRow { field }
        });
        vcx.run_until_parked();
        vcx
    }

    /// A field in a row keeps its width rather than collapsing to its padding (G5).
    #[gpui::test]
    async fn a_field_in_a_row_keeps_its_width(cx: &mut TestAppContext) {
        let vcx = field_row(cx, "");
        let field = vcx.debug_bounds("spec").expect("the field is drawn");
        assert_eq!(field.size.width, gpui::px(180.));
    }

    /// A long value stays inside its field: the button after it starts at or past its right edge.
    #[gpui::test]
    async fn a_long_value_stays_inside_the_field(cx: &mut TestAppContext) {
        let vcx = field_row(cx, &"x".repeat(200));
        let field = vcx.debug_bounds("spec").expect("the field is drawn");
        let after = vcx.debug_bounds("after").expect("the button is drawn");
        assert!(
            after.left() >= field.right(),
            "button at {:?}, field ends at {:?}",
            after.left(),
            field.right()
        );
        assert_eq!(field.size.width, gpui::px(180.));
    }
}
