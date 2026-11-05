//! The main window (M6 design §8.2): a sidebar of screens with badges, a banner when yardd is
//! offline, stale or its config does not parse, and the chosen screen.

use crate::env::AppEnv;
use crate::menubar::link;
use crate::platform::activation;
use crate::runner::ActionId;
use crate::widgets::banner::{Banner, BannerKind};
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::theme::Theme;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, ElementId, Global, IntoElement, ParentElement, Render,
    Role, SharedString, Styled, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, px, size,
};
use yard_app_model::health::Link;
use yard_app_model::hint::Screen;
use yard_app_model::refresh::ResourceKey;
use yard_client::api;
use yard_proto::methods::{ServiceParams, XdebugParams};
use yard_proto::{PortStatus, ServiceState};

struct MainHandle(WindowHandle<MainWindow>);

impl Global for MainHandle {}

pub struct MainWindow {
    env: AppEnv,
}

impl MainWindow {
    fn new(env: AppEnv, cx: &mut Context<Self>) -> MainWindow {
        cx.observe(&env.state, |_, _, cx| cx.notify()).detach();
        cx.observe(&env.runner, |_, _, cx| cx.notify()).detach();
        cx.observe(&env.nav, |_, _, cx| cx.notify()).detach();
        MainWindow { env }
    }
}

/// The lists a screen shows: fetched when it opens and when the window becomes active.
pub fn visible_keys(screen: &Screen) -> Vec<ResourceKey> {
    match screen {
        Screen::Overview => vec![ResourceKey::Status],
        Screen::Sites(Some(site)) => vec![ResourceKey::Sites, ResourceKey::SiteDetail(site.clone())],
        Screen::Sites(None) => vec![ResourceKey::Sites],
        Screen::Services(_) => vec![ResourceKey::Services],
        Screen::Php => vec![ResourceKey::Php],
        Screen::Node => vec![ResourceKey::Node],
        Screen::Tunnels => vec![ResourceKey::Tunnels, ResourceKey::TunnelDomains, ResourceKey::Sites],
        Screen::Ports => vec![ResourceKey::Ports],
        Screen::Sources => vec![ResourceKey::Sources],
        Screen::Logs(_) | Screen::Doctor | Screen::Activity => vec![],
    }
}

/// Shows the main window on `screen`, opening it if it is closed.
pub fn open(env: &AppEnv, screen: Screen, cx: &mut App) {
    let keys = visible_keys(&screen);
    env.nav.update(cx, |n, cx| {
        // The Logs screen's tail holds a connection; it closes when the screen is left.
        if !matches!(screen, Screen::Logs(_)) {
            n.logs = None;
        }
        n.screen = screen;
        cx.notify();
    });
    env.state.update(cx, |s, cx| s.became_active(&keys, cx));
    if let Some(handle) = cx.try_global::<MainHandle>().map(|h| h.0)
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        activation::window_opened(cx);
        return;
    }
    let bounds = Bounds::centered(None, size(px(980.), px(660.)), cx);
    let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
    let env = env.clone();
    if let Ok(handle) = cx.open_window(options, |_, cx| cx.new(|cx| MainWindow::new(env, cx))) {
        cx.set_global(MainHandle(handle));
        activation::window_opened(cx);
    }
}

const PAGES: [(&str, Screen); 11] = [
    ("Overview", Screen::Overview),
    ("Sites", Screen::Sites(None)),
    ("Services", Screen::Services(None)),
    ("PHP", Screen::Php),
    ("Node", Screen::Node),
    ("Tunnels", Screen::Tunnels),
    ("Ports", Screen::Ports),
    ("Logs", Screen::Logs(None)),
    ("Doctor", Screen::Doctor),
    ("Sources", Screen::Sources),
    ("Activity", Screen::Activity),
];

fn same_page(a: &Screen, b: &Screen) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Starts or stops a service from a menu item or a row.
pub fn service_action(env: &AppEnv, name: &str, verb: &str, cx: &mut App) {
    let svc = env.state.read(cx).services.value.iter().flatten().find(|s| s.name == name).cloned();
    let Some(svc) = svc else { return };
    if verb == "console" {
        if let Some(url) = svc.console {
            cx.open_url(&url);
        }
        return;
    }
    let daemon = env.daemon.clone();
    let running = svc.state == ServiceState::Running;
    let id = ActionId(format!("service:{name}:{}", if running { "stop" } else { "start" }));
    let params = ServiceParams { name: name.to_string() };
    env.runner.update(cx, |r, cx| {
        r.perform(
            id,
            async move {
                let m = if running { api::SERVICES_STOP } else { api::SERVICES_START };
                crate::daemon::call(&*daemon, m, params).await.map(|_| ())
            },
            cx,
        )
        .detach()
    });
}

/// Turns xdebug on or off for PHP `v`, then refetches its extensions.
pub fn xdebug(env: &AppEnv, v: &str, on: bool, cx: &mut App) {
    let daemon = env.daemon.clone();
    let state = env.state.clone();
    let php = v.to_string();
    env.runner.update(cx, |r, cx| {
        let task = r.perform(
            ActionId(format!("xdebug:{php}")),
            {
                let php = php.clone();
                async move {
                    crate::daemon::call(&*daemon, api::XDEBUG_SET, XdebugParams { on, php: Some(php) })
                        .await
                        .map(|_| ())
                }
            },
            cx,
        );
        cx.spawn(async move |_, cx| {
            task.await;
            state.update(cx, |s, cx| s.refresh([ResourceKey::Extensions(php)], cx));
        })
        .detach();
    });
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(window);
        let env = self.env.clone();
        let screen = env.nav.read(cx).screen.clone();
        let sheet = sheet(&env, window, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .bg(t.background)
            .text_color(t.text)
            .child(sidebar(&env, &screen, window, cx))
            .child(
                div().flex_1().flex().flex_col().children(banner(&env, cx)).child(
                    div()
                        .id("content")
                        .flex_1()
                        .overflow_y_scroll()
                        .p(px(16.))
                        .child(content(&env, &screen, window, cx)),
                ),
            )
            .children(sheet)
    }
}

/// The open sheet, drawn over the window. A port or settle request from a hint opens its sheet.
fn sheet(env: &AppEnv, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if let Some((owner, true)) = env.nav.read(cx).edit_port.clone() {
        let port = cx.new(|cx| {
            crate::widgets::text_field::TextField::new(
                format!("sheet:port:{owner}"),
                format!("Port for {owner}"),
                "a free port",
                cx,
            )
        });
        env.nav.update(cx, |n, _| {
            n.edit_port = None;
            n.sheet = Some(crate::env::Sheet::Port { name: owner, port });
        });
    }
    if let Some(site) = env.nav.read(cx).settle.clone() {
        let target = site.or_else(|| match &env.nav.read(cx).screen {
            Screen::Sites(s) => s.clone(),
            _ => None,
        });
        env.nav.update(cx, |n, _| {
            n.settle = None;
            if let Some(t) = target {
                n.sheet = Some(crate::env::Sheet::Settle(t));
            }
        });
    }
    let open = env.nav.read(cx).sheet.clone()?;
    crate::sites::forms::sheet(env, &open, window, cx)
        .or_else(|| crate::services::forms::sheet(env, &open, window, cx))
}

fn badge(env: &AppEnv, screen: &Screen, cx: &App) -> Option<usize> {
    let s = env.state.read(cx);
    let n = match screen {
        Screen::Services(_) => s
            .services
            .value
            .iter()
            .flatten()
            .filter(|x| matches!(x.state, ServiceState::Failed { .. }))
            .count(),
        Screen::Ports => s.ports.value.iter().flatten().filter(|p| p.status == PortStatus::InUse).count(),
        Screen::Activity => s.jobs.running().count(),
        _ => 0,
    };
    (n > 0).then_some(n)
}

fn sidebar(env: &AppEnv, current: &Screen, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let items = PAGES.iter().map(|(label, page)| {
        let selected = same_page(current, page);
        let env = env.clone();
        let page = page.clone();
        let count = badge(&env, &page, cx);
        div()
            .id(ElementId::Name(format!("nav:{label}").into()))
            .role(Role::Tab)
            .aria_label(SharedString::from(*label))
            .aria_selected(selected)
            .flex()
            .justify_between()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |d| d.bg(Theme::wash(t.accent)))
            .on_click(move |_, _, cx| open(&env, page.clone(), cx))
            .child(*label)
            .when_some(count, |d, n| {
                d.child(
                    div()
                        .px(px(6.))
                        .rounded(px(8.))
                        .bg(t.danger)
                        .text_color(t.on_accent)
                        .text_xs()
                        .child(n.to_string()),
                )
            })
    });
    div()
        .id("sidebar")
        .role(Role::TabList)
        .aria_label("Screens")
        .w(px(170.))
        .p(px(8.))
        .gap(px(2.))
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.border)
        .text_sm()
        .children(items)
        .into_any_element()
}

fn banner(env: &AppEnv, cx: &App) -> Option<AnyElement> {
    let s = env.state.read(cx);
    if let Link::Offline(why) = link(&s.connection) {
        let env2 = env.clone();
        let start = Button::new("banner:start", "Start yardd")
            .style(ButtonStyle::Primary)
            .on_click(move |_, _, cx| env2.start_daemon(cx));
        return Some(Banner::new("banner:offline", BannerKind::Error, why).action(start).into_any_element());
    }
    if let Some(e) = s.status.value.as_ref().and_then(|st| st.config_error.clone()) {
        return Some(Banner::new("banner:config", BannerKind::Error, e.message).into_any_element());
    }
    if matches!(link(&s.connection), Link::Connected { stale: true }) {
        let env2 = env.clone();
        let restart =
            Button::new("banner:restart", "Restart yardd").on_click(move |_, _, cx| env2.restart_daemon(cx));
        return Some(
            Banner::new(
                "banner:stale",
                BannerKind::Warning,
                "yardd is older than this app; restart it to use everything",
            )
            .action(restart)
            .into_any_element(),
        );
    }
    None
}

fn content(env: &AppEnv, screen: &Screen, window: &mut Window, cx: &mut App) -> AnyElement {
    match screen {
        Screen::Overview => crate::overview::overview(env, window, cx),
        Screen::Activity => crate::activity::activity(env, window, cx),
        Screen::Sites(_) => crate::sites::sites(env, window, cx),
        Screen::Services(_) => crate::services::services(env, window, cx),
        Screen::Php => crate::screens::php::php(env, window, cx),
        Screen::Node => crate::screens::node::node(env, window, cx),
        Screen::Tunnels => crate::screens::tunnels::tunnels(env, window, cx),
        Screen::Ports => crate::screens::ports::ports(env, window, cx),
        Screen::Logs(_) => crate::screens::logs::logs(env, window, cx),
        Screen::Doctor => crate::screens::doctor::doctor(env, window, cx),
        Screen::Sources => crate::screens::sources::sources(env, window, cx),
    }
}
