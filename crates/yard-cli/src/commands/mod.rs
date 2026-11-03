//! One module per command group. Each command prints its own output and returns the
//! exit code; `run` turns errors into exit codes 1 and 3.

mod doctor;
mod env;
mod ext;
mod helper;
mod info;
mod init;
mod lifecycle;
mod node;
mod php;
mod ports;
mod services;
mod shims;
mod sites;
mod sources;
mod trust;
mod units;
mod version;

use crate::cli::{
    Cli, Command, ExtAction, HelperAction, NodeAction, OnOff, PhpAction, PortsAction, ServiceAction,
    SourceAction,
};
use crate::client::{CliError, Client};
use crate::output::{print_error, print_json, print_json_line};
use clap::CommandFactory;
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use yard_core::Paths;
use yard_proto::methods::{self, JobParams};
use yard_proto::{ErrorCode, Event, JobStarted, Topic, YardError};

pub type Outcome = Result<i32, CliError>;

pub struct Ctx {
    pub paths: Paths,
    pub json: bool,
    /// Set by `--root` or `YARD_ROOT`; launchd and the helper must then be told the root.
    pub root_overridden: bool,
    /// macOS's `security`, or `YARD_SECURITY` when set, which tests point at a fake.
    pub security: PathBuf,
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
    let security =
        std::env::var_os("YARD_SECURITY").map_or_else(|| PathBuf::from("/usr/bin/security"), PathBuf::from);
    let ctx = Ctx { paths, json, root_overridden, security };
    let command = match (cli.version, cli.command) {
        (true, _) | (false, Some(Command::Version)) => Command::Version,
        (false, Some(c)) => c,
        (false, None) => {
            // As `git` and `cargo` do: bare `yard` shows what it can do.
            eprintln!("{}", Cli::command().render_help());
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
        Command::Link { name, yes } => sites::link(&ctx, name, yes).await,
        Command::Init { name, force, replace, yes, local } => {
            init::init(&ctx, init::InitArgs { name, force, replace, yes, local }).await
        }
        Command::Unlink { name } => sites::unlink(&ctx, name).await,
        Command::Proxy { name, url } => sites::proxy(&ctx, name, url).await,
        Command::Unproxy { name } => sites::unproxy(&ctx, name).await,
        Command::Docroot { path, site } => sites::docroot(&ctx, path, site).await,
        Command::Isolate { version, site } => sites::isolate(&ctx, Some(version), site).await,
        Command::Unisolate { site } => sites::isolate(&ctx, None, site).await,
        Command::Secure { site } => sites::secure(&ctx, site, true).await,
        Command::Unsecure { site } => sites::secure(&ctx, site, false).await,
        Command::Trust => trust::trust(&ctx).await,
        Command::Untrust => trust::untrust(&ctx).await,
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
        Command::Node { action: None } => node::list(&ctx, None).await,
        Command::Node { action: Some(NodeAction::List { available }) } => node::list(&ctx, available).await,
        Command::Node { action: Some(NodeAction::Install { spec }) } => node::install(&ctx, spec).await,
        Command::Node { action: Some(NodeAction::Uninstall { version }) } => {
            node::uninstall(&ctx, version).await
        }
        Command::Node { action: Some(NodeAction::Default { spec }) } => node::set_default(&ctx, spec).await,
        Command::Node { action: Some(NodeAction::Use { spec, site }) } => {
            node::pin(&ctx, Some(spec), site).await
        }
        Command::Node { action: Some(NodeAction::Unuse { site }) } => node::pin(&ctx, None, site).await,
        Command::Ext { action: ExtAction::List { php } } => ext::list(&ctx, php).await,
        Command::Ext { action: ExtAction::Add { name, php } } => ext::add(&ctx, name, php).await,
        Command::Ext { action: ExtAction::Enable { name, php } } => ext::toggle(&ctx, name, php, true).await,
        Command::Ext { action: ExtAction::Disable { name, php } } => {
            ext::toggle(&ctx, name, php, false).await
        }
        Command::Ext { action: ExtAction::Remove { name, php } } => ext::remove(&ctx, name, php).await,
        Command::Xdebug { state, php } => ext::xdebug(&ctx, matches!(state, OnOff::On), php).await,
        Command::Services => services::list(&ctx).await,
        Command::Service { action } => match action {
            ServiceAction::Add { engine, version, port, yes: _ } => {
                services::add(&ctx, engine, version, port).await
            }
            ServiceAction::Remove { instance, keep_data, yes } => {
                services::remove(&ctx, instance, keep_data, yes).await
            }
            ServiceAction::Start { instance } => services::start(&ctx, instance).await,
            ServiceAction::Stop { instance } => services::stop(&ctx, instance).await,
            ServiceAction::Restart { instance } => services::restart(&ctx, instance).await,
            ServiceAction::Logs { instance, follow, lines } => {
                services::logs(&ctx, instance, follow, lines).await
            }
            ServiceAction::Port { instance, port } => services::port(&ctx, instance, port).await,
            ServiceAction::SleepAfter { instance, after } => {
                services::sleep_after(&ctx, instance, after).await
            }
        },
        Command::Env { site, service } => env::env(&ctx, site, service).await,
        Command::Info { site } => info::info(&ctx, site).await,
        Command::Sources => sources::list(&ctx).await,
        Command::Source { action: SourceAction::Trust { target, tap } } => {
            sources::change(&ctx, target, tap, true).await
        }
        Command::Source { action: SourceAction::Untrust { target, tap } } => {
            sources::change(&ctx, target, tap, false).await
        }
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
/// A failed job is the command's error. Ctrl-C cancels the job and waits for it to clean
/// up (`JOB_CANCELLED`); a second Ctrl-C stops waiting and exits 130.
///
/// The SIGINT stream is made once, before the first line is printed, and kept. A
/// `ctrl_c()` made anew on each turn of the loop registers nothing until `select!` polls it,
/// so a Ctrl-C right after a line could kill the command outright, or, once tokio's handler
/// was installed, reach no listener and leave the command waiting on a job nobody cancelled.
pub async fn run_job<P: Serialize>(
    ctx: &Ctx,
    method: &str,
    params: P,
    done: impl FnOnce() -> String,
) -> Outcome {
    match wait_job(ctx, method, params).await? {
        JobEnd::Finished(_) => {
            if !ctx.json {
                print!("{}", done());
            }
            Ok(0)
        }
        JobEnd::Abandoned(code) => Ok(code),
    }
}

/// How a job [`wait_job`] waited for ended: finished, with what it made, or no longer waited
/// for (Ctrl-C twice), with the exit code.
#[derive(Debug)]
pub enum JobEnd {
    Finished(#[expect(dead_code, reason = "yard tunnel reads it (M5 Task 14)")] Option<serde_json::Value>),
    Abandoned(i32),
}

/// [`run_job`]'s waiting: starts the job, prints its progress (each event under `--json`),
/// cancels it on Ctrl-C, and returns its result once it succeeds. A failed job is the error.
pub async fn wait_job<P: Serialize>(ctx: &Ctx, method: &str, params: P) -> Result<JobEnd, CliError> {
    let mut client = ctx.client().await?;
    client.subscribe(&[Topic::Jobs]).await?;
    let started: JobStarted = client.call(method, params).await?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|e| YardError::internal(format!("cannot listen for Ctrl-C: {e}")))?;
    let mut cancelling = false;
    loop {
        let event = tokio::select! {
            event = client.next_event() => event?,
            _ = interrupt.recv() => {
                if cancelling {
                    eprintln!("Stopped waiting; {} may still be finishing.", started.job_id);
                    return Ok(JobEnd::Abandoned(130));
                }
                cancelling = true;
                eprintln!("Cancelling {} (Ctrl-C again to stop waiting)…", started.job_id);
                let params = JobParams { job_id: started.job_id.clone() };
                match client.call::<_, ()>(methods::JOBS_CANCEL, params).await {
                    // It finished meanwhile; its job_finished is on the way.
                    Err(e) if e.code == ErrorCode::JobNotFound => {}
                    other => other?,
                }
                continue;
            }
        };
        match &event {
            Event::JobProgress { job_id, line } if *job_id == started.job_id => {
                if ctx.json {
                    print_json_line(&event);
                } else {
                    println!("{line}");
                }
            }
            Event::JobFinished { job_id, error, result } if *job_id == started.job_id => {
                if ctx.json {
                    print_json_line(&event);
                }
                return match error {
                    Some(e) => Err(e.clone().into()),
                    None => Ok(JobEnd::Finished(result.clone())),
                };
            }
            _ => {}
        }
    }
}

/// A job `run_job` stopped waiting for (Ctrl-C twice) ends the command with its code.
pub fn finished(outcome: Outcome) -> Result<(), CliError> {
    match outcome? {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

/// A person to ask: stdin and stdout are terminals, and the output is not `--json`.
pub fn interactive(ctx: &Ctx) -> bool {
    !ctx.json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Asks a y/N question on stdout and reads the answer from stdin; anything but y or yes is no.
pub fn confirm(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// The current directory, canonicalized, for a request yardd resolves against its own
/// working directory: the daemon runs elsewhere and compares real paths.
pub fn cwd_absolute() -> Result<String, YardError> {
    let dir = cwd()?;
    let real = std::fs::canonicalize(&dir).unwrap_or(dir);
    Ok(real.display().to_string())
}

/// The current directory, as a Yard error when it is gone.
pub fn cwd() -> Result<std::path::PathBuf, YardError> {
    std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })
}
