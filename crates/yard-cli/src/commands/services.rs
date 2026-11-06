//! `yard services` and `yard service add|remove|start|stop|restart|logs|port|sleep-after`.

use super::sources::with_trust;
use super::{Ctx, Outcome, confirm, env, finished, interactive, run_job};
use crate::client::CliError;
use crate::output::{for_agents, print_json_line, table};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::signal::unix::{SignalKind, signal};
use yard_core::emailulator::NAME as EMAILULATOR;
use yard_core::present::{sleep_text, state_text};
use yard_proto::methods::{
    self, EnvParams, ServiceAddParams, ServiceLogsParams, ServiceParams, ServicePortParams,
    ServiceRemoveParams, ServiceSleepParams,
};
use yard_proto::{
    Account, EnvBlock, ErrorCode, Event, ServiceInfo, ServiceManager, ServiceState, SettingSource, SiteInfo,
    Topic, YardError,
};

/// How long `start`, `restart` and `port` wait for an instance to come up: the same wait as
/// the daemon's add job, longer than any instance's start timeout.
const UP_WAIT: Duration = yard_core::services::READY_WAIT;
const POLL: Duration = Duration::from_millis(200);

/// How many of a failed instance's last lines are printed under the error.
const SHOWN_LINES: usize = 10;

/// An account as `root (no password)`, `— (no auth)` or `minioadmin / minioadmin`, split into
/// the user and what follows it. Yard stores no other password, so these are the only
/// secrets that can appear, and each is a default it set up on a loopback-only server.
pub fn account_parts(account: &Account) -> (String, String) {
    match (&account.user, account.password.as_deref()) {
        (None, _) => ("—".into(), "(no auth)".into()),
        (Some(user), None | Some("")) => (user.clone(), "(no password)".into()),
        (Some(user), Some(password)) => (user.clone(), format!("/ {password}")),
    }
}

pub fn account_text(account: &Account) -> String {
    let (user, rest) = account_parts(account);
    format!("{user} {rest}")
}

/// `mysql-8.4 failed: port 3307 is held by mysqld (pid 77034, DBngin)  (yard service port
/// mysql-8.4 <port>)`: why a failed instance failed, when yardd says, with the command that
/// fixes it. `None` for any other state.
pub fn failure_line(info: &ServiceInfo) -> Option<String> {
    let ServiceState::Failed { reason: Some(reason), .. } = &info.state else { return None };
    Some(match &reason.hint {
        Some(hint) => format!("{} failed: {}  ({hint})", info.name, reason.message),
        None => format!("{} failed: {}", info.name, reason.message),
    })
}

/// `127.0.0.1:3307`: where the instance listens (its first port; MinIO's is the API's).
pub fn address(info: &ServiceInfo) -> String {
    info.ports.first().map_or_else(|| "-".to_string(), |p| format!("127.0.0.1:{}", p.port))
}

async fn instances(ctx: &Ctx) -> Result<Vec<ServiceInfo>, CliError> {
    Ok(ctx.client().await?.call(methods::SERVICES_LIST, Value::Null).await?)
}

pub async fn list(ctx: &Ctx) -> Outcome {
    let services = instances(ctx).await?;
    ctx.emit(&services, || {
        if services.is_empty() {
            return format!(
                "No services yet. Add one with yard service add <engine>: mysql, postgres, redis, minio or emailulator.\n\n{}",
                for_agents("yard services")
            );
        }
        let now = OffsetDateTime::now_utc();
        let rows: Vec<Vec<String>> = services
            .iter()
            .map(|s| {
                let port = s.ports.first().map_or_else(|| "-".to_string(), |p| p.port.to_string());
                let used_by = if s.used_by.is_empty() { "-".to_string() } else { s.used_by.join(", ") };
                vec![
                    s.name.clone(),
                    s.engine.clone(),
                    s.version.clone().unwrap_or_else(|| "-".into()),
                    port,
                    state_text(s, now),
                    sleep_text(s),
                    account_text(&s.account),
                    used_by,
                    managed_by(&s.managed_by),
                ]
            })
            .collect();
        let header = ["INSTANCE", "ENGINE", "VERSION", "PORT", "STATE", "SLEEP", "ACCOUNT", "USED BY", "MANAGED BY"];
        let mut out = table(&header, &rows);
        out.push('\n');
        let consoles: Vec<String> =
            services.iter().filter_map(|s| Some(format!("{} web UI: {}\n", s.name, s.console.as_ref()?))).collect();
        if !consoles.is_empty() {
            consoles.iter().for_each(|line| out.push_str(line));
            out.push('\n');
        }
        let failures: Vec<String> = services.iter().filter_map(failure_line).collect();
        if !failures.is_empty() {
            failures.iter().for_each(|f| out.push_str(&format!("{f}\n")));
            out.push('\n');
        }
        out.push_str(&for_agents("yard services"));
        out
    });
    Ok(0)
}

/// `yard` or `LaunchAgent <label>`: who starts and stops the instance.
pub fn managed_by(manager: &ServiceManager) -> String {
    match manager {
        ServiceManager::Yard => "yard".into(),
        ServiceManager::LaunchAgent { label, .. } => format!("LaunchAgent {label}"),
        ServiceManager::Other => "-".into(),
    }
}

/// The first executable regular file named `name` in a directory of `$PATH`, as a shell
/// would run it. Relative directories are skipped: yardd's working directory is not ours.
fn on_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).filter(|dir| dir.is_absolute()).map(|dir| dir.join(name)).find(|candidate| {
        candidate.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// `yard service add`: the job, then the new instance's `.env` lines.
pub async fn add(ctx: &Ctx, engine: String, version: Option<String>, port: Option<u16>) -> Outcome {
    add_instance(ctx, &engine, version.as_deref(), port).await?;
    Ok(0)
}

/// Adds an instance (asking about trust when a person can answer), then prints what to put
/// in `.env`. `yard init` and `yard link` add the services a `.yard` needs the same way.
pub async fn add_instance(
    ctx: &Ctx,
    engine: &str,
    version: Option<&str>,
    port: Option<u16>,
) -> Result<(), CliError> {
    // The line is the daemon's to default, so the new instance is found by what it added.
    let before: Vec<String> =
        if ctx.json { vec![] } else { instances(ctx).await?.into_iter().map(|s| s.name).collect() };
    // emailulator comes from its own installer, so yardd is told which one this shell runs.
    let path =
        (engine == EMAILULATOR).then(|| on_path(EMAILULATOR)).flatten().map(|p| p.display().to_string());
    let params = ServiceAddParams { engine: engine.into(), version: version.map(Into::into), port, path };
    finished(with_trust(ctx, || run_job(ctx, methods::SERVICES_ADD, params.clone(), String::new)).await)?;
    if ctx.json {
        return Ok(());
    }
    if engine == EMAILULATOR {
        return print_mail(ctx).await;
    }
    let created = instances(ctx).await?.into_iter().find(|s| s.engine == engine && !before.contains(&s.name));
    let Some(created) = created else {
        println!("Added {engine}.");
        return Ok(());
    };
    println!("Added {}. For your .env:", created.name);
    let params = EnvParams { service: Some(created.name), ..Default::default() };
    let blocks: Vec<EnvBlock> = ctx.client().await?.call(methods::ENV_PRINT, params).await?;
    print!("{}", yard_core::present::env_text(&env::warn_notes(blocks)));
    Ok(())
}

/// After `yard service add emailulator`: where its web UI is, and its `.env` lines.
async fn print_mail(ctx: &Ctx) -> Result<(), CliError> {
    let row = instances(ctx).await?.into_iter().find(|s| s.name == EMAILULATOR);
    match row.and_then(|r| r.console) {
        Some(url) => println!("emailulator catches mail; its web UI is {url}. For your .env:"),
        None => println!("emailulator catches mail. For your .env:"),
    }
    let params = EnvParams { service: Some(EMAILULATOR.into()), ..Default::default() };
    let blocks: Vec<EnvBlock> = ctx.client().await?.call(methods::ENV_PRINT, params).await?;
    print!("{}", yard_core::present::env_text(&env::warn_notes(blocks)));
    Ok(())
}

/// What the daemon's `services.remove` would refuse, found before anything is asked: an
/// instance that is not there, or one a linked site's `.yard` lists (`SERVICE_IN_USE`,
/// worded as the daemon words it, with the `.yard` files named in the hint). The daemon
/// still checks again when the delete arrives.
async fn check_removable(ctx: &Ctx, instance: &str) -> Result<(), CliError> {
    let found = instances(ctx).await?.into_iter().find(|s| s.name == instance);
    let Some(found) = found else {
        return Err(YardError::new(
            ErrorCode::ServiceNotFound,
            format!("no service instance named {instance:?}"),
        )
        .with_hint("yard services lists the instances")
        .into());
    };
    if found.used_by.is_empty() {
        return Ok(());
    }
    // The files are for the hint only: a failed lookup leaves the sites to name it.
    let sites: Vec<SiteInfo> = match ctx.client().await {
        Ok(mut client) => client.call(methods::SITES_LIST, Value::Null).await.unwrap_or_default(),
        Err(_) => vec![],
    };
    let files: Vec<String> = sites
        .into_iter()
        .filter(|site| found.used_by.contains(&site.name))
        .filter_map(|site| site.project_file)
        .collect();
    let message = format!("{instance} is used by {}, from its .yard", found.used_by.join(", "));
    let mut refused = YardError::new(ErrorCode::ServiceInUse, message)
        .with_data(serde_json::json!({ "sites": found.used_by, "files": files }));
    refused = match files.first() {
        Some(file) => refused.with_hint(format!("edit {file}")),
        None => refused.with_hint(format!("yard info {}", found.used_by[0])),
    };
    Err(refused.into())
}

/// Checks the instance can be removed, then asks, then deletes. Without a terminal and
/// without `--yes` it deletes nothing.
pub async fn remove(ctx: &Ctx, instance: String, keep_data: bool, yes: bool) -> Outcome {
    if instance == EMAILULATOR {
        // While its LaunchAgent runs it yardd refuses, naming emailulator's own command; without
        // one it only forgets the recorded path. Nothing is Yard's to delete, so nothing to ask.
        let params = ServiceRemoveParams { name: instance, keep_data };
        ctx.client().await?.call::<_, ()>(methods::SERVICES_REMOVE, params).await?;
        ctx.emit(&(), || "Yard no longer records emailulator; it was not installed.\n".to_string());
        return Ok(0);
    }
    check_removable(ctx, &instance).await?;
    let dir = ctx.paths.service_data(&instance).display().to_string();
    if !yes {
        let question = if keep_data {
            format!("Delete {instance}? Its data stays in {dir}. [y/N] ")
        } else {
            format!("Delete {instance} and its data? [y/N] ")
        };
        if !(interactive(ctx) && confirm(&question)) {
            let what = if keep_data { instance.clone() } else { format!("{instance} and its data") };
            let flags = if keep_data { " --keep-data" } else { "" };
            return Err(YardError::new(ErrorCode::NeedsConfirmation, format!("Removing {what} needs a yes"))
                .with_hint(format!("yard service remove {instance}{flags} --yes"))
                .into());
        }
    }
    let params = ServiceRemoveParams { name: instance.clone(), keep_data };
    ctx.client().await?.call::<_, ()>(methods::SERVICES_REMOVE, params).await?;
    ctx.emit(&(), || match keep_data {
        true => format!("Removed {instance}. Its data stays in {dir}.\n"),
        false => format!("Removed {instance}.\n"),
    });
    Ok(0)
}

pub async fn start(ctx: &Ctx, instance: String) -> Outcome {
    let info: ServiceInfo =
        ctx.client().await?.call(methods::SERVICES_START, ServiceParams { name: instance }).await?;
    let info = wait_up(ctx, info, false).await?;
    ctx.emit(&info, || format!("Started {} on {}.\n", info.name, address(&info)));
    Ok(0)
}

pub async fn stop(ctx: &Ctx, instance: String) -> Outcome {
    let info: ServiceInfo =
        ctx.client().await?.call(methods::SERVICES_STOP, ServiceParams { name: instance }).await?;
    ctx.emit(&info, || format!("Stopped {}.\n", info.name));
    Ok(0)
}

pub async fn restart(ctx: &Ctx, instance: String) -> Outcome {
    let info: ServiceInfo =
        ctx.client().await?.call(methods::SERVICES_RESTART, ServiceParams { name: instance }).await?;
    let info = wait_up(ctx, info, false).await?;
    ctx.emit(&info, || format!("Restarted {} on {}.\n", info.name, address(&info)));
    Ok(0)
}

pub async fn port(ctx: &Ctx, instance: String, port: u16) -> Outcome {
    let params = ServicePortParams { name: instance, port };
    let info: ServiceInfo = ctx.client().await?.call(methods::SERVICES_PORT, params).await?;
    let info = wait_up(ctx, info, true).await?;
    ctx.emit(&info, || match info.state {
        // A sleeping instance keeps the new port and takes it when something wakes it.
        ServiceState::Stopped if info.sleeping_since.is_some() => {
            format!("{} will listen on {} when it wakes.\n", info.name, address(&info))
        }
        ServiceState::Stopped => format!(
            "{0} will listen on {1} once it is started: yard service start {0}\n",
            info.name,
            address(&info)
        ),
        _ => format!("{} now listens on {}.\n", info.name, address(&info)),
    });
    Ok(0)
}

/// `yard service sleep-after`: sets or clears the instance's `sleep_after` in `services.toml`.
pub async fn sleep_after(ctx: &Ctx, instance: String, after: String) -> Outcome {
    let params = ServiceSleepParams { name: instance, after: after.clone() };
    let info: ServiceInfo = ctx.client().await?.call(methods::SERVICES_SLEEP_SET, params).await?;
    ctx.emit(&info, || match (after.as_str(), info.sleep_after_secs) {
        ("default", _) if info.sleep_after_source == SettingSource::Default => {
            format!("{} now follows the default ({})\n", info.name, sleep_text(&info))
        }
        ("default", _) => format!("{} now follows yard.toml ({})\n", info.name, sleep_text(&info)),
        (_, None) => format!("{} never sleeps\n", info.name),
        (_, Some(_)) => format!("{} now sleeps after {} without use\n", info.name, sleep_text(&info)),
    });
    Ok(0)
}

/// `start`, `restart` and `port` answer while the instance is still `starting`: polls
/// `services.list` until it is `running`, or, with `stopped_ok` (`port`), `stopped` by the
/// user or asleep, which a port move on such an instance leaves it. A `failed` instance is the command's error, with its last log
/// lines printed first, and so is one `stopped` that the user did not stop, which yardd runs
/// nothing for: nothing here ever reports an instance up that is not `running`. One still
/// starting after `UP_WAIT` is an error too.
async fn wait_up(ctx: &Ctx, mut info: ServiceInfo, stopped_ok: bool) -> Result<ServiceInfo, CliError> {
    let deadline = Instant::now() + UP_WAIT;
    let mut client = ctx.client().await?;
    loop {
        match &info.state {
            ServiceState::Running => return Ok(info),
            ServiceState::Stopped
                if stopped_ok && (info.stopped_by_user || info.sleeping_since.is_some()) =>
            {
                return Ok(info);
            }
            ServiceState::Stopped => {
                return Err(YardError::new(
                    ErrorCode::JobFailed,
                    format!("{} did not start: yardd runs nothing for it", info.name),
                )
                .with_hint("yard doctor says why")
                .into());
            }
            ServiceState::Failed { last_lines, reason } => {
                // A held port or a missing keg failed it before it ran: its log is not why.
                let before_running = reason
                    .as_ref()
                    .is_some_and(|r| matches!(r.code, ErrorCode::PortInUse | ErrorCode::FormulaMissing));
                if !ctx.json && !before_running {
                    let skip = last_lines.len().saturating_sub(SHOWN_LINES);
                    for line in &last_lines[skip..] {
                        eprintln!("  {line}");
                    }
                }
                let error = info.state.failure(&info.name).unwrap_or_else(|| {
                    YardError::new(ErrorCode::JobFailed, format!("{} failed to start", info.name))
                });
                return Err(error.into());
            }
            // A state a newer yardd added: keep asking until it is one this client knows.
            ServiceState::Starting | ServiceState::Other => {}
        }
        if Instant::now() >= deadline {
            return Err(YardError::new(
                ErrorCode::JobFailed,
                format!("{} did not come up within {} s", info.name, UP_WAIT.as_secs()),
            )
            .with_hint(format!("yard service logs {}", info.name))
            .into());
        }
        tokio::time::sleep(POLL).await;
        let list: Vec<ServiceInfo> = client.call(methods::SERVICES_LIST, Value::Null).await?;
        let name = info.name.clone();
        info = list.into_iter().find(|s| s.name == name).ok_or_else(|| {
            YardError::new(ErrorCode::ServiceNotFound, format!("no service instance named {name:?}"))
                .with_hint("yard services lists the instances")
        })?;
    }
}

/// The tail of the instance's log; with `follow`, then each new line of it until Ctrl-C.
/// It subscribes before it asks for the tail, so no line falls between the two.
pub async fn logs(ctx: &Ctx, instance: String, follow: bool, lines: Option<usize>) -> Outcome {
    let mut client = ctx.client().await?;
    let params = ServiceLogsParams { name: instance.clone(), lines };
    if !follow {
        // No handler here: a plain Ctrl-C must still kill a command stuck on a slow daemon.
        let tail: Vec<String> = client.call(methods::SERVICES_LOGS, params).await?;
        ctx.emit(&tail, || tail.iter().map(|l| format!("{l}\n")).collect());
        return Ok(0);
    }
    // Registered before any output, so a Ctrl-C ends the follow cleanly, never as a kill.
    let mut interrupt = signal(SignalKind::interrupt())
        .map_err(|e| YardError::internal(format!("cannot listen for Ctrl-C: {e}")))?;
    client.subscribe(&[Topic::Logs]).await?;
    let tail: Vec<String> = client.call(methods::SERVICES_LOGS, params).await?;
    let show = |line: &str| {
        if ctx.json {
            print_json_line(&Event::LogLine { unit: instance.clone(), line: line.to_string() });
        } else {
            println!("{line}");
        }
    };
    tail.iter().for_each(|l| show(l));
    loop {
        tokio::select! {
            event = client.next_event() => {
                if let Event::LogLine { unit, line } = event?
                    && unit == instance
                {
                    show(&line);
                }
            }
            _ = interrupt.recv() => return Ok(0),
        }
    }
}
