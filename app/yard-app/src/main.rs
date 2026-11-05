//! Yard.app: a menu bar app and window for yardd (M6). With `--root <path>`, a window shows the
//! version of the yardd serving that data root, or why it cannot be reached.

mod bridge;
#[expect(dead_code, reason = "reconnect_now and logs: the shell and the Logs screen (M6 Tasks 13 and 15)")]
mod daemon;
mod menubar;
#[cfg_attr(not(test), expect(dead_code, reason = "the screens run their actions through it from M6 Task 13"))]
mod runner;
mod state;
mod testing;

use gpui::{
    ActivationPolicy, App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};
use std::path::PathBuf;
use std::sync::Arc;
use yard_client::session::{ConnectionState, SessionConfig};

/// What `hello` says this client is.
pub fn client_name() -> String {
    format!("Yard.app {} ({})", env!("CARGO_PKG_VERSION"), env!("YARD_BUILD_COMMIT"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.iter().position(|a| a == "--root").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let bridge = Arc::new(bridge::Bridge::new());

    gpui_platform::application().run(move |cx: &mut App| {
        // GPUI drops this closure once it returns, and with it anything only the closure holds:
        // the runtime under every session must live as long as the app (found 2026-11-06, when
        // a session stopped noticing yardd the moment start-up finished).
        cx.set_global(bridge::AppBridge(bridge.clone()));
        cx.set_activation_policy(ActivationPolicy::Accessory);
        if let Err(e) = menubar::install(cx) {
            eprintln!("Yard.app: no menu bar item: {e}");
        }
        if let Some(root) = root.clone() {
            let bridge = cx.global::<bridge::AppBridge>().0.clone();
            let bounds = Bounds::centered(None, size(px(420.), px(140.)), cx);
            let options =
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
            let opened = cx.open_window(options, |_, cx| cx.new(|cx| VersionView::new(&root, &bridge, cx)));
            if let Err(e) = opened {
                eprintln!("Yard.app: no window: {e}");
            }
            cx.activate(true);
        }
    });
}

/// The daemon a data root's yardd reports, or why it is not answering, read from `AppState`.
struct VersionView {
    state: Entity<state::AppState>,
}

impl VersionView {
    fn new(root: &std::path::Path, bridge: &Arc<bridge::Bridge>, cx: &mut Context<Self>) -> Self {
        let config = SessionConfig::new(root.join("yardd.sock"), client_name());
        let daemon = daemon::SessionDaemon::start(config, bridge.clone());
        let state = state::AppState::new(daemon, std::time::Duration::from_millis(150), cx);
        cx.observe(&state, |this: &mut VersionView, _, cx| {
            // The window's text, also on stderr, so a session can be followed without a screen.
            eprintln!("Yard.app: {}", this.text(cx));
            cx.notify();
        })
        .detach();
        VersionView { state }
    }

    fn text(&self, cx: &App) -> String {
        match &self.state.read(cx).connection {
            ConnectionState::Connecting => "connecting to yardd…".to_string(),
            ConnectionState::Connected(info) => {
                format!("yardd {} ({})", info.hello.daemon_version, info.hello.commit)
            }
            ConnectionState::Disconnected(e) | ConnectionState::Incompatible(e) => e.message.clone(),
        }
    }
}

impl Render for VersionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_lg()
            .child(SharedString::from(self.text(cx)))
    }
}
