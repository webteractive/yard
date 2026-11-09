//! Yard.app: a menu bar app and window for yardd (M6). With `--root <path>`, a window shows the
//! version of the yardd serving that data root, or why it cannot be reached.

mod activity;
mod bridge;
mod daemon;
mod env;
mod menubar;
mod onboarding;
mod overview;
mod platform;
mod runner;
mod screens;
mod services;
mod settings_window;
mod sites;
mod state;
mod testing;
mod widgets;
mod window;

use gpui::{ActivationPolicy, App};
use std::path::PathBuf;
use std::sync::Arc;
use yard_client::session::SessionConfig;

/// What `hello` says this client is.
pub fn client_name() -> String {
    format!("Yard.app {} ({})", env!("CARGO_PKG_VERSION"), env!("YARD_BUILD_COMMIT"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.iter().position(|a| a == "--root").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let bridge = Arc::new(bridge::Bridge::new());

    gpui_platform::application().run(move |cx: &mut App| {
        cx.set_activation_policy(ActivationPolicy::Accessory);
        widgets::text_field::TextField::bind_keys(cx);
        widgets::form::Form::bind_keys(cx);
        platform::terminal::clean_stale(&std::env::temp_dir());
        let settings = platform::settings::Settings::default();
        let location = platform::locator::location(root.clone(), settings.yard_path());
        let socket = yard_core::Paths::new(location.root.clone()).daemon_socket();
        let daemon = daemon::SessionDaemon::start(SessionConfig::new(socket, client_name()), bridge.clone());
        let root_arg = root.clone();
        let locate_yard: env::LocateYard =
            std::rc::Rc::new(move |setting| platform::locator::location(root_arg.clone(), setting).yard);
        let env = env::AppEnv::new(daemon, location, locate_yard, bridge.clone(), cx);
        // GPUI drops this closure once it returns, and with it anything only the closure holds;
        // the session and the CLI runner hold the bridge, and AppEnv holds them as a global, so
        // the runtime under every session lives as long as the app (found 2026-11-06, when a
        // session stopped noticing yardd the moment start-up finished).
        cx.set_global(env.clone());
        cx.on_window_closed(|cx, _| {
            platform::activation::window_closed(cx);
            window::main::closed(&env::AppEnv::global(cx), cx);
        })
        .detach();
        if let Err(e) = menubar::install(&env, cx) {
            eprintln!("Yard.app: no menu bar item: {e}");
        }
        window::main::ask_without_a_window(&env, cx);
        onboarding::open_if_needed(&env, cx);
        // Started for a scratch root: open the window, as a developer running it wants.
        if root.is_some() {
            window::main::open(&env, yard_app_model::hint::Screen::Overview, cx);
        }
    });
}
