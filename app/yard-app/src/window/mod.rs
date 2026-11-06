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
