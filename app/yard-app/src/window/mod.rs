pub mod main;

use gpui::{App, WindowHandle};

/// Whether `handle`'s window is still open. Read from the app's windows, not by updating the
/// handle: an update fails while the window is busy with its own event, which is no sign it closed.
pub fn is_open<V: 'static>(handle: WindowHandle<V>, cx: &App) -> bool {
    cx.windows().iter().any(|w| w.window_id() == handle.window_id())
}

/// Raises `handle`'s window once the current event is done, so a window can be raised from
/// inside its own handler.
pub fn raise<V: 'static>(handle: WindowHandle<V>, cx: &mut App) {
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    });
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    #[test]
    fn window_titles_name_each_window() {
        let b = Bounds::new(point(px(0.), px(0.)), size(px(900.), px(600.)));
        let main = super::main::window_options(b);
        let setup = crate::onboarding::window_options(b);
        let settings = crate::settings_window::window_options(b);
        let title = |o: &gpui::WindowOptions| {
            o.titlebar.as_ref().and_then(|t| t.title.clone()).map(|t| t.to_string())
        };
        assert_eq!(title(&main).as_deref(), Some("Yard"));
        assert_eq!(title(&setup).as_deref(), Some("Set up Yard"));
        assert_eq!(title(&settings).as_deref(), Some("Yard Settings"));
        assert_eq!(main.window_min_size, Some(size(px(760.), px(480.))));
    }
}
