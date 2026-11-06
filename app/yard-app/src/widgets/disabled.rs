//! A control assistive technology reports as unavailable. GPUI's builder has no disabled state, so
//! a disabled button or picker would read as enabled and do nothing when pressed; this element
//! wraps one and marks its accessibility node disabled, leaving layout and painting to it.

use gpui::{
    A11ySubtreeBuilder, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, accesskit,
};

pub struct Disabled<E>(pub E);

impl<E: Element> IntoElement for Disabled<E> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<E: Element> Element for Disabled<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.0.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.0.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.0.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.0.prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(id, inspector_id, bounds, request_layout, prepaint, window, cx)
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.0.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.0.write_a11y_info(node);
        node.set_disabled();
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        self.0.a11y_synthetic_children(prepaint, builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{InteractiveElement, Role, StatefulInteractiveElement, div};

    #[test]
    fn a_disabled_control_keeps_its_role_and_label_and_says_it_is_disabled() {
        let control = || div().id("php").role(Role::ComboBox).aria_label("PHP for chk-c");
        let mut node = accesskit::Node::new(Role::ComboBox);
        Element::write_a11y_info(&Disabled(control()), &mut node);
        assert!(node.is_disabled());
        assert_eq!(node.label(), Some("PHP for chk-c"));
        assert_eq!(Disabled(control()).a11y_role(), Some(Role::ComboBox));

        let mut plain = accesskit::Node::new(Role::ComboBox);
        Element::write_a11y_info(&control(), &mut plain);
        assert!(!plain.is_disabled());
    }
}
