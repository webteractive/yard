//! `yard services` and `yard service add|remove|start|stop|restart|logs|port`.

use super::sources::with_trust;
use super::{Ctx, Outcome, confirm, env, finished, interactive, run_job};
use crate::client::CliError;
use crate::output::{for_agents, print_json_line, table};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::signal::unix::{SignalKind, signal};
use yard_proto::methods::{
    self, EnvParams, ServiceAddParams, ServiceLogsParams, ServiceParams, ServicePortParams,
    ServiceRemoveParams,
};
use yard_proto::{Account, EnvBlock, ErrorCode, Event, ServiceInfo, ServiceState, Topic, YardError};

/// How long `start` and `restart` wait for an instance to come up: the daemon gives a new
/// instance 60 s to pass its health check, and so does this.
const UP_WAIT: Duration = Duration::from_secs(60);
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

pub fn state_name(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "running",
        ServiceState::Starting => "starting",
        ServiceState::Stopped => "stopped",
        ServiceState::Failed { .. } => "failed",
    }
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
                "No services yet. Add one with yard service add <engine>: mysql, postgres, redis or minio.\n\n{}",
                for_agents("yard services")
            );
        }
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
                    state_name(&s.state).into(),
                    account_text(&s.account),
                    used_by,
                ]
            })
            .collect();
        let mut out = table(&["INSTANCE", "ENGINE", "VERSION", "PORT", "STATE", "ACCOUNT", "USED BY"], &rows);
        out.push('\n');
        out.push_str(&for_agents("yard services"));
        out
    });
    Ok(0)
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
    let params = ServiceAddParams { engine: engine.into(), version: version.map(Into::into), port };
    finished(with_trust(ctx, || run_job(ctx, methods::SERVICES_ADD, params.clone(), String::new)).await)?;
    if ctx.json {
        return Ok(());
    }
    let created = instances(ctx).await?.into_iter().find(|s| s.engine == engine && !before.contains(&s.name));
    let Some(created) = created else {
        println!("Added {engine}.");
        return Ok(());
    };
    println!("Added {}. For your .env:", created.name);
    let params = EnvParams { service: Some(created.name), ..Default::default() };
    let blocks: Vec<EnvBlock> = ctx.client().await?.call(methods::ENV_PRINT, params).await?;
    print!("{}", env::human(&blocks));
    Ok(())
}

/// The question, then the delete. Without a terminal and without `--yes` it deletes nothing.
pub async fn remove(ctx: &Ctx, instance: String, keep_data: bool, yes: bool) -> Outcome {
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
    let info = wait_up(ctx, info).await?;
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
    let info = wait_up(ctx, info).await?;
    ctx.emit(&info, || format!("Restarted {} on {}.\n", info.name, address(&info)));
    Ok(0)
}

pub async fn port(ctx: &Ctx, instance: String, port: u16) -> Outcome {
    let params = ServicePortParams { name: instance, port };
    let info: ServiceInfo = ctx.client().await?.call(methods::SERVICES_PORT, params).await?;
    let info = wait_up(ctx, info).await?;
    ctx.emit(&info, || match info.state {
        ServiceState::Stopped => format!(
            "{0} will listen on {1} once it is started: yard service start {0}\n",
            info.name,
            address(&info)
        ),
        _ => format!("{} now listens on {}.\n", info.name, address(&info)),
    });
    Ok(0)
}

/// `start`, `restart` and `port` answer while the instance is still `starting`: polls
/// `services.list` until it is `running` (or `stopped`, which a port move on a stopped
/// instance leaves it). A `failed` instance is the command's error, with its last log
/// lines printed first; one still starting after `UP_WAIT` is an error too.
async fn wait_up(ctx: &Ctx, mut info: ServiceInfo) -> Result<ServiceInfo, CliError> {
    let deadline = Instant::now() + UP_WAIT;
    let mut client = ctx.client().await?;
    loop {
        match &info.state {
            ServiceState::Running | ServiceState::Stopped => return Ok(info),
            ServiceState::Failed { last_lines } => {
                if !ctx.json {
                    let skip = last_lines.len().saturating_sub(SHOWN_LINES);
                    for line in &last_lines[skip..] {
                        eprintln!("  {line}");
                    }
                }
                let last =
                    last_lines.iter().rev().find(|l| !l.trim().is_empty()).map_or("no output", |l| l.trim());
                return Err(YardError::new(
                    ErrorCode::JobFailed,
                    format!("{} failed to start: {last}", info.name),
                )
                .with_hint(format!("yard service logs {}", info.name))
                .with_data(serde_json::json!({ "last_lines": last_lines }))
                .into());
            }
            ServiceState::Starting => {}
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
    // Registered before any output, so a Ctrl-C is never the default kill.
    let mut interrupt = signal(SignalKind::interrupt())
        .map_err(|e| YardError::internal(format!("cannot listen for Ctrl-C: {e}")))?;
    if follow {
        client.subscribe(&[Topic::Logs]).await?;
    }
    let tail: Vec<String> =
        client.call(methods::SERVICES_LOGS, ServiceLogsParams { name: instance.clone(), lines }).await?;
    if !follow {
        ctx.emit(&tail, || tail.iter().map(|l| format!("{l}\n")).collect());
        return Ok(0);
    }
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
