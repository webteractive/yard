//! The status item and its menu (M6 design §8.1): a native `NSStatusItem` and `NSMenu` through
//! tray-icon and muda, with GPUI only for windows. `menu_model` decides what the menu says from
//! `AppState`; `rebuild` changes the shown items' text and check marks in place, and builds the
//! muda items again only when one is added, removed or replaced (`diff`).

use crate::env::AppEnv;
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::App;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::collections::HashMap;
use time::OffsetDateTime;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use yard_app_model::health::{Health, Link, evaluate};
use yard_app_model::hint::HintAction;
use yard_client::session::ConnectionState;
use yard_core::present::state_text;
use yard_proto::{ServiceManager, ServiceState};

/// One line of the menu, before it becomes a muda item.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    /// A disabled line of text.
    Line(String),
    Problem {
        id: String,
        title: String,
        action: Option<HintAction>,
    },
    Action {
        id: String,
        title: String,
    },
    Check {
        id: String,
        title: String,
        checked: bool,
    },
    Submenu {
        title: String,
        entries: Vec<MenuEntry>,
    },
    Separator,
}

impl MenuEntry {
    /// The entry as a test reads it: `[x]` for a checked item, `!` for a problem.
    pub fn describe(&self) -> String {
        match self {
            MenuEntry::Line(t) | MenuEntry::Action { title: t, .. } => t.clone(),
            MenuEntry::Problem { title, .. } => format!("! {title}"),
            MenuEntry::Check { title, checked, .. } => {
                format!("[{}] {title}", if *checked { "x" } else { " " })
            }
            MenuEntry::Submenu { title, entries } => {
                format!("{title}: {}", entries.iter().map(MenuEntry::describe).collect::<Vec<_>>().join(", "))
            }
            MenuEntry::Separator => "—".into(),
        }
    }
}

pub fn link(state: &ConnectionState) -> Link {
    match state {
        ConnectionState::Connected(info) => Link::Connected { stale: info.stale },
        ConnectionState::Connecting => Link::Offline("connecting to yardd…".into()),
        ConnectionState::Disconnected(e) | ConnectionState::Incompatible(e) => {
            Link::Offline(e.message.clone())
        }
    }
}

/// What the menu shows, top to bottom (design §8.1). `recent` is the sites opened last.
pub fn menu_model(env: &AppEnv, recent: &[String], cx: &App) -> Vec<MenuEntry> {
    let s = env.state.read(cx);
    let mut out = vec![];
    let health = evaluate(
        &link(&s.connection),
        s.status.value.as_ref(),
        s.ports.value.as_deref(),
        s.services.value.as_deref(),
        s.tunnels.value.as_deref(),
        s.jobs.running().count(),
    );
    match (&s.connection, s.status.value.as_ref()) {
        (ConnectionState::Connected(info), Some(st)) => {
            let version = st.version.daemon.clone().unwrap_or_else(|| info.hello.daemon_version.clone());
            out.push(MenuEntry::Line(format!("Yard {version} · {} · .{}", st.profile.name(), st.tld)));
            if info.stale {
                out.push(MenuEntry::Action { id: "restart-yardd".into(), title: "Restart yardd".into() });
            }
        }
        (ConnectionState::Connected(_), None) => out.push(MenuEntry::Line("Yard".into())),
        (other, _) => {
            if let Link::Offline(why) = link(other) {
                out.push(MenuEntry::Line(why));
            }
            let (id, title) = if env.location.agent_installed {
                ("start-yardd", "Start yardd")
            } else {
                ("setup", "Set up Yard…")
            };
            out.push(MenuEntry::Action { id: id.into(), title: title.into() });
        }
    }
    if let Health::Error(problems) | Health::Warning(problems) = &health {
        out.push(MenuEntry::Separator);
        for (i, p) in problems.iter().enumerate() {
            out.push(MenuEntry::Problem {
                id: format!("problem:{i}"),
                title: p.text.clone(),
                action: p.action.clone(),
            });
        }
    }
    let running: Vec<_> = s.jobs.running().collect();
    if !running.is_empty() {
        out.push(MenuEntry::Separator);
        for job in running {
            let last = job.lines.back().map(|l| format!(" — {}", truncate(l, 60))).unwrap_or_default();
            out.push(MenuEntry::Line(format!("{}{last}", job.title)));
            out.push(MenuEntry::Action {
                id: format!("job:{}:cancel", job.key.id),
                title: format!("Cancel {}", job.title),
            });
        }
    }
    if let Some(sites) = &s.sites.value
        && !sites.is_empty()
    {
        out.push(MenuEntry::Separator);
        let mut first: Vec<&str> =
            recent.iter().map(String::as_str).filter(|r| sites.iter().any(|x| x.name == *r)).collect();
        let mut by_name: Vec<&str> = sites.iter().map(|x| x.name.as_str()).collect();
        by_name.sort();
        for name in by_name.iter() {
            if first.len() >= 5 {
                break;
            }
            if !first.contains(name) {
                first.push(name);
            }
        }
        for name in first {
            out.push(MenuEntry::Action { id: format!("site:{name}:open"), title: name.to_string() });
        }
        out.push(MenuEntry::Submenu {
            title: "Sites".into(),
            entries: by_name
                .iter()
                .map(|n| MenuEntry::Action { id: format!("site:{n}:open"), title: n.to_string() })
                .collect(),
        });
    }
    if let Some(services) = &s.services.value
        && !services.is_empty()
    {
        out.push(MenuEntry::Separator);
        let now = OffsetDateTime::now_utc();
        for svc in services {
            let title = format!("{} — {}", svc.name, state_text(svc, now));
            match svc.managed_by {
                ServiceManager::Yard => out.push(MenuEntry::Check {
                    id: format!("service:{}:toggle", svc.name),
                    title,
                    checked: svc.state == ServiceState::Running,
                }),
                _ => out.push(MenuEntry::Action { id: format!("service:{}:console", svc.name), title }),
            }
        }
    }
    let mut xdebug: Vec<MenuEntry> = s
        .extensions
        .iter()
        .filter_map(|(v, r)| {
            r.value.as_ref()?.iter().find(|e| e.name == "xdebug").map(|e| (v.clone(), e.enabled))
        })
        .map(|(v, on)| MenuEntry::Check {
            id: format!("xdebug:{v}:{}", if on { "off" } else { "on" }),
            title: format!("PHP {v}"),
            checked: on,
        })
        .collect();
    if !xdebug.is_empty() {
        xdebug.sort_by_key(MenuEntry::describe);
        out.push(MenuEntry::Submenu { title: "Xdebug".into(), entries: xdebug });
    }
    if let Some(item) = s.update.as_ref().and_then(yard_app_model::update::item) {
        // An update can start from here or by itself with no window open, so its failure is
        // said here too.
        let failed =
            env.runner.read(cx).errors.get(&crate::runner::ActionId("update:install".into())).cloned();
        let title = match failed {
            Some(e) => format!("{} (failed: {})", item.title, e.message),
            None => item.title,
        };
        out.push(MenuEntry::Separator);
        out.push(MenuEntry::Action { id: "update".into(), title });
    }
    out.push(MenuEntry::Separator);
    out.push(MenuEntry::Action { id: "open".into(), title: "Open Yard".into() });
    out.push(MenuEntry::Action { id: "settings".into(), title: "Settings…".into() });
    out.push(MenuEntry::Action { id: "quit".into(), title: "Quit Yard.app".into() });
    out
}

/// How a new menu differs from the one shown.
#[derive(Debug, PartialEq)]
pub enum MenuChange {
    Same,
    /// Only these items' text or check mark changed, by their place in `flat` order.
    Patch(Vec<usize>),
    /// An item was added, removed or replaced: build the menu again.
    Rebuild,
}

/// Every entry, each submenu followed by its own entries: the order the native items are kept in.
fn flat(entries: &[MenuEntry]) -> Vec<&MenuEntry> {
    let mut out = vec![];
    for e in entries {
        out.push(e);
        if let MenuEntry::Submenu { entries, .. } = e {
            out.extend(flat(entries));
        }
    }
    out
}

/// What makes an item the same item: its kind, its id, and for a problem whether it is enabled.
fn identity(e: &MenuEntry) -> (u8, Option<&str>, bool) {
    match e {
        MenuEntry::Line(_) => (0, None, false),
        MenuEntry::Problem { id, action, .. } => (1, Some(id), action.is_some()),
        MenuEntry::Action { id, .. } => (2, Some(id), false),
        MenuEntry::Check { id, .. } => (3, Some(id), false),
        MenuEntry::Submenu { .. } => (4, None, false),
        MenuEntry::Separator => (5, None, false),
    }
}

/// The text the native item shows.
fn label(e: &MenuEntry) -> String {
    match e {
        MenuEntry::Line(t)
        | MenuEntry::Action { title: t, .. }
        | MenuEntry::Check { title: t, .. }
        | MenuEntry::Submenu { title: t, .. } => t.clone(),
        MenuEntry::Problem { title, .. } => format!("⚠︎ {title}"),
        MenuEntry::Separator => String::new(),
    }
}

fn checked(e: &MenuEntry) -> Option<bool> {
    match e {
        MenuEntry::Check { checked, .. } => Some(*checked),
        _ => None,
    }
}

/// Whether `new` can be shown by changing the items of `old` in place. A running job's output
/// changes one line many times a second; rebuilding the menu for each would close it under the
/// pointer, and an item whose id changed must be rebuilt so a click names the right thing.
pub fn diff(old: &[MenuEntry], new: &[MenuEntry]) -> MenuChange {
    let (old, new) = (flat(old), flat(new));
    if old.len() != new.len() || old.iter().zip(&new).any(|(a, b)| identity(a) != identity(b)) {
        return MenuChange::Rebuild;
    }
    let changed: Vec<usize> = old
        .iter()
        .zip(&new)
        .enumerate()
        .filter(|(_, (a, b))| label(a) != label(b) || checked(a) != checked(b))
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() { MenuChange::Same } else { MenuChange::Patch(changed) }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}

/// A native item, kept so its text or check mark can change in place.
enum Native {
    Text(MenuItem),
    Check(CheckMenuItem),
    Sub(Submenu),
    Separator,
}

/// Keeps the status item alive, what its menu shows (in `flat` order beside the native items), and
/// the actions of the problems it lists.
pub struct MenuBar {
    tray: TrayIcon,
    shown: Vec<MenuEntry>,
    items: Vec<Native>,
    health: Option<Health>,
    problems: HashMap<String, HintAction>,
}

impl MenuBar {
    /// Puts each check mark back to what the menu shows: muda flips one when it is clicked, and
    /// the state, not the click, decides whether it is on.
    fn reset_checks(&self) {
        for (entry, item) in flat(&self.shown).into_iter().zip(&self.items) {
            if let (Some(on), Native::Check(c)) = (checked(entry), item) {
                c.set_checked(on);
            }
        }
    }
}

impl gpui::Global for MenuBar {}

/// Puts the status item in the menu bar, rebuilds its menu when the state changes, and forwards
/// clicks into GPUI.
pub fn install(env: &AppEnv, cx: &mut App) -> Result<(), String> {
    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon(&Health::Ok))
        .with_tooltip("Yard")
        .build()
        .map_err(|e| e.to_string())?;
    cx.set_global(MenuBar { tray, shown: vec![], items: vec![], health: None, problems: HashMap::new() });
    rebuild(env, cx);
    let (tx, mut rx) = mpsc::unbounded::<String>();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = tx.unbounded_send(event.id.0);
    }));
    cx.spawn(async move |cx| {
        while let Some(id) = rx.next().await {
            cx.update(|cx| on_click(&id, cx));
        }
    })
    .detach();
    cx.observe(&env.state, |_, cx| rebuild(&AppEnv::global(cx), cx)).detach();
    // VoiceOver's press on the status item opens the menu as a click does.
    let (press_tx, mut press_rx) = mpsc::unbounded::<()>();
    if crate::platform::status_press::install(move || {
        let _ = press_tx.unbounded_send(());
    }) {
        cx.spawn(async move |cx| {
            while press_rx.next().await.is_some() {
                cx.update(|cx| {
                    let tray = &cx.global::<MenuBar>().tray;
                    crate::platform::status_press::opening(|| tray.show_menu());
                });
            }
        })
        .detach();
    }
    Ok(())
}

fn rebuild(env: &AppEnv, cx: &mut App) {
    let recent = env.settings.recent_sites();
    let model = menu_model(env, &recent, cx);
    let s = env.state.read(cx);
    let health = evaluate(
        &link(&s.connection),
        s.status.value.as_ref(),
        s.ports.value.as_deref(),
        s.services.value.as_deref(),
        s.tunnels.value.as_deref(),
        s.jobs.running().count(),
    );
    let problems: HashMap<String, HintAction> = flat(&model)
        .into_iter()
        .filter_map(|e| match e {
            MenuEntry::Problem { id, action: Some(a), .. } => Some((id.clone(), a.clone())),
            _ => None,
        })
        .collect();
    let bar = cx.global_mut::<MenuBar>();
    bar.problems = problems;
    match diff(&bar.shown, &model) {
        MenuChange::Same => {}
        MenuChange::Patch(changed) => {
            let entries = flat(&model);
            for i in changed {
                match &bar.items[i] {
                    Native::Text(item) => item.set_text(label(entries[i])),
                    Native::Check(item) => {
                        item.set_text(label(entries[i]));
                        item.set_checked(checked(entries[i]).unwrap_or(false));
                    }
                    Native::Sub(item) => item.set_text(label(entries[i])),
                    Native::Separator => {}
                }
            }
        }
        MenuChange::Rebuild => {
            let menu = Menu::new();
            let mut items = vec![];
            for entry in &model {
                append(&menu, entry, &mut items);
            }
            bar.items = items;
            bar.tray.set_menu(Some(Box::new(menu)));
        }
    }
    bar.shown = model;
    if bar.health.as_ref() != Some(&health) {
        let _ = bar.tray.set_icon_templated(Some(icon(&health)));
        bar.health = Some(health);
    }
}

/// Adds `entry` to `menu`, and its native item (then its submenu's) to `items`, in `flat` order.
fn append(menu: &impl MenuParent, entry: &MenuEntry, items: &mut Vec<Native>) {
    let text = label(entry);
    match entry {
        MenuEntry::Line(_) => {
            let item = MenuItem::new(text, false, None);
            menu.add(&item);
            items.push(Native::Text(item));
        }
        MenuEntry::Problem { id, action, .. } => {
            let item = MenuItem::with_id(id.as_str(), text, action.is_some(), None);
            menu.add(&item);
            items.push(Native::Text(item));
        }
        MenuEntry::Action { id, .. } => {
            let item = MenuItem::with_id(id.as_str(), text, true, None);
            menu.add(&item);
            items.push(Native::Text(item));
        }
        MenuEntry::Check { id, checked, .. } => {
            let item = CheckMenuItem::with_id(id.as_str(), text, true, *checked, None);
            menu.add(&item);
            items.push(Native::Check(item));
        }
        MenuEntry::Submenu { entries, .. } => {
            let sub = Submenu::new(text, true);
            menu.add(&sub);
            items.push(Native::Sub(sub.clone()));
            for e in entries {
                append(&sub, e, items);
            }
        }
        MenuEntry::Separator => {
            menu.add(&PredefinedMenuItem::separator());
            items.push(Native::Separator);
        }
    }
}

/// `Menu` and `Submenu` take items the same way.
trait MenuParent {
    fn add(&self, item: &dyn muda::IsMenuItem);
}

impl MenuParent for Menu {
    fn add(&self, item: &dyn muda::IsMenuItem) {
        let _ = self.append(item);
    }
}

impl MenuParent for Submenu {
    fn add(&self, item: &dyn muda::IsMenuItem) {
        let _ = self.append(item);
    }
}

fn on_click(id: &str, cx: &mut App) {
    cx.global::<MenuBar>().reset_checks();
    let env = AppEnv::global(cx);
    let parts: Vec<&str> = id.splitn(3, ':').collect();
    match parts.as_slice() {
        ["quit"] => cx.quit(),
        ["open"] => crate::window::main::open(&env, env.nav.read(cx).screen.clone(), cx),
        ["settings"] => crate::settings_window::open(&env, cx),
        ["setup"] => crate::onboarding::open(&env, cx),
        ["update"] => env.update_now(cx),
        ["start-yardd"] => env.start_daemon(cx),
        ["restart-yardd"] => env.restart_daemon(cx),
        ["problem", _] => {
            if let Some(action) = cx.global::<MenuBar>().problems.get(id).cloned() {
                env.perform(action, cx);
            }
        }
        ["job", job, "cancel"] => {
            let job = job.to_string();
            env.runner.update(cx, |r, cx| r.cancel_job(job, cx).detach());
        }
        ["site", name, "open"] => {
            let url = env
                .state
                .read(cx)
                .sites
                .value
                .iter()
                .flatten()
                .find(|s| s.name == *name)
                .map(|s| s.url.clone());
            if let Some(url) = url {
                env.settings.note_recent_site(name);
                cx.open_url(&url);
            }
        }
        ["service", name, verb] => crate::window::main::service_action(&env, name, verb, cx),
        ["xdebug", v, onoff] => crate::window::main::xdebug(&env, v, *onoff == "on", cx),
        _ => {}
    }
}

/// An 18×18 template image per health (black on clear; macOS tints it): a filled circle when all
/// is well, a ring while offline, a ring with a dot while busy, a notch for a warning or error.
fn icon(health: &Health) -> Icon {
    const N: u32 = 36;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 17.5, y as f32 - 17.5);
            let r = (dx * dx + dy * dy).sqrt();
            let on = match health {
                Health::Ok => r < 15.0,
                Health::Offline(_) => (11.5..15.0).contains(&r),
                Health::Busy => (11.5..15.0).contains(&r) || r < 5.0,
                Health::Warning(_) | Health::Error(_) => {
                    r < 15.0
                        && !(dx.abs() < 2.5 && (-9.0..4.0).contains(&dy)
                            || (dx.abs() < 2.5 && (7.0..10.0).contains(&dy)))
                }
            };
            rgba.extend_from_slice(if on { &[0, 0, 0, 255] } else { &[0, 0, 0, 0] });
        }
    }
    Icon::from_rgba(rgba, N, N).expect("a 36×36 RGBA icon")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{env, services, site};
    use gpui::TestAppContext;
    use serde_json::json;
    use yard_proto::Event;

    fn titles(entries: &[MenuEntry]) -> Vec<String> {
        entries.iter().map(MenuEntry::describe).collect()
    }

    fn line(t: &str) -> MenuEntry {
        MenuEntry::Line(t.into())
    }

    fn action(id: &str, t: &str) -> MenuEntry {
        MenuEntry::Action { id: id.into(), title: t.into() }
    }

    fn check(id: &str, checked: bool) -> MenuEntry {
        MenuEntry::Check { id: id.into(), title: id.into(), checked }
    }

    fn menu(job_line: &str, redis: bool) -> Vec<MenuEntry> {
        vec![
            line("Yard 0.1.0"),
            MenuEntry::Separator,
            line(job_line),
            action("job:1:cancel", "Cancel Install PHP"),
            MenuEntry::Submenu { title: "Sites".into(), entries: vec![action("site:blog:open", "blog")] },
            check("service:redis:toggle", redis),
            action("quit", "Quit Yard.app"),
        ]
    }

    #[test]
    fn an_unchanged_menu_is_left_alone() {
        assert_eq!(diff(&menu("Install PHP — a", true), &menu("Install PHP — a", true)), MenuChange::Same);
    }

    #[test]
    fn a_new_job_line_or_check_mark_changes_items_in_place() {
        assert_eq!(
            diff(&menu("Install PHP — a", true), &menu("Install PHP — b", true)),
            MenuChange::Patch(vec![2])
        );
        assert_eq!(
            diff(&menu("Install PHP — a", true), &menu("Install PHP — a", false)),
            MenuChange::Patch(vec![6])
        );
    }

    #[test]
    fn a_renamed_item_in_a_submenu_changes_only_that_item() {
        let mut new = menu("x", true);
        new[4] = MenuEntry::Submenu {
            title: "Sites".into(),
            entries: vec![action("site:blog:open", "blog (8.4)")],
        };
        assert_eq!(diff(&menu("x", true), &new), MenuChange::Patch(vec![5]));
    }

    #[test]
    fn an_item_added_removed_or_replaced_rebuilds_the_menu() {
        let old = menu("x", true);
        let mut added = old.clone();
        added.insert(5, action("site:shop:open", "shop"));
        assert_eq!(diff(&old, &added), MenuChange::Rebuild);
        assert_eq!(diff(&old, &old[..6]), MenuChange::Rebuild);
        let mut replaced = old.clone();
        replaced[3] = action("job:2:cancel", "Cancel Install PHP");
        assert_eq!(diff(&old, &replaced), MenuChange::Rebuild, "a new id is a new item, so a click names it");
        let fixable = |action| {
            vec![MenuEntry::Problem { id: "problem:0".into(), title: "nginx failed".into(), action }]
        };
        assert_eq!(
            diff(&fixable(None), &fixable(Some(HintAction::RestartDaemon))),
            MenuChange::Rebuild,
            "a problem with a fix is enabled; one without is not"
        );
    }

    /// With no yardd and no LaunchAgent for the root, the menu's Set up Yard… is the onboarding
    /// checklist, not the main window (M6 final review; design §8.1).
    #[gpui::test]
    async fn an_available_update_shows_in_the_menu(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let before = cx.update(|cx| menu_model(&env, &[], cx));
        assert!(!before.iter().any(|e| matches!(e, MenuEntry::Action { id, .. } if id == "update")));
        env.state.update(cx, |s, _| {
            s.update = Some(yard_app_model::update::Check {
                installed: "0.1.0".into(),
                latest: "0.2.0".into(),
                available: true,
                kind: "bundle".into(),
                notes_url: "x".into(),
            })
        });
        let entries = cx.update(|cx| menu_model(&env, &[], cx));
        assert!(entries.contains(&action("update", "Update to 0.2.0…")), "{entries:?}");
    }

    /// An update started from the menu or by itself, with no window open, says why it failed
    /// on the menu item (M7 final review).
    #[gpui::test]
    async fn a_failed_update_says_so_in_the_menu(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        env.state.update(cx, |s, _| {
            s.update = Some(yard_app_model::update::Check {
                installed: "0.1.0".into(),
                latest: "0.2.0".into(),
                available: true,
                kind: "bundle".into(),
                notes_url: "x".into(),
            })
        });
        env.runner.update(cx, |r, _| {
            r.errors.record(
                crate::runner::ActionId("update:install".into()),
                yard_proto::YardError::internal("the download does not match its checksum"),
            )
        });
        let entries = cx.update(|cx| menu_model(&env, &[], cx));
        assert!(
            entries.contains(&action(
                "update",
                "Update to 0.2.0… (failed: the download does not match its checksum)"
            )),
            "{entries:?}"
        );
    }

    #[gpui::test]
    async fn set_up_yard_opens_onboarding(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        env.state.update(cx, |s, _| {
            s.connection = ConnectionState::Disconnected(yard_proto::YardError::new(
                yard_proto::ErrorCode::DaemonNotRunning,
                "yardd is not running",
            ));
        });
        let entries = cx.update(|cx| menu_model(&env, &[], cx));
        assert!(entries.contains(&action("setup", "Set up Yard…")), "{entries:?}");
    }

    #[gpui::test]
    async fn menu_lists_problems_jobs_sites_services_and_xdebug(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        env.state.update(cx, |s, _| {
            let mut st: yard_proto::StatusInfo = serde_json::from_value(crate::testing::status()).unwrap();
            st.units[0].state = yard_proto::UnitState::Failed { last_lines: vec!["bind failed".into()] };
            s.status.value = Some(st);
            s.services.value = serde_json::from_value(services()).ok();
            s.sites.value =
                serde_json::from_value(json!([site("blog", json!([])), site("shop", json!([]))])).ok();
            s.extensions.entry("8.4".into()).or_default().value = serde_json::from_value(json!([
                {"name": "xdebug", "source": "pecl", "enabled": true}
            ]))
            .ok();
        });
        fake.event(Event::JobProgress { job_id: "php-install-1".into(), line: "==> Pouring php@8.3".into() });
        cx.run_until_parked();
        let model = cx.update(|cx| menu_model(&env, &[], cx));
        let t = titles(&model);
        assert_eq!(t[0], "Yard 0.1.0 · standard · .yard", "{t:#?}");
        assert!(t.iter().any(|x| x == "! nginx failed"), "{t:#?}");
        assert!(
            t.iter().any(|x| x == "! mysql-8.4 failed to start: port 3307 is held by mysqld (pid 9)"),
            "{t:#?}"
        );
        assert!(t.iter().any(|x| x == "Install PHP (started elsewhere) — ==> Pouring php@8.3"), "{t:#?}");
        assert!(t.iter().any(|x| x == "Cancel Install PHP (started elsewhere)"), "{t:#?}");
        assert!(t.iter().any(|x| x == "blog"), "{t:#?}");
        assert!(t.iter().any(|x| x == "[x] redis — running"), "{t:#?}");
        assert!(t.iter().any(|x| x == "[ ] mysql-8.4 — failed"), "{t:#?}");
        assert!(
            t.iter().any(|x| x == "emailulator — running"),
            "a LaunchAgent instance has no checkmark: {t:#?}"
        );
        assert!(t.iter().any(|x| x == "Xdebug: [x] PHP 8.4"), "{t:#?}");
        assert!(
            t.ends_with(&["Open Yard".to_string(), "Settings…".to_string(), "Quit Yard.app".to_string()]),
            "{t:#?}"
        );
    }
}
