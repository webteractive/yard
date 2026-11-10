//! The main window (M6 design §8.2): a sidebar of screens with badges, a banner when yardd is
//! offline, stale or its config does not parse, and the chosen screen.

use crate::env::AppEnv;
use crate::menubar::link;
use crate::platform::activation;
use crate::runner::ActionId;
use crate::widgets::Pressable;
use crate::widgets::banner::{Banner, BannerKind};
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::error_view::ErrorView;
use crate::widgets::theme::{Theme, Typography, size};
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, ElementId, Global, IntoElement, ParentElement, Pixels,
    Render, Role, SharedString, Styled, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
    div, prelude::*, px, size,
};
use yard_app_model::health::Link;
use yard_app_model::hint::HintAction;
use yard_app_model::hint::Screen;
use yard_app_model::refresh::ResourceKey;
use yard_client::api;
use yard_proto::methods::{ServiceParams, XdebugParams};
use yard_proto::{PortStatus, ServiceState};

struct MainHandle(WindowHandle<MainWindow>);

impl Global for MainHandle {}

pub struct MainWindow {
    env: AppEnv,
    /// A question from the runner is on screen.
    asking: bool,
}

impl MainWindow {
    fn new(env: AppEnv, cx: &mut Context<Self>) -> MainWindow {
        cx.observe(&env.state, |_, _, cx| cx.notify()).detach();
        cx.observe(&env.runner, |_, _, cx| cx.notify()).detach();
        cx.observe(&env.nav, |_, _, cx| cx.notify()).detach();
        MainWindow { env, asking: false }
    }
}

/// The lists a screen shows: fetched when it opens and when the window becomes active.
pub fn visible_keys(screen: &Screen) -> Vec<ResourceKey> {
    match screen {
        Screen::Overview => vec![ResourceKey::Status, ResourceKey::Ca],
        Screen::Sites(Some(site)) => {
            vec![ResourceKey::Sites, ResourceKey::SiteDetail(site.clone()), ResourceKey::Php]
        }
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
    if let Some(handle) = open_handle(cx) {
        // Called from inside the window (a site row, a hint), it is raised once the click is done.
        super::raise(handle, cx);
        activation::window_opened(cx);
        return;
    }
    let bounds = Bounds::centered(None, size(px(980.), px(660.)), cx);
    let options = window_options(bounds);
    let env = env.clone();
    if let Ok(handle) = cx.open_window(options, |_, cx| cx.new(|cx| MainWindow::new(env, cx))) {
        cx.set_global(MainHandle(handle));
        activation::window_opened(cx);
    }
}

/// The main window, while it is open.
fn open_handle(cx: &App) -> Option<WindowHandle<MainWindow>> {
    let handle = cx.try_global::<MainHandle>()?.0;
    super::is_open(handle, cx).then_some(handle)
}

/// Whether the main window is open.
pub fn is_open(cx: &mut App) -> bool {
    open_handle(cx).is_some()
}

/// A window closed: with the main window gone, its log tails close too.
pub fn closed(env: &AppEnv, cx: &mut App) {
    if is_open(cx) {
        return;
    }
    env.nav.update(cx, |n, _| {
        n.logs = None;
        n.service_log = None;
    });
}

/// A question from the runner needs a window to be asked in: the main window asks it, so when an
/// install started from the menu bar needs brew's trust and no window is open, this opens one on
/// Activity, where the job shows.
pub fn ask_without_a_window(env: &AppEnv, cx: &mut App) {
    cx.observe(&env.runner, |runner, cx| {
        if runner.read(cx).prompt().is_some() && !is_open(cx) {
            open(&AppEnv::global(cx), Screen::Activity, cx);
        }
    })
    .detach();
}

type Page = (&'static str, Screen);

/// The sidebar's groups and their pages, top to bottom (spec §3.1).
const SIDEBAR: [(&str, &[Page]); 4] = [
    ("Yard", &[("Overview", Screen::Overview), ("Activity", Screen::Activity)]),
    (
        "Serve",
        &[("Sites", Screen::Sites(None)), ("Services", Screen::Services(None)), ("Tunnels", Screen::Tunnels)],
    ),
    ("Runtimes", &[("PHP", Screen::Php), ("Node", Screen::Node)]),
    (
        "System",
        &[
            ("Ports", Screen::Ports),
            ("Logs", Screen::Logs(None)),
            ("Doctor", Screen::Doctor),
            ("Sources", Screen::Sources),
        ],
    ),
];

/// The page's name, as the sidebar and the toolbar show it.
pub(crate) fn page_title(screen: &Screen) -> &'static str {
    SIDEBAR
        .iter()
        .flat_map(|(_, pages)| pages.iter())
        .find(|(_, p)| same_page(p, screen))
        .map(|(l, _)| *l)
        .unwrap_or("Yard")
}

/// The toolbar strip at the top of the content: the page's title, read as its heading, and its
/// primary actions. Its empty space moves the window, as a title bar does (spec §3.1).
fn toolbar(env: &AppEnv, screen: &Screen, cx: &mut App) -> AnyElement {
    let actions: Vec<AnyElement> = match screen {
        Screen::Overview => crate::overview::actions(env, cx),
        Screen::Sites(_) => crate::sites::actions(env),
        Screen::Services(_) => crate::services::actions(env),
        Screen::Tunnels => crate::screens::tunnels::actions(env),
        Screen::Node => crate::screens::node::actions(env),
        Screen::Doctor => crate::screens::doctor::actions(env, cx),
        _ => vec![],
    };
    let drag =
        |d: gpui::Div| d.on_mouse_down(gpui::MouseButton::Left, |_, window, _| window.start_window_move());
    div()
        .debug_selector(|| "toolbar".into())
        .h(size::TOOLBAR)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(20.))
        .child(
            drag(div().debug_selector(|| "toolbar:title".into()).flex_none().type_title())
                .child(crate::widgets::text("heading", page_title(screen))),
        )
        .child(drag(div().flex_1().h_full()))
        .child(crate::widgets::bar().flex_none().children(actions))
        .into_any_element()
}

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

impl MainWindow {
    /// Shows the runner's question (brew's trust) as a native alert and passes the answer back.
    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.env.runner.read(cx).prompt().filter(|_| !self.asking) else { return };
        let (title, detail, buttons) = match &prompt {
            crate::runner::Prompt::Trust { formula, tap } => (
                format!("Trust {formula}?"),
                format!(
                    "Homebrew installs it only once you trust it or the {tap} tap. Yard never trusts anything on its own."
                ),
                vec!["Trust".to_string(), "Don't trust".to_string()],
            ),
            crate::runner::Prompt::Confirm { title, message, buttons, .. } => {
                (title.clone(), message.clone(), buttons.clone())
            }
        };
        self.asking = true;
        let labels: Vec<&str> = buttons.iter().map(String::as_str).collect();
        let answer = window.prompt(gpui::PromptLevel::Info, &title, Some(&detail), &labels, cx);
        let runner = self.env.runner.clone();
        cx.spawn(async move |this, cx| {
            let choice = answer.await.unwrap_or(1);
            runner.update(cx, |r, cx| r.answer(choice, cx));
            let _ = this.update(cx, |w, _| w.asking = false);
        })
        .detach();
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(window);
        let env = self.env.clone();
        self.ask(window, cx);
        let screen = env.nav.read(cx).screen.clone();
        let sheet = sheet(&env, window, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .bg(t.background)
            .text_color(t.text)
            .type_body()
            .child(sidebar(&env, &screen, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(toolbar(&env, &screen, cx))
                    .children(banner(&env, cx))
                    .child(
                        div()
                            .id("content")
                            .debug_selector(|| "content".into())
                            .min_w(px(0.))
                            .flex_1()
                            .overflow_y_scroll()
                            .p(px(16.))
                            .children(issues(&env, &screen, window, cx))
                            .child(content(&env, &screen, window, cx)),
                    ),
            )
            .children(sheet)
    }
}

/// The open sheet, drawn over the window. A settle request from a hint opens its sheet.
fn sheet(env: &AppEnv, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
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
        .or_else(|| crate::screens::forms::sheet(env, &open, window, cx))
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

/// A badge's count in words, read with its tab.
fn badge_text(screen: &Screen, n: usize) -> String {
    match screen {
        Screen::Services(_) => format!("{n} failed"),
        Screen::Ports => format!("{n} held by another app"),
        _ => format!("{n} running"),
    }
}

/// The sidebar's width (spec §3.3).
pub(crate) const SIDEBAR_WIDTH: gpui::Pixels = px(200.);

fn sidebar(env: &AppEnv, current: &Screen, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let item = |label: &'static str, page: &Screen, cx: &App| {
        let selected = same_page(current, page);
        let env = env.clone();
        let page = page.clone();
        let count = badge(&env, &page, cx);
        let quiet = matches!(page, Screen::Activity);
        div()
            .id(ElementId::Name(format!("nav:{label}").into()))
            .role(Role::Tab)
            .aria_label(SharedString::from(label))
            .when_some(count, |d, n| d.aria_description(badge_text(&page, n)))
            .aria_selected(selected)
            .debug_selector({
                let id = format!("nav:{label}");
                move || id
            })
            .flex()
            .items_center()
            .justify_between()
            .h(size::SIDEBAR_ITEM)
            .flex_none()
            .px(px(10.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |d| d.type_body_strong())
            .when(selected, |d| d.bg(Theme::wash(t.accent, 0.14)))
            .on_press(move |_, _, cx| open(&env, page.clone(), cx))
            .child(label)
            .when_some(count, |d, n| {
                // A failure is a red pill with white text; running jobs a quiet one (spec §3.5).
                let (bg, fg) = if quiet {
                    (Theme::wash(t.c.neutral, 0.15), t.c.neutral_text)
                } else {
                    (t.c.danger_fill, t.c.on_accent)
                };
                d.child(
                    div()
                        .debug_selector({
                            let id = format!("nav:{label}:badge");
                            move || id
                        })
                        .flex()
                        .items_center()
                        .h(px(16.))
                        .px(px(6.))
                        .rounded_full()
                        .bg(bg)
                        .text_color(fg)
                        .type_caption()
                        .child(n.to_string()),
                )
            })
    };
    let mut items: Vec<AnyElement> = vec![];
    for (i, (group, pages)) in SIDEBAR.iter().enumerate() {
        // A group's label is plain text, not a tab (spec §3.1).
        items.push(
            div()
                .px(px(10.))
                .pt(if i == 0 { px(4.) } else { px(14.) })
                .pb(px(4.))
                .type_caption()
                .text_color(t.c.text_secondary)
                .child(crate::widgets::text(format!("nav-group:{group}"), *group))
                .into_any_element(),
        );
        for (label, page) in pages.iter() {
            items.push(item(label, page, cx).into_any_element());
        }
    }
    div()
        .id("sidebar")
        .debug_selector(|| "sidebar".into())
        .role(Role::TabList)
        .aria_label("Screens")
        .w(SIDEBAR_WIDTH)
        .flex_shrink_0()
        .pt(size::TOOLBAR)
        .on_mouse_down(gpui::MouseButton::Left, |e, window, _| {
            // Its top strip, beside the traffic lights, moves the window as a title bar does.
            if e.position.y < size::TOOLBAR {
                window.start_window_move();
            }
        })
        .p(px(8.))
        .gap(px(2.))
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.border)
        .type_body()
        .children(items)
        .into_any_element()
}

/// The action ids of opening a file or folder from any screen or the menu; no screen owns them.
pub const OPEN_ERRORS: &str = "open-";

/// Whether `screen` shows the error of action `id` beside its own controls.
fn shown_on(screen: &Screen, id: &str, services: &[String]) -> bool {
    let prefixed = |prefixes: &[&str]| prefixes.iter().any(|p| id.starts_with(p));
    match screen {
        Screen::Overview => crate::overview::ERRORS.contains(&id),
        Screen::Sites(selected) => {
            id == "site:link"
                || id == "site:proxy"
                || selected
                    .as_ref()
                    .is_some_and(|n| crate::sites::DETAIL_VERBS.iter().any(|v| id == format!("site:{n}:{v}")))
        }
        Screen::Services(_) => {
            id == "service:add"
                || services
                    .iter()
                    .any(|n| crate::services::VERBS.iter().any(|v| id == format!("service:{n}:{v}")))
        }
        Screen::Php => prefixed(&["php:"]),
        Screen::Node => prefixed(&["node:"]),
        Screen::Tunnels => prefixed(&["tunnel:", "domain:"]),
        Screen::Ports => prefixed(&["port:"]),
        Screen::Sources => prefixed(&["source:"]),
        Screen::Logs(_) | Screen::Doctor | Screen::Activity => false,
    }
}

/// The window's options: its title (for Mission Control and the Window menu) and its size.
pub(crate) fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // The sidebar runs to the top edge under the traffic lights (spec §3.1).
        titlebar: Some(TitlebarOptions {
            title: Some("Yard".into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(16.), px(18.))),
        }),
        window_min_size: Some(size(px(760.), px(480.))),
        ..Default::default()
    }
}

/// The action errors `screen` does not show itself, by id: an action can start from the menu, a
/// hint on another screen or a file open, and its failure must reach whatever screen is open.
#[cfg(test)]
pub fn elsewhere_error_ids(env: &AppEnv, screen: &Screen, cx: &App) -> Vec<String> {
    elsewhere(env, screen, cx).into_iter().map(|(id, _)| id.0).collect()
}

fn elsewhere(env: &AppEnv, screen: &Screen, cx: &App) -> Vec<(ActionId, yard_proto::YardError)> {
    let services: Vec<String> =
        env.state.read(cx).services.value.iter().flatten().map(|s| s.name.clone()).collect();
    let mut all = env.runner.read(cx).errors.matching("");
    all.retain(|(id, _)| !shown_on(screen, &id.0, &services));
    all
}

/// Those errors, above the screen's content.
/// The errors of actions the open screen does not show, each one line with Dismiss.
pub fn elsewhere_errors(env: &AppEnv, screen: &Screen, cx: &App) -> Vec<AnyElement> {
    elsewhere(env, screen, cx)
        .into_iter()
        .map(|(id, e)| {
            let (act, dismiss) = (env.clone(), env.clone());
            ErrorView::new(format!("{}:error", id.0), &e, HintAction::parse(e.hint.as_deref().unwrap_or("")))
                .on_action(move |a, _, cx| act.perform(a, cx))
                .compact()
                .dismissible(id.0.clone(), move |_, cx| dismiss.runner.update(cx, |r, cx| r.dismiss(&id, cx)))
                .into_any_element()
        })
        .collect()
}

/// The issues strip at the top of the content (G6): with three or more errors it shows two and
/// a button for the rest. An error stays until it is dismissed or its action succeeds.
fn issues(env: &AppEnv, screen: &Screen, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let mut views = elsewhere_errors(env, screen, cx);
    if views.is_empty() {
        return None;
    }
    let expanded = window.use_keyed_state("issues:expanded", cx, |_, _| false);
    let hidden = if views.len() >= 3 && !*expanded.read(cx) { views.len() - 2 } else { 0 };
    views.truncate(views.len() - hidden);
    Some(
        div()
            .debug_selector(|| "issues".into())
            .flex()
            .flex_col()
            .gap(px(4.))
            .mb(px(16.))
            .children(views)
            .when(hidden > 0, |d| {
                d.child(crate::widgets::bar().child(
                    Button::new("issues:more", format!("{hidden} more…")).small().on_click(
                        move |_, _, cx| {
                            expanded.update(cx, |e, cx| {
                                *e = true;
                                cx.notify();
                            })
                        },
                    ),
                ))
            })
            .into_any_element(),
    )
}

/// The file a configuration error is in (`data.file`), which the banner opens.
fn config_file(e: &yard_proto::YardError) -> Option<std::path::PathBuf> {
    e.data.as_ref()?.get("file")?.as_str().map(std::path::PathBuf::from)
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
        let mut banner = Banner::new("banner:config", BannerKind::Error, e.message.clone());
        if let Some(file) = config_file(&e) {
            let env2 = env.clone();
            banner = banner.action(
                Button::new("banner:config:open", "Open file")
                    .on_click(move |_, _, cx| env2.perform(HintAction::OpenFile(file.clone()), cx)),
            );
        }
        return Some(banner.into_any_element());
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

#[cfg(test)]
mod tests {
    use crate::runner::ActionId;
    use crate::testing::env;
    use gpui::{AppContext, TestAppContext};
    use serde_json::json;
    use yard_app_model::hint::Screen;
    use yard_client::api;
    use yard_proto::methods::PhpParams;
    use yard_proto::{ErrorCode, YardError};

    /// A hinted file Yard refuses to open (a symlink, an executable) says why in the main window,
    /// opened for it if need be: the refusal was recorded where no screen showed it.
    #[gpui::test]
    async fn a_refused_file_says_why_in_the_main_window(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.conf");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("link.conf");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        cx.update(|cx| env.perform(yard_app_model::hint::HintAction::OpenFile(link.clone()), cx));
        cx.run_until_parked();
        assert!(cx.update(super::is_open), "the main window opens to say it");
        let shown = cx.update(|cx| super::elsewhere_errors(&env, &Screen::Overview, cx).len());
        assert_eq!(shown, 1);
    }

    /// Every action error reaches the screen the person is on: the screen that owns it shows it
    /// beside its controls, and any other screen above its content (M6 final review: Restart on
    /// the Overview's units, the menu's Xdebug switch and a Doctor hint's yard trust failed with
    /// no word anywhere).
    /// A port hint opens the sheet that sets it: a service's port or, for any other unit, its
    /// port in ports.toml (M6 final review: a unit's hint went to Ports and opened nothing).
    #[gpui::test]
    async fn a_port_hint_opens_its_sheet(cx: &mut TestAppContext) {
        use crate::env::Sheet;
        use yard_app_model::hint::HintAction;
        let (_fake, env) = env(cx);
        let open = |owner: &str, is_service: bool, cx: &mut TestAppContext| {
            cx.update(|cx| {
                env.nav.update(cx, |n, _| n.sheet = None);
                env.perform(HintAction::EditPort { owner: owner.into(), is_service }, cx)
            });
            cx.run_until_parked();
            cx.update(|cx| {
                let n = env.nav.read(cx);
                match &n.sheet {
                    Some(Sheet::Port { name, .. }) => format!("service {name}"),
                    Some(Sheet::UnitPort { unit, .. }) => format!("unit {unit}"),
                    _ => "none".into(),
                }
            })
        };
        assert_eq!(open("mysql-8.4", true, cx), "service mysql-8.4");
        assert_eq!(open("nginx-http", false, cx), "unit nginx-http");
    }

    /// Closing the main window closes its log tails, and with them their connections (M6 final
    /// review: the Logs screen's and a service's tail stayed subscribed with no window to show them).
    #[gpui::test]
    async fn closing_the_window_drops_its_log_tails(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let all = cx.new(|cx| crate::services::logs::LogTail::new(&env, None, None, cx));
        let one = cx.new(|cx| crate::services::logs::LogTail::new(&env, None, Some("redis".into()), cx));
        env.nav.update(cx, |n, _| {
            n.logs = Some(all);
            n.service_log = Some(("redis".into(), one));
        });
        cx.update(|cx| super::closed(&env, cx));
        cx.update(|cx| {
            let n = env.nav.read(cx);
            assert!(n.logs.is_none() && n.service_log.is_none());
        });
    }

    /// The port sheet a menu problem opens is drawn in the window, opened for it or already open.
    #[gpui::test]
    async fn a_port_hint_draws_its_sheet(cx: &mut TestAppContext) {
        use yard_app_model::hint::HintAction;
        let (_fake, env) = env(cx);
        cx.update(|cx| cx.set_global(env.clone()));
        for _ in 0..2 {
            cx.update(|cx| {
                env.nav.update(cx, |n, _| n.sheet = None);
                env.perform(HintAction::EditPort { owner: "redis".into(), is_service: true }, cx)
            });
            cx.run_until_parked();
            let handle = cx.update(|cx| cx.try_global::<super::MainHandle>().map(|h| h.0)).expect("a window");
            let vcx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("sheet:port").is_some(), "the sheet is drawn");
        }
    }

    /// A sidebar tab's badge is read with the tab, in words.
    #[test]
    fn a_badge_says_what_it_counts() {
        assert_eq!(super::badge_text(&Screen::Services(None), 2), "2 failed");
        assert_eq!(super::badge_text(&Screen::Ports, 1), "1 held by another app");
        assert_eq!(super::badge_text(&Screen::Activity, 3), "3 running");
    }

    /// A configuration error names its file in `data.file`: the banner opens it.
    #[test]
    fn a_config_error_names_the_file_to_open() {
        let e = YardError::new(ErrorCode::ConfigInvalid, "sites.toml line 3: expected =")
            .with_data(json!({"file": "/r/sites.toml", "line": 3, "column": 7}));
        assert_eq!(super::config_file(&e), Some(std::path::PathBuf::from("/r/sites.toml")));
        assert_eq!(super::config_file(&YardError::internal("x")), None);
    }

    #[gpui::test]
    async fn an_error_shows_on_whatever_screen_is_open(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        env.runner.update(cx, |r, _| {
            for id in ["unit:nginx:restart", "xdebug:8.4", "cli:trust", "php:8.4:install", "yardd:start"] {
                r.errors.record(ActionId(id.into()), YardError::internal("it failed"));
            }
        });
        let ids = |screen: Screen, cx: &mut TestAppContext| -> Vec<String> {
            cx.update(|cx| super::elsewhere_error_ids(&env, &screen, cx))
        };
        assert_eq!(
            ids(Screen::Overview, cx),
            ["cli:trust", "php:8.4:install", "unit:nginx:restart", "xdebug:8.4"]
        );
        assert_eq!(ids(Screen::Php, cx), ["cli:trust", "unit:nginx:restart", "xdebug:8.4", "yardd:start"]);
        assert_eq!(ids(Screen::Doctor, cx).len(), 5);
    }

    /// A site's detail shows a PHP picker of the installed versions, so it loads the PHP list
    /// with the site (found by the checklist script: opened before the PHP screen, the picker of a
    /// site on 7.1 read "—").
    #[test]
    fn a_sites_detail_loads_the_php_list_for_its_picker() {
        use yard_app_model::refresh::ResourceKey;
        let keys = super::visible_keys(&Screen::Sites(Some("blog".into())));
        assert!(keys.contains(&ResourceKey::Php), "{keys:?}");
        assert!(keys.contains(&ResourceKey::SiteDetail("blog".into())));
    }

    /// An install started from the menu with no window open still gets brew's trust question
    /// (found in the M6 final review: it showed only while the main window was open).
    #[gpui::test]
    async fn a_question_with_no_window_open_opens_one_to_ask_it(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        cx.update(|cx| {
            cx.set_global(env.clone());
            super::ask_without_a_window(&env, cx);
        });
        let untrusted = YardError::new(ErrorCode::FormulaUntrusted, "not trusted")
            .with_data(json!({"formula": "shivammathur/php/php@8.3", "tap": "shivammathur/php"}));
        fake.fail("php.install", untrusted);
        assert!(cx.windows().is_empty());
        env.runner
            .update(cx, |r, cx| {
                r.run_job(
                    ActionId("php:8.3:install".into()),
                    api::PHP_INSTALL,
                    PhpParams { php: "8.3".into() },
                    "Install PHP 8.3".into(),
                    cx,
                )
            })
            .detach();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1, "the main window opened");
        assert_eq!(cx.read_entity(&env.nav, |n, _| n.screen.clone()), Screen::Activity);
        assert!(cx.has_pending_prompt(), "and asks the question");
        cx.simulate_prompt_answer("Don't trust");
        cx.run_until_parked();
        assert!(cx.read_entity(&env.runner, |r, _| r.prompt().is_none()));
        assert_eq!(fake.count("sources.trust"), 0);
    }

    /// Opening a screen from inside the main window (a site row, a Doctor hint) shows it there
    /// (found by the checklist script: each click on a site row opened another main window).
    #[gpui::test]
    async fn a_screen_opened_from_inside_the_main_window_reuses_it(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        cx.update(|cx| {
            cx.set_global(env.clone());
            super::open(&env, Screen::Sites(None), cx);
        });
        cx.run_until_parked();
        let handle = cx.update(|cx| cx.global::<super::MainHandle>().0);
        let inner = env.clone();
        handle.update(cx, |_, _, cx| super::open(&inner, Screen::Sites(Some("chk1".into())), cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1, "no second main window");
        assert_eq!(cx.read_entity(&env.nav, |n, _| n.screen.clone()), Screen::Sites(Some("chk1".into())));
        assert!(cx.update(super::is_open), "and it is still the one open");
    }

    /// Opens the main window on Doctor with one check whose message is 300 characters long.
    fn doctor_with_a_long_message(cx: &mut TestAppContext) -> gpui::VisualTestContext {
        use yard_proto::{Check, CheckStatus, DoctorReport};
        let (_fake, env) = env(cx);
        cx.update(|cx| cx.set_global(env.clone()));
        let check =
            Check { id: "long".into(), status: CheckStatus::Fail, message: "x ".repeat(150), hint: None };
        env.state.update(cx, |s, _| s.doctor.value = Some(DoctorReport { checks: vec![check] }));
        cx.update(|cx| super::open(&env, Screen::Doctor, cx));
        cx.run_until_parked();
        let handle = cx.update(|cx| cx.try_global::<super::MainHandle>().map(|h| h.0)).expect("a window");
        let vcx = gpui::VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        vcx
    }

    /// The sidebar keeps its width and the content stays inside the window, however wide the
    /// content's text is (G3, G4: a long Doctor message squeezed the sidebar and pushed the page
    /// past the window's right edge).
    #[gpui::test]
    async fn sidebar_keeps_its_width_beside_wide_content(cx: &mut TestAppContext) {
        let mut vcx = doctor_with_a_long_message(cx);
        let sidebar = vcx.debug_bounds("sidebar").expect("the sidebar is drawn");
        let content = vcx.debug_bounds("content").expect("the content is drawn");
        let window = vcx.update(|w, _| w.viewport_size());
        assert_eq!(sidebar.size.width, super::SIDEBAR_WIDTH);
        assert!(
            content.right() <= window.width,
            "content ends at {:?}, window at {:?}",
            content.right(),
            window.width
        );
    }

    /// A button on its own sizes to its label rather than stretching across the pane (G7).
    #[gpui::test]
    async fn a_lone_button_sizes_to_its_label(cx: &mut TestAppContext) {
        let mut vcx = doctor_with_a_long_message(cx);
        let run = vcx.debug_bounds("doctor:run").expect("Run doctor is drawn");
        let content = vcx.debug_bounds("content").expect("the content is drawn");
        assert!(run.size.width < content.size.width / 2., "Run doctor is {:?} wide", run.size.width);
    }

    /// The sidebar's badge is centred on its item's label (G16: it sat high).
    #[gpui::test]
    async fn the_badge_is_centred_on_its_label(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        cx.update(|cx| cx.set_global(env.clone()));
        let services = serde_json::from_value(crate::testing::services()).unwrap();
        env.state.update(cx, |s, _| s.services.value = Some(services));
        cx.update(|cx| super::open(&env, Screen::Overview, cx));
        cx.run_until_parked();
        let handle = cx.update(|cx| cx.try_global::<super::MainHandle>().map(|h| h.0)).expect("a window");
        let mut vcx = gpui::VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        let item = vcx.debug_bounds("nav:Services").expect("the Services item");
        let badge = vcx.debug_bounds("nav:Services:badge").expect("its badge");
        let (a, b) = (item.center().y, badge.center().y);
        assert!((a - b).abs() <= gpui::px(1.), "item centre {a:?}, badge centre {b:?}");
        assert_eq!(badge.size.height, gpui::px(16.));
    }

    /// A sidebar item is 28 pt high (G13: φ line heights made it taller and uneven).
    #[gpui::test]
    async fn sidebar_items_are_28_high(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        cx.update(|cx| cx.set_global(env.clone()));
        cx.update(|cx| super::open(&env, Screen::Overview, cx));
        cx.run_until_parked();
        let handle = cx.update(|cx| cx.try_global::<super::MainHandle>().map(|h| h.0)).expect("a window");
        let mut vcx = gpui::VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        assert_eq!(vcx.debug_bounds("nav:Overview").unwrap().size.height, gpui::px(28.));
    }

    fn record_elsewhere(env: &crate::env::AppEnv, ids: &[&str], cx: &mut TestAppContext) {
        env.runner.update(cx, |r, _| {
            for id in ids {
                r.errors.record(ActionId((*id).into()), YardError::internal(format!("{id} failed")));
            }
        });
    }

    /// Dismiss takes one error off the strip and leaves the others (G6: old errors never left).
    #[gpui::test]
    async fn a_dismissed_error_leaves_the_strip(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        record_elsewhere(&env, &["php:8.4:install", "unit:nginx:restart"], cx);
        cx.update(|cx| env.runner.update(cx, |r, cx| r.dismiss(&ActionId("php:8.4:install".into()), cx)));
        let ids = cx.update(|cx| super::elsewhere_error_ids(&env, &Screen::Doctor, cx));
        assert_eq!(ids, ["unit:nginx:restart"]);
    }

    fn main_window_on(
        env: &crate::env::AppEnv,
        screen: Screen,
        cx: &mut TestAppContext,
    ) -> gpui::VisualTestContext {
        cx.update(|cx| cx.set_global(env.clone()));
        cx.update(|cx| super::open(env, screen, cx));
        cx.run_until_parked();
        let handle = cx.update(|cx| cx.try_global::<super::MainHandle>().map(|h| h.0)).expect("a window");
        let vcx = gpui::VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        vcx
    }

    /// Three or more errors show two and a button for the rest, so old errors do not take the screen.
    #[gpui::test]
    async fn three_errors_show_two_and_more(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        record_elsewhere(&env, &["php:8.4:install", "unit:nginx:restart", "xdebug:8.4"], cx);
        let mut vcx = main_window_on(&env, Screen::Doctor, cx);
        assert!(vcx.debug_bounds("dismiss:php:8.4:install").is_some());
        assert!(vcx.debug_bounds("dismiss:unit:nginx:restart").is_some());
        assert!(vcx.debug_bounds("dismiss:xdebug:8.4").is_none(), "the third waits behind More");
        assert!(vcx.debug_bounds("issues:more").is_some());
    }

    /// The strip is part of the scrolling content, not a band above it.
    #[gpui::test]
    async fn the_strip_scrolls_with_the_content(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        record_elsewhere(&env, &["php:8.4:install"], cx);
        let mut vcx = main_window_on(&env, Screen::Doctor, cx);
        let content = vcx.debug_bounds("content").expect("the content");
        let strip = vcx.debug_bounds("issues").expect("the strip");
        assert!(content.contains(&strip.origin), "the strip {strip:?} is inside the content {content:?}");
    }

    /// Each screen's inline form opens as a sheet that holds the fields it had, by id (spec §5
    /// step 15: the fields collapsed inline under the tables).
    #[gpui::test]
    async fn inline_forms_open_as_sheets_with_their_fields(cx: &mut TestAppContext) {
        let cases = [
            (
                Screen::Tunnels,
                "tunnels:share",
                "sheet:share",
                vec!["tunnel:share:site:box", "tunnel:share:host:box", "tunnel:share:named"],
            ),
            (Screen::Tunnels, "tunnels:add-domain", "sheet:add-domain", vec!["tunnel:domain:add:box"]),
            (Screen::Ports, "ports:range", "sheet:range", vec!["port:range:box"]),
            (Screen::Node, "node:install:open", "sheet:node-install", vec!["node:install:spec:box"]),
        ];
        for (screen, opener, sheet, fields) in cases {
            let (_fake, env) = env(cx);
            let mut vcx = main_window_on(&env, screen, cx);
            assert!(vcx.debug_bounds(sheet).is_none(), "{sheet} is closed until asked for");
            let at = vcx.debug_bounds(opener).unwrap_or_else(|| panic!("{opener} is drawn")).center();
            vcx.simulate_click(at, gpui::Modifiers::none());
            vcx.run_until_parked();
            assert!(vcx.debug_bounds(sheet).is_some(), "{opener} opens {sheet}");
            for f in fields {
                assert!(vcx.debug_bounds(f).is_some(), "{sheet} holds {f}");
            }
            cx.update(|cx| {
                cx.windows().iter().for_each(|w| {
                    let _ = w.update(cx, |_, w, _| w.remove_window());
                })
            });
            cx.run_until_parked();
        }
    }

    /// The sidebar is four groups in the design's order; a group's label is text, not a tab.
    #[test]
    fn the_sidebar_is_grouped() {
        let order: Vec<String> = super::SIDEBAR
            .iter()
            .flat_map(|(group, pages)| {
                std::iter::once(format!("[{group}]")).chain(pages.iter().map(|(label, _)| label.to_string()))
            })
            .collect();
        assert_eq!(
            order,
            [
                "[Yard]",
                "Overview",
                "Activity",
                "[Serve]",
                "Sites",
                "Services",
                "Tunnels",
                "[Runtimes]",
                "PHP",
                "Node",
                "[System]",
                "Ports",
                "Logs",
                "Doctor",
                "Sources"
            ]
        );
    }

    /// Every page's title is in the toolbar strip, read as the heading (spec §3.1).
    #[gpui::test]
    async fn each_screen_has_its_title_in_the_toolbar(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let mut vcx = main_window_on(&env, Screen::Overview, cx);
        for (_, pages) in super::SIDEBAR {
            for (label, page) in pages {
                vcx.update(|_, cx| super::open(&env, page.clone(), cx));
                vcx.run_until_parked();
                let toolbar = vcx.debug_bounds("toolbar").expect("the toolbar");
                let title = vcx.debug_bounds("toolbar:title").expect("its title");
                assert!(toolbar.contains(&title.center()), "{label}'s title is in the toolbar");
                assert_eq!(vcx.update(|_, cx| super::page_title(&env.nav.read(cx).screen)), *label);
            }
        }
    }

    /// A page's primary actions sit in the toolbar, not in the scrolling content.
    #[gpui::test]
    async fn primary_actions_sit_in_the_toolbar(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let mut vcx = main_window_on(&env, Screen::Sites(None), cx);
        let toolbar = vcx.debug_bounds("toolbar").expect("the toolbar");
        let link = vcx.debug_bounds("site:link").expect("Link a folder…");
        assert!(toolbar.contains(&link.center()));
        assert_eq!(toolbar.size.height, gpui::px(52.));
    }
}
