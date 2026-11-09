//! `AppEnv`: what every screen and the menu share, and the one place a `HintAction` becomes
//! something done (M6 design §10.2).

use crate::bridge::Bridge;
use crate::daemon::{Daemon, call};
use crate::platform::cli::CliRunner;
use crate::platform::open::Opener;
use crate::platform::settings::Settings;
use crate::platform::terminal::{TerminalLauncher, helper_install_args, uninstall_args};
use crate::runner::{ActionId, ActionRunner};
use crate::state::AppState;
use gpui::{App, AppContext, Entity, Global};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use yard_app_model::hint::{HintAction, JobCall, RpcCall, Screen, TerminalCommand};
use yard_app_model::locate::Location;
use yard_app_model::refresh::ResourceKey;
use yard_client::api;
use yard_proto::YardError;

/// A quick call's work, boxed so the three kinds share a type.
type Work = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), YardError>>>>;

/// A sheet over the main window, with the fields it owns while it is open.
#[derive(Clone)]
pub enum Sheet {
    Link {
        folder: std::path::PathBuf,
        name: Entity<crate::widgets::text_field::TextField>,
    },
    Proxy {
        name: Entity<crate::widgets::text_field::TextField>,
        url: Entity<crate::widgets::text_field::TextField>,
    },
    Settle(String),
    AddService {
        engine: usize,
        line: Entity<crate::widgets::text_field::TextField>,
        port: Entity<crate::widgets::text_field::TextField>,
        /// The lines yardd offered after refusing one (`SERVICE_VERSION_UNSUPPORTED`).
        lines: Vec<String>,
    },
    Port {
        name: String,
        port: Entity<crate::widgets::text_field::TextField>,
    },
    /// A unit's port in ports.toml (`ports.set`), for a unit that is not a service instance.
    UnitPort {
        unit: String,
        port: Entity<crate::widgets::text_field::TextField>,
    },
    Sleep {
        name: String,
        after: Entity<crate::widgets::text_field::TextField>,
    },
}

/// Where the main window is and what it should open on.
pub struct Nav {
    pub screen: Screen,
    /// A site whose settle form to open; `Some(None)` for the selected site.
    pub settle: Option<Option<String>>,
    pub sheet: Option<Sheet>,
    /// The service whose log the Services screen shows.
    pub service_log: Option<(String, Entity<crate::services::logs::LogTail>)>,
    /// Text fields a screen keeps between frames, by id.
    pub fields: std::collections::HashMap<String, Entity<crate::widgets::text_field::TextField>>,
    /// The PHP version whose extensions the PHP screen shows.
    pub php_version: Option<String>,
    /// The Logs screen's tail, open while the screen is shown.
    pub logs: Option<Entity<crate::services::logs::LogTail>>,
    /// The Node releases nodejs.org lists, once asked for.
    pub node_available: Option<String>,
}

impl Nav {
    /// The text field `id`, made on first use.
    pub fn field(
        env: &AppEnv,
        id: &str,
        label: &str,
        placeholder: &str,
        cx: &mut App,
    ) -> Entity<crate::widgets::text_field::TextField> {
        if let Some(f) = env.nav.read(cx).fields.get(id) {
            return f.clone();
        }
        let f = cx.new(|cx| {
            crate::widgets::text_field::TextField::new(
                id.to_string(),
                label.to_string(),
                placeholder.to_string(),
                cx,
            )
        });
        env.nav.update(cx, |n, _| n.fields.insert(id.to_string(), f.clone()));
        f
    }
}

/// Finds the `yard` to run, given the path chosen in Settings (or none).
pub type LocateYard = Rc<dyn Fn(Option<PathBuf>) -> Option<PathBuf>>;

#[derive(Clone)]
pub struct AppEnv {
    pub daemon: Arc<dyn Daemon>,
    pub state: Entity<AppState>,
    pub runner: Entity<ActionRunner>,
    pub nav: Entity<Nav>,
    pub cli: Rc<CliRunner>,
    pub terminal: Rc<TerminalLauncher>,
    pub settings: Rc<Settings>,
    /// Where Yard was found at launch. The `yard` in use is `cli.yard()`: a path chosen later
    /// applies at once.
    pub location: Location,
    locate_yard: LocateYard,
}

impl Global for AppEnv {}

impl AppEnv {
    pub fn new(
        daemon: Arc<dyn Daemon>,
        location: Location,
        locate_yard: LocateYard,
        bridge: Arc<Bridge>,
        cx: &mut App,
    ) -> AppEnv {
        let state = AppState::new(daemon.clone(), Duration::from_millis(150), cx);
        let runner = ActionRunner::new(daemon.clone(), state.clone(), cx);
        let nav = cx.new(|_| Nav {
            screen: Screen::Overview,
            settle: None,
            sheet: None,
            service_log: None,
            fields: Default::default(),
            php_version: None,
            logs: None,
            node_available: None,
        });
        let settings = Rc::new(Settings::default());
        let cli = Rc::new(CliRunner::new(
            location.yard.clone(),
            location.root.clone(),
            location.root_is_default,
            bridge.clone(),
        ));
        let terminal = Rc::new(TerminalLauncher::new(std::env::temp_dir(), {
            let settings = settings.clone();
            move || settings.terminal_app()
        }));
        AppEnv { daemon, state, runner, nav, cli, terminal, settings, location, locate_yard }
    }

    /// Saves `path` as the `yard` to run and runs it from now on; with none, `yard` is found again
    /// beside the LaunchAgent's yardd or in the bundle.
    pub fn set_yard_path(&self, path: Option<&Path>, cx: &mut App) {
        self.settings.set_yard_path(path);
        self.use_yard_setting(path.map(Path::to_path_buf), cx);
    }

    /// Finds the `yard` to run with `setting` as the Settings choice, and runs it from now on.
    pub fn use_yard_setting(&self, setting: Option<PathBuf>, cx: &mut App) {
        self.cli.set_yard((self.locate_yard)(setting));
        cx.refresh_windows();
    }

    /// The files of the data root the app talks to, as yard-core names them.
    pub fn paths(&self) -> yard_core::Paths {
        yard_core::Paths::new(self.location.root.clone())
    }

    pub fn global(cx: &App) -> AppEnv {
        cx.global::<AppEnv>().clone()
    }

    /// Runs `yard <args> --json`; its error stays beside `id`.
    pub fn run_cli(&self, id: &str, args: Vec<String>, then_reconnect: bool, cx: &mut App) {
        let cli = self.cli.clone();
        let daemon = self.daemon.clone();
        self.runner.update(cx, |r, cx| {
            r.perform(
                ActionId(id.to_string()),
                async move {
                    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                    let result = cli.run::<Value>(&argv).await.map(|_| ());
                    if then_reconnect {
                        daemon.reconnect_now();
                    }
                    result
                },
                cx,
            )
            .detach()
        });
    }

    /// Runs `yard <args> --json` like `run_cli`, then fetches `keys` again whatever the result:
    /// for a CLI command that changes something yardd sends no event for (`trust`, `untrust`).
    pub fn run_cli_refreshing(&self, id: &str, args: Vec<String>, keys: Vec<ResourceKey>, cx: &mut App) {
        let cli = self.cli.clone();
        let state = self.state.clone();
        let task = self.runner.update(cx, |r, cx| {
            r.perform(
                ActionId(id.to_string()),
                async move {
                    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                    cli.run::<Value>(&argv).await.map(|_| ())
                },
                cx,
            )
        });
        cx.spawn(async move |cx| {
            task.await;
            state.update(cx, |s, cx| s.refresh(keys, cx));
        })
        .detach();
    }

    /// Asks `yard self-update --check` and keeps the answer; with Settings' switch on, an
    /// install.sh bundle then updates itself (M7 spec §6.3). A failed check shows nothing: the
    /// next one comes in a day.
    pub fn check_for_update(&self, cx: &mut App) {
        let (cli, env) = (self.cli.clone(), self.clone());
        cx.spawn(async move |cx| {
            let Ok(check) = cli.run::<yard_app_model::update::Check>(&["self-update", "--check"]).await
            else {
                return;
            };
            let auto = cx.update(|cx| {
                env.state.update(cx, |s, cx| {
                    s.update = Some(check.clone());
                    cx.notify();
                });
                env.settings.auto_update()
            });
            if yard_app_model::update::auto_install(&check, auto) {
                cx.update(|cx| env.install_update(cx));
            }
        })
        .detach();
    }

    /// Checks now and then every [`CHECK_EVERY`](yard_app_model::update::CHECK_EVERY).
    pub fn check_for_updates_daily(&self, cx: &mut App) {
        let env = self.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.update(|cx| env.check_for_update(cx));
                cx.background_executor().timer(yard_app_model::update::CHECK_EVERY).await;
            }
        })
        .detach();
    }

    /// What the menu item and the Overview button do.
    pub fn update_now(&self, cx: &mut App) {
        let Some(item) = self.state.read(cx).update.as_ref().and_then(yard_app_model::update::item) else {
            return;
        };
        match item.action {
            yard_app_model::update::UpdateAction::Install => self.install_update(cx),
            yard_app_model::update::UpdateAction::CopyBrewCommand => {
                Opener::copy(cx, yard_app_model::update::BREW_UPGRADE)
            }
        }
    }

    /// `yard self-update --yes` as an action (its error shows beside `update:install`), then the
    /// app relaunches from the same path, which now holds the new bundle.
    pub fn install_update(&self, cx: &mut App) {
        let cli = self.cli.clone();
        let task = self.runner.update(cx, |r, cx| {
            r.perform(
                ActionId("update:install".into()),
                async move { cli.run::<Value>(&["self-update", "--yes"]).await.map(|_| ()) },
                cx,
            )
        });
        cx.spawn(async move |cx| {
            if task.await.is_some() {
                cx.update(relaunch);
            }
        })
        .detach();
    }

    /// Hands `yard uninstall` to Terminal, which asks for the typed confirmation and sudo, then
    /// quits: the app is one of the things it removes.
    pub fn uninstall(&self, cx: &mut App) {
        let Some(yard) = self.cli.yard() else {
            return self.record(
                "settings:uninstall",
                YardError::new(yard_proto::ErrorCode::InvalidArgument, "yard was not found")
                    .with_hint("set its path in Settings"),
                cx,
            );
        };
        let args = uninstall_args(&yard, &self.location.root, self.location.root_is_default);
        match self.terminal.launch(&args) {
            Ok(()) => cx.quit(),
            Err(e) => self.record("settings:uninstall", e, cx),
        }
    }

    pub fn start_daemon(&self, cx: &mut App) {
        // A click on Start yardd is the go-ahead to install a missing nginx or dnsmasq, which
        // `yard start` would otherwise ask for on a terminal the app does not have (M7 spec §5.4).
        self.run_cli("yardd:start", vec!["start".into(), "--yes".into()], true, cx);
    }

    pub fn stop_daemon(&self, cx: &mut App) {
        self.run_cli("yardd:stop", vec!["stop".into()], true, cx);
    }

    pub fn restart_daemon(&self, cx: &mut App) {
        let cli = self.cli.clone();
        let daemon = self.daemon.clone();
        self.runner.update(cx, |r, cx| {
            r.perform(
                ActionId("yardd:restart".into()),
                async move {
                    cli.run::<Value>(&["stop"]).await?;
                    let started = cli.run::<Value>(&["start", "--yes"]).await.map(|_| ());
                    daemon.reconnect_now();
                    started
                },
                cx,
            )
            .detach()
        });
    }

    /// Does what a hint says, through the API, the CLI, a screen, Terminal or the clipboard.
    pub fn perform(&self, action: HintAction, cx: &mut App) {
        let daemon = self.daemon.clone();
        match action {
            HintAction::Rpc(rpc) => self.runner.update(cx, |r, cx| {
                let (id, work): (String, Work) = match rpc {
                    RpcCall::ServicesStart(p) => (
                        format!("service:{}:start", p.name),
                        Box::pin(async move { call(&*daemon, api::SERVICES_START, p).await.map(|_| ()) }),
                    ),
                    RpcCall::ExtEnable(p) => (
                        format!("ext:{}:enable", p.name),
                        Box::pin(async move { call(&*daemon, api::EXT_ENABLE, p).await.map(|_| ()) }),
                    ),
                    RpcCall::ExtDisable(p) => (
                        format!("ext:{}:disable", p.name),
                        Box::pin(async move { call(&*daemon, api::EXT_DISABLE, p).await.map(|_| ()) }),
                    ),
                };
                r.perform(ActionId(id), work, cx).detach()
            }),
            HintAction::Job(job) => self.runner.update(cx, |r, cx| {
                let task = match job {
                    JobCall::PhpInstall(p) => {
                        let title = format!("Install PHP {}", p.php);
                        r.run_job(ActionId(format!("php:{}:install", p.php)), api::PHP_INSTALL, p, title, cx)
                    }
                    JobCall::ExtAdd(p) => {
                        let title = format!("Add the {} extension", p.name);
                        r.run_job(ActionId(format!("ext:{}:add", p.name)), api::EXT_ADD, p, title, cx)
                    }
                    JobCall::ExtRemove(p) => {
                        let title = format!("Remove the {} extension", p.name);
                        r.run_job(ActionId(format!("ext:{}:remove", p.name)), api::EXT_REMOVE, p, title, cx)
                    }
                    JobCall::NodeInstall(p) => {
                        let title = format!("Install Node {}", p.spec.clone().unwrap_or_default());
                        r.run_job(ActionId("node:install".into()), api::NODE_INSTALL, p, title, cx)
                    }
                    JobCall::ServiceAdd(p) => {
                        let title = format!("Add {}", yard_proto::engine_display(&p.engine));
                        r.run_job(
                            ActionId(format!("service:{}:add", p.engine)),
                            api::SERVICES_ADD,
                            p,
                            title,
                            cx,
                        )
                    }
                    JobCall::SourceTrust(p) => {
                        let title = format!("Trust {}", p.target);
                        r.run_job(
                            ActionId(format!("source:{}:trust", p.target)),
                            api::SOURCES_TRUST,
                            p,
                            title,
                            cx,
                        )
                    }
                };
                task.detach()
            }),
            HintAction::Cli(args) => {
                let reconnect = args.first().is_some_and(|a| a == "start");
                self.run_cli(&format!("cli:{}", args.join(" ")), args, reconnect, cx)
            }
            HintAction::RestartDaemon => self.restart_daemon(cx),
            HintAction::GoTo(screen) => crate::window::main::open(self, screen, cx),
            HintAction::EditPort { owner, is_service } => {
                let screen = if is_service { Screen::Services(Some(owner.clone())) } else { Screen::Ports };
                let kind = if is_service { "port" } else { "unit-port" };
                let port = cx.new(|cx| {
                    crate::widgets::text_field::TextField::new(
                        format!("sheet:{kind}:{owner}"),
                        format!("Port for {owner}"),
                        "a free port",
                        cx,
                    )
                });
                let sheet = if is_service {
                    Sheet::Port { name: owner, port }
                } else {
                    Sheet::UnitPort { unit: owner, port }
                };
                crate::window::main::open(self, screen, cx);
                self.nav.update(cx, |n, cx| {
                    n.sheet = Some(sheet);
                    cx.notify();
                });
            }
            HintAction::OpenFile(file) => {
                // A hint's file opens only as text in an editor, never with its default handler.
                let id = ActionId(format!("{}file", crate::window::main::OPEN_ERRORS));
                match Opener::open_text(&file, self.settings.editor_app().as_deref()) {
                    Ok(()) => self.runner.update(cx, |r, cx| {
                        r.errors.clear(&id);
                        cx.notify();
                    }),
                    Err(e) => {
                        self.runner.update(cx, |r, cx| {
                            r.errors.record(id, e);
                            cx.notify();
                        });
                        // Asked from the menu or a closed window: open one to say why.
                        if !crate::window::main::is_open(cx) {
                            crate::window::main::open(self, Screen::Overview, cx);
                        }
                    }
                }
            }
            HintAction::Settle { site } => {
                self.nav.update(cx, |n, _| n.settle = Some(site.clone()));
                crate::window::main::open(self, Screen::Sites(site), cx)
            }
            HintAction::Terminal(command) => {
                let args = match command {
                    TerminalCommand::HelperInstall => match self.cli.yard() {
                        Some(yard) => {
                            helper_install_args(&yard, &self.location.root, self.location.root_is_default)
                        }
                        None => {
                            return self.record(
                                "terminal",
                                YardError::new(yard_proto::ErrorCode::InvalidArgument, "yard was not found")
                                    .with_hint("set its path in Settings"),
                                cx,
                            );
                        }
                    },
                    TerminalCommand::Brew { reinstall, formula } => {
                        vec![
                            "/opt/homebrew/bin/brew".into(),
                            if reinstall { "reinstall" } else { "install" }.into(),
                            formula,
                        ]
                    }
                };
                if let Err(e) = self.terminal.launch(&args) {
                    self.record("terminal", e, cx);
                }
            }
            HintAction::Copy(text) => Opener::copy(cx, &text),
        }
    }

    fn record(&self, id: &str, e: YardError, cx: &mut App) {
        self.runner.update(cx, |r, cx| {
            r.errors.record(ActionId(id.into()), e);
            cx.notify();
        });
    }
}

/// Run by `/bin/sh -c` with the app's path as `$0`: waits until this app (the shell's parent)
/// has quit, then opens the app again.
const RELAUNCH: &str = "while kill -0 $PPID 2>/dev/null; do sleep 0.2; done; open \"$0\"";

/// Opens this app again once it has quit, from its own path, which an update has just
/// replaced. A run outside a bundle (`cargo run`) only quits.
fn relaunch(cx: &mut App) {
    let app = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.parent()?.to_path_buf()))
        .filter(|app| app.extension().is_some_and(|e| e == "app"));
    if let Some(app) = app {
        let _ = std::process::Command::new("/bin/sh").args(["-c", RELAUNCH]).arg(&app).spawn();
    }
    cx.quit();
}

#[cfg(test)]
mod tests {
    /// The relaunch waits for this app to be gone, or `open` would only bring the quitting one
    /// forward (M7 final review).
    #[test]
    fn the_relaunch_waits_for_this_app_to_quit() {
        assert!(super::RELAUNCH.contains("while kill -0 $PPID"), "{}", super::RELAUNCH);
        assert!(super::RELAUNCH.ends_with("open \"$0\""), "{}", super::RELAUNCH);
    }
}
