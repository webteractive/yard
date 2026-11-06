//! The main window (M6 design §8.2): a sidebar of screens with badges, a banner when yardd is
//! offline, stale or its config does not parse, and the chosen screen.

use crate::env::AppEnv;
use crate::menubar::link;
use crate::platform::activation;
use crate::runner::ActionId;
use crate::widgets::Pressable;
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
        Screen::Overview => vec![ResourceKey::Status],
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
    let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
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

/// A question from the runner needs a window to be asked in: the main window asks it, so when an
/// install started from the menu bar needs brew's trust and no window is open, this opens one on
/// Activity, where the job shows.
pub fn ask_without_a_window(env: &AppEnv, cx: &mut App) {
    cx.observe(&env.runner, |runner, cx| {
        if runner.read(cx).prompt.is_some() && !is_open(cx) {
            open(&AppEnv::global(cx), Screen::Activity, cx);
        }
    })
    .detach();
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

impl MainWindow {
    /// Shows the runner's question (brew's trust) as a native alert and passes the answer back.
    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.env.runner.read(cx).prompt.clone().filter(|_| !self.asking) else { return };
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
            runner.update(cx, |r, cx| r.answer(prompt, choice, cx));
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
            .child(sidebar(&env, &screen, window, cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .children(banner(&env, cx))
                    .children(open_errors(&env, cx))
                    .child(
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
            .on_press(move |_, _, cx| open(&env, page.clone(), cx))
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

/// The action ids of opening a file or folder from any screen or the menu.
pub const OPEN_ERRORS: &str = "open-";

/// Why a file or folder was not opened, above every screen: the open can start anywhere.
pub fn open_errors(env: &AppEnv, cx: &App) -> Vec<AnyElement> {
    crate::screens::errors(env, OPEN_ERRORS, cx)
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

    /// An install started from the menu with no window open still gets brew's trust question
    /// (found in the M6 final review: it showed only while the main window was open).
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
        let shown = cx.update(|cx| super::open_errors(&env, cx).len());
        assert_eq!(shown, 1);
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
        assert!(cx.read_entity(&env.runner, |r, _| r.prompt.is_none()));
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
}
