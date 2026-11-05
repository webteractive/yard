//! The Dock icon follows the windows (M6 design §18 Q8).

use gpui::{ActivationPolicy, App};

/// A window opened: the app shows in the Dock and comes to the front.
pub fn window_opened(cx: &mut App) {
    cx.set_activation_policy(ActivationPolicy::Regular);
    cx.activate(true);
}

/// A window closed: with none left the app is a menu bar item again, with no Dock icon.
pub fn window_closed(cx: &mut App) {
    if cx.windows().is_empty() {
        cx.set_activation_policy(ActivationPolicy::Accessory);
    }
}
