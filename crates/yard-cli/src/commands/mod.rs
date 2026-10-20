//! One module per command group. Each command prints its own output and returns the
//! exit code; `run` turns errors into exit codes 1 and 3.

mod doctor;
mod ext;
mod helper;
mod lifecycle;
mod php;
mod ports;
mod shims;
mod sites;
mod units;
mod version;

use crate::cli::{Cli, Command, ExtAction, HelperAction, OnOff, PhpAction, PortsAction};
use crate::client::{CliError, Client};
use crate::output::{print_error, print_json, print_json_line};
use serde::Serialize;
use yard_core::Paths;
use yard_proto::{ErrorCode, Event, JobStarted, Topic, YardError};

pub type Outcome = Result<i32, CliError>;

pub struct Ctx {
    pub paths: Paths,
    pub json: bool,
    /// Set by `--root` or `YARD_ROOT`; launchd and the helper must then be told the root.
    pub root_overridden: bool,
}

impl Ctx {
    pub async fn client(&self) -> Result<Client, CliError> {
        Client::connect(&self.paths).await
    }

    /// `value` as JSON with `--json`, else the human text.
    pub fn emit<T: Serialize>(&self, value: &T, human: impl FnOnce() -> String) {
        if self.json {
            print_json(value);
        } else {
            print!("{}", human());
        }
    }
}

pub async fn run(cli: Cli) -> i32 {
    let json = cli.json;
    let root_overridden = cli.root.is_some() || std::env::var_os("YARD_ROOT").is_some();
    let paths = match cli.root {
        Some(root) => Ok(Paths::new(root)),
        None => Paths::from_env(),
    };
    let paths = match paths {
        Ok(p) => p,
        Err(e) => {
            print_error(json, &e);
            return 1;
        }
    };
    let ctx = Ctx { paths, json, root_overridden };
    let command = match (cli.version, cli.command) {
        (true, _) | (false, Some(Command::Version)) => Command::Version,
        (false, Some(c)) => c,
        (false, None) => {
            eprintln!("yard: a command is required; yard --help lists them");
            return 2;
        }
    };
    let outcome = match command {
        Command::Start => lifecycle::start(&ctx).await,
        Command::Stop => lifecycle::stop(&ctx).await,
        Command::Status => units::status(&ctx).await,
        Command::Version => version::version(&ctx).await,
        Command::Doctor => doctor::doctor(&ctx).await,
        Command::Sites => sites::list(&ctx).await,
        Command::Link { name } => sites::link(&ctx, name).await,
        Command::Unlink { name } => sites::unlink(&ctx, name).await,
        Command::Proxy { name, url } => sites::proxy(&ctx, name, url).await,
        Command::Unproxy { name } => sites::unproxy(&ctx, name).await,
        Command::Docroot { path, site } => sites::docroot(&ctx, path, site).await,
        Command::Isolate { version, site } => sites::isolate(&ctx, Some(version), site).await,
        Command::Unisolate { site } => sites::isolate(&ctx, None, site).await,
        Command::Ports { action: None } => ports::list(&ctx).await,
        Command::Ports { action: Some(PortsAction::Check) } => ports::check(&ctx).await,
        Command::Ports { action: Some(action) } => ports::change(&ctx, action).await,
        Command::Php { action: None | Some(PhpAction::List) } => php::list(&ctx).await,
        Command::Php { action: Some(PhpAction::Install { version }) } => php::install(&ctx, version).await,
        Command::Php { action: Some(PhpAction::Uninstall { version }) } => {
            php::uninstall(&ctx, version).await
        }
        Command::Php { action: Some(PhpAction::Default { version }) } => {
            php::set_default(&ctx, version).await
        }
        Command::Php { action: Some(PhpAction::Config { action }) } => php::config(&ctx, action).await,
        Command::Ext { action: ExtAction::List { php } } => ext::list(&ctx, php).await,
        Command::Ext { action: ExtAction::Add { name, php } } => ext::add(&ctx, name, php).await,
        Command::Ext { action: ExtAction::Enable { name, php } } => ext::toggle(&ctx, name, php, true).await,
        Command::Ext { action: ExtAction::Disable { name, php } } => {
            ext::toggle(&ctx, name, php, false).await
        }
        Command::Ext { action: ExtAction::Remove { name, php } } => ext::remove(&ctx, name, php).await,
        Command::Xdebug { state, php } => ext::xdebug(&ctx, matches!(state, OnOff::On), php).await,
        Command::InstallShims => shims::install(&ctx),
        Command::Restart { unit } => units::restart(&ctx, unit).await,
        Command::Helper { action: HelperAction::Install } => helper::install(&ctx).await,
        Command::Helper { action: HelperAction::Uninstall } => helper::uninstall(&ctx).await,
    };
    match outcome {
        Ok(code) => code,
        Err(CliError::Yard(e)) => {
            print_error(json, &e);
            1
        }
        Err(CliError::DaemonNotRunning) => {
            if json {
                let e = YardError::new(ErrorCode::DaemonNotRunning, "yardd is not running")
                    .with_hint("yard start");
                print_error(true, &e);
            } else {
                eprintln!("yardd is not running → yard start");
            }
            3
        }
    }
}

/// Runs a job method: subscribes to `jobs` first so no line is missed, prints each line
/// of the job's output (each event as NDJSON with `--json`), then `done` once it succeeds.
/// A failed job is the command's error.
pub async fn run_job<P: Serialize>(
    ctx: &Ctx,
    method: &str,
    params: P,
    done: impl FnOnce() -> String,
) -> Outcome {
    let mut client = ctx.client().await?;
    client.subscribe(&[Topic::Jobs]).await?;
    let started: JobStarted = client.call(method, params).await?;
    loop {
        let event = client.next_event().await?;
        match &event {
            Event::JobProgress { job_id, line } if *job_id == started.job_id => {
                if ctx.json {
                    print_json_line(&event);
                } else {
                    println!("{line}");
                }
            }
            Event::JobFinished { job_id, error } if *job_id == started.job_id => {
                if ctx.json {
                    print_json_line(&event);
                }
                return match error {
                    Some(e) => Err(e.clone().into()),
                    None => {
                        if !ctx.json {
                            print!("{}", done());
                        }
                        Ok(0)
                    }
                };
            }
            _ => {}
        }
    }
}

/// The current directory, as a Yard error when it is gone.
pub fn cwd() -> Result<std::path::PathBuf, YardError> {
    std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })
}
