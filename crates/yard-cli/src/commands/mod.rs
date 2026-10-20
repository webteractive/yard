//! One module per command group. Each command prints its own output and returns the
//! exit code; `run` turns errors into exit codes 1 and 3.

mod doctor;
mod helper;
mod lifecycle;
mod ports;
mod sites;
mod units;
mod version;

use crate::cli::{Cli, Command, HelperAction, PortsAction};
use crate::client::{CliError, Client};
use crate::output::{print_error, print_json};
use serde::Serialize;
use yard_core::Paths;
use yard_proto::{ErrorCode, YardError};

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
        Command::Ports { action: None } => ports::list(&ctx).await,
        Command::Ports { action: Some(PortsAction::Check) } => ports::check(&ctx).await,
        Command::Ports { action: Some(action) } => ports::change(&ctx, action).await,
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

/// The current directory, as a Yard error when it is gone.
pub fn cwd() -> Result<std::path::PathBuf, YardError> {
    std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })
}
