//! Yard.app: a menu bar app and window for yardd (M6). With `--root <path>`, a window shows the
//! version of the yardd serving that data root, or why it cannot be reached.

mod bridge;
mod menubar;

use gpui::{
    ActivationPolicy, App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, WindowBounds, WindowOptions, div, px, size,
};
use std::path::PathBuf;
use std::sync::Arc;
use yard_client::session::{ConnectionState, Session, SessionConfig, Update};

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

/// The daemon a data root's yardd reports, or why it is not answering.
struct VersionView {
    text: SharedString,
    _session: Session,
}

impl VersionView {
    fn new(root: &std::path::Path, bridge: &bridge::Bridge, cx: &mut Context<Self>) -> Self {
        let _guard = bridge.runtime.enter();
        let config = SessionConfig::new(root.join("yardd.sock"), client_name());
        let (session, mut updates) = Session::start(config);
        cx.spawn(async move |this, cx| {
            while let Some(update) = updates.recv().await {
                let Update::State(state) = update else { continue };
                let text = match state {
                    ConnectionState::Connecting => "connecting to yardd…".to_string(),
                    ConnectionState::Connected(info) => {
                        format!("yardd {} ({})", info.hello.daemon_version, info.hello.commit)
                    }
                    ConnectionState::Disconnected(e) | ConnectionState::Incompatible(e) => e.message,
                };
                // The window's text, also on stderr, so a session can be followed without a screen.
                eprintln!("Yard.app: {text}");
                if this
                    .update(cx, |this, cx| {
                        this.text = text.into();
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        VersionView { text: "connecting to yardd…".into(), _session: session }
    }
}

impl Render for VersionView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().items_center().justify_center().text_lg().child(self.text.clone())
    }
}
