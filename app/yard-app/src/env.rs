//! `AppEnv`: what every screen and the menu share, and the one place a `HintAction` becomes
//! something done (M6 design §10.2).

use crate::bridge::Bridge;
use crate::daemon::{Daemon, call};
use crate::platform::cli::CliRunner;
use crate::platform::open::Opener;
use crate::platform::settings::Settings;
use crate::platform::terminal::{TerminalLauncher, helper_install_args};
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
    /// `(owner, is_service)`: a port field to open.
    pub edit_port: Option<(String, bool)>,
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
            edit_port: None,
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
        let terminal =
            Rc::new(TerminalLauncher { temp: std::env::temp_dir(), terminal_app: settings.terminal_app() });
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

    pub fn start_daemon(&self, cx: &mut App) {
        self.run_cli("yardd:start", vec!["start".into()], true, cx);
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
                    let started = cli.run::<Value>(&["start"]).await.map(|_| ());
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
                self.nav.update(cx, |n, _| n.edit_port = Some((owner, is_service)));
                crate::window::main::open(self, screen, cx)
            }
            HintAction::OpenFile(file) => {
                // A hint's file opens only as text in an editor, never with its default handler.
                if let Err(e) = Opener::open_text(&file, self.settings.editor_app().as_deref()) {
                    self.runner.update(cx, |r, _| r.errors.record(ActionId("open-file".into()), e));
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
