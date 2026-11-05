//! The status item and its menu (M6 design §8.1): a native `NSStatusItem` and `NSMenu` through
//! tray-icon and muda, with GPUI only for windows. `menu_model` decides what the menu says from
//! `AppState`; `rebuild` turns that into muda items whenever what it shows changes.

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
                ("open", "Set up Yard…")
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
    out.push(MenuEntry::Separator);
    out.push(MenuEntry::Action { id: "open".into(), title: "Open Yard".into() });
    out.push(MenuEntry::Action { id: "settings".into(), title: "Settings…".into() });
    out.push(MenuEntry::Action { id: "quit".into(), title: "Quit Yard.app".into() });
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}

/// Keeps the status item alive, and the actions of the problems the menu shows.
pub struct MenuBar {
    tray: TrayIcon,
    problems: HashMap<String, HintAction>,
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
    cx.set_global(MenuBar { tray, problems: HashMap::new() });
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
    let menu = Menu::new();
    let mut problems = HashMap::new();
    for entry in &model {
        append(&menu, entry, &mut problems);
    }
    let bar = cx.global_mut::<MenuBar>();
    bar.problems = problems;
    bar.tray.set_menu(Some(Box::new(menu)));
    let _ = bar.tray.set_icon_templated(Some(icon(&health)));
}

fn append(menu: &impl MenuParent, entry: &MenuEntry, problems: &mut HashMap<String, HintAction>) {
    match entry {
        MenuEntry::Line(t) => menu.add(&MenuItem::new(t, false, None)),
        MenuEntry::Problem { id, title, action } => {
            if let Some(a) = action {
                problems.insert(id.clone(), a.clone());
            }
            menu.add(&MenuItem::with_id(id.as_str(), format!("⚠︎ {title}"), action.is_some(), None))
        }
        MenuEntry::Action { id, title } => menu.add(&MenuItem::with_id(id.as_str(), title, true, None)),
        MenuEntry::Check { id, title, checked } => {
            menu.add(&CheckMenuItem::with_id(id.as_str(), title, true, *checked, None))
        }
        MenuEntry::Submenu { title, entries } => {
            let sub = Submenu::new(title, true);
            for e in entries {
                append(&sub, e, problems);
            }
            menu.add(&sub)
        }
        MenuEntry::Separator => menu.add(&PredefinedMenuItem::separator()),
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
    let env = AppEnv::global(cx);
    let parts: Vec<&str> = id.splitn(3, ':').collect();
    match parts.as_slice() {
        ["quit"] => cx.quit(),
        ["open"] => crate::window::main::open(&env, env.nav.read(cx).screen.clone(), cx),
        ["settings"] => crate::settings_window::open(&env, cx),
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
        assert!(t.iter().any(|x| x == "! mysql-8.4 failed to start: port 3307 is held by mysqld (pid 9)"), "{t:#?}");
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
