//! A line across the top of the window: offline, stale, or a config that does not parse.

use super::A11y;
use super::theme::Theme;
use gpui::{
    AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, Styled, Window,
    div, prelude::*, px,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BannerKind {
    Info,
    Warning,
    Error,
}

#[derive(IntoElement)]
pub struct Banner {
    id: SharedString,
    kind: BannerKind,
    text: SharedString,
    actions: Vec<AnyElement>,
}

impl Banner {
    pub fn new(id: impl Into<SharedString>, kind: BannerKind, text: impl Into<SharedString>) -> Banner {
        Banner { id: id.into(), kind, text: text.into(), actions: vec![] }
    }

    pub fn action(mut self, button: impl IntoElement) -> Self {
        self.actions.push(button.into_any_element());
        self
    }

    pub fn a11y(&self) -> A11y {
        let role = if self.kind == BannerKind::Info { Role::Status } else { Role::Alert };
        A11y::new(self.id.clone(), role, self.text.clone())
    }
}

impl RenderOnce for Banner {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let t = Theme::of(window);
        let a11y = self.a11y();
        let color = match self.kind {
            BannerKind::Info => t.accent,
            BannerKind::Warning => t.warning,
            BannerKind::Error => t.danger,
        };
        div()
            .id(ElementId::Name(a11y.id))
            .role(a11y.role)
            .aria_label(a11y.label)
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .bg(Theme::wash(color, 0.14))
            .border_b_1()
            .border_color(color)
            .child(div().flex_1().text_sm().text_color(t.text).child(self.text))
            .children(self.actions)
    }
}
