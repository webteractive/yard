//! `services.*`: the service instances in `services.toml` (M4b spec §4 to §8). `add` is a
//! job; the handler checks what it can first, so the CLI gets ordinary errors it can act on
//! (`FORMULA_UNTRUSTED` in particular), and the job re-checks what a job queued behind
//! another can no longer rely on (`crate::services::Add`).

use super::emailulator;
use super::handlers::{reconcile, snapshot};
use crate::daemon::Context;
use crate::services::{self, Add, Resolved};
use crate::sleep::Sleeper;
use crate::sources::require_trusted;
use crate::state::Snapshot;
use crate::supervisor::Supervisor;
use yard_core::config::load_services;
use yard_core::services::sleep::SleepAfter;
use yard_core::services::{Engine, instance_name};
use yard_core::sources::tap_of;
use yard_proto::methods::{
    ServiceAddParams, ServiceLogsParams, ServiceParams, ServicePortParams, ServiceRemoveParams,
    ServiceSleepParams,
};
use yard_proto::{ErrorCode, JobStarted, ServiceInfo, ServiceState, YardError};

fn bad(message: String) -> YardError {
    YardError::new(ErrorCode::InvalidArgument, message)
}

/// The line `add` is asked for: the engine's default when none is given, `None` for an
/// engine that has no lines.
fn line_of(engine: Engine, version: Option<&str>) -> Result<Option<String>, YardError> {
    match (engine.versioned(), version) {
        (false, None) => Ok(None),
        (false, Some(v)) => Err(bad(format!("{} has no versions, so {v:?} means nothing", engine.display()))
            .with_hint(format!("yard service add {}", engine.key()))),
        (true, None) => Ok(engine.default_line().map(str::to_string)),
        (true, Some(v)) if engine.valid_line(v) => Ok(Some(v.to_string())),
        (true, Some(v)) => {
            let example = engine.default_line().unwrap_or_default();
            Err(bad(format!("{v:?} is not a {} release line", engine.display()))
                .with_hint(format!("yard service add {} --version {example}", engine.key())))
        }
    }
}

pub(super) async fn add(ctx: &Context, p: ServiceAddParams) -> Result<JobStarted, YardError> {
    if emailulator::is_emailulator(&p.engine) {
        return emailulator::add(ctx, p).await;
    }
    let engine = Engine::from_key(&p.engine).ok_or_else(|| {
        bad(format!("{:?} is not a service Yard runs", p.engine))
            .with_hint("the engines are mysql, postgres, redis, minio and emailulator")
    })?;
    let line = line_of(engine, p.version.as_deref())?;
    if p.port == Some(0) {
        return Err(bad("0 is not a port".into()).with_hint("pass a port such as --port 3307"));
    }
    let instance = instance_name(engine, line.as_deref());
    if load_services(&ctx.paths)?.services.contains_key(&instance) {
        return Err(services::exists(&instance));
    }
    let resolved = services::resolve(&ctx.brew, engine, line.as_deref()).await?;
    // Nothing is asked of brew for a keg that is already there, and homebrew/core needs no trust.
    let Resolved { formula, .. } = &resolved;
    if !services::keg(&ctx.brew, formula).exists() && tap_of(formula).is_some() {
        require_trusted(&ctx.brew, formula).await?;
    }
    let add = Add {
        paths: ctx.paths.clone(),
        brew: ctx.brew.clone(),
        reconciler: ctx.reconciler.clone(),
        engine,
        instance,
        version: line,
        resolved,
        port: p.port,
        usual_ports: ctx.usual_ports.clone(),
        app_dirs: ctx.app_dirs.clone(),
    };
    Ok(ctx.jobs.start("service-add", move |job| add.run(job)))
}

pub(super) async fn list(ctx: &Context) -> Result<Vec<ServiceInfo>, YardError> {
    let snap = snapshot(ctx).await?;
    let supervisor = ctx.supervisor();
    let mut list: Vec<ServiceInfo> = snap
        .services
        .services
        .keys()
        .filter_map(|name| services::service_info(&snap, supervisor, &ctx.sleeper, name))
        .collect();
    list.extend(emailulator::info(&snap));
    Ok(list)
}

/// Refused with `SERVICE_IN_USE` while a linked site's `.yard` asks for the instance: the
/// hint is the edit that frees it. Confirming the delete is the CLI's.
pub(super) async fn remove(ctx: &Context, p: ServiceRemoveParams) -> Result<(), YardError> {
    if emailulator::is_emailulator(&p.name) {
        return emailulator::remove(ctx).await;
    }
    let services = load_services(&ctx.paths)?;
    let Some(entry) = services.services.get(&p.name) else {
        return Err(services::not_found(&p.name));
    };
    let snap = snapshot(ctx).await?;
    let sites = services::used_by(&snap, entry.engine, &p.name);
    if !sites.is_empty() {
        let files: Vec<String> = sites
            .iter()
            .filter_map(|site| snap.projects.get(site))
            .map(|loaded| loaded.file.display().to_string())
            .collect();
        let message = format!("{} is used by {}, from its .yard", p.name, sites.join(", "));
        let mut refused = YardError::new(ErrorCode::ServiceInUse, message)
            .with_data(serde_json::json!({ "sites": sites, "files": files }));
        if let Some(file) = files.first() {
            refused = refused.with_hint(format!("edit {file}"));
        }
        return Err(refused);
    }
    // Its reconcile forgets the instance's sleep and drops its sites from `run/sleeping`.
    services::remove(&ctx.paths, &ctx.reconciler, entry.engine, &p.name, p.keep_data).await?;
    Ok(())
}

/// Wakes an asleep instance for `services.start` or `restart`: the caller's reconcile then
/// starts its unit.
fn wake_for_start(ctx: &Context, name: &str) {
    ctx.sleeper.touch(name);
    ctx.sleeper.wake(name);
}

/// The lines `services.logs` returns when the caller names no count.
const LOG_LINES: usize = 100;

/// The `ServiceInfo` of `name` as it is now. A unit spawned a moment ago has not left its
/// initial `Stopped` yet, and an instance the user did not stop is not stopped: it is
/// starting.
async fn info(ctx: &Context, name: &str) -> Result<ServiceInfo, YardError> {
    let snap = snapshot(ctx).await?;
    instance_info(&snap, ctx.supervisor(), &ctx.sleeper, name).ok_or_else(|| services::not_found(name))
}

/// `service_info` with the state a caller can act on: see `info`. `None` for a name
/// `services.toml` does not list. An asleep instance stays `Stopped`, even while its unit is
/// still stopping.
pub(super) fn instance_info(
    snap: &Snapshot,
    supervisor: &Supervisor,
    sleeper: &Sleeper,
    name: &str,
) -> Option<ServiceInfo> {
    let mut info = services::service_info(snap, supervisor, sleeper, name)?;
    if info.state == ServiceState::Stopped
        && !info.stopped_by_user
        && info.sleeping_since.is_none()
        && supervisor.list().iter().any(|u| u.name == name)
    {
        info.state = ServiceState::Starting;
    }
    Some(info)
}

/// The engine of the instance `services.toml` lists as `name`, read from the file as it is now.
/// emailulator is `SERVICE_EXTERNAL`: its LaunchAgent runs it, not Yard.
async fn engine_of(ctx: &Context, name: &str) -> Result<Engine, YardError> {
    if emailulator::is_emailulator(name) {
        return Err(emailulator::refuse(ctx, false).await);
    }
    load_services(&ctx.paths)?.services.get(name).map(|e| e.engine).ok_or_else(|| services::not_found(name))
}

/// `services.stop` and `services.start`: `stopped` is written through `TomlFile` (a start
/// removes the key), then reconcile stops or starts the unit. A start answers as soon as the
/// unit is spawned; its state says whether it is up yet.
pub(super) async fn set_stopped(
    ctx: &Context,
    p: ServiceParams,
    stopped: bool,
) -> Result<ServiceInfo, YardError> {
    engine_of(ctx, &p.name).await?;
    services::set_stopped(&ctx.paths, &p.name, stopped)?;
    if !stopped && ctx.sleeper.is_asleep(&p.name) {
        // A start of an asleep instance wakes it; the CLI waits for it as for any start.
        wake_for_start(ctx, &p.name);
    }
    // A stop's reconcile forgets the instance's sleep: it wakes with its next start.
    reconcile(ctx).await?;
    if !stopped {
        refuse_left_out(ctx, &p.name).await?;
    }
    info(ctx, &p.name).await
}

/// A start or restart of an instance reconcile started no unit for is refused with the
/// reason (`FORMULA_MISSING`, hint `brew install <formula>`), so nothing reports it started.
async fn refuse_left_out(ctx: &Context, name: &str) -> Result<(), YardError> {
    match snapshot(ctx).await?.left_out.get(name) {
        Some(why) => Err(why.clone()),
        None => Ok(()),
    }
}

/// `services.restart`: `units.restart` for the instance. A stopped one is the user's to
/// start, and a restart never clears `stopped`. It answers once the unit has taken the
/// restart, so the state is this attempt's (`starting`), never the failure it replaces or a
/// `running` that has not stopped yet.
pub(super) async fn restart(ctx: &Context, p: ServiceParams) -> Result<ServiceInfo, YardError> {
    engine_of(ctx, &p.name).await?;
    let current = info(ctx, &p.name).await?;
    if current.stopped_by_user {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{} is stopped, so there is nothing to restart", p.name),
        )
        .with_hint(format!("yard service start {}", p.name)));
    }
    refuse_left_out(ctx, &p.name).await?;
    // An asleep instance has no unit to restart: waking it starts it afresh (spec §5.3).
    if ctx.sleeper.is_asleep(&p.name) {
        wake_for_start(ctx, &p.name);
        reconcile(ctx).await?;
        refuse_left_out(ctx, &p.name).await?;
        return info(ctx, &p.name).await;
    }
    ctx.supervisor().restart_and_wait(&p.name).await?;
    info(ctx, &p.name).await
}

/// `services.sleep.set`: `after` is a duration of at least 60s, `off`, or `default`, which
/// removes the instance's key so `yard.toml` decides. A new setting restarts the idle clock.
pub(super) async fn set_sleep(ctx: &Context, p: ServiceSleepParams) -> Result<ServiceInfo, YardError> {
    engine_of(ctx, &p.name).await?;
    let after = match p.after.as_str() {
        "default" => None,
        text => Some(text.parse::<SleepAfter>().map_err(|why| {
            bad(format!("{why}; or write default to follow yard.toml"))
                .with_hint(format!("yard service sleep-after {} 15m", p.name))
        })?),
    };
    services::set_sleep_after(&ctx.paths, &p.name, after)?;
    ctx.sleeper.touch(&p.name);
    reconcile(ctx).await?;
    info(ctx, &p.name).await
}

/// `services.logs`: the tail of `logs/<instance>.log`, empty while there is no file.
/// emailulator's is the log its LaunchAgent writes.
pub(super) async fn logs(ctx: &Context, p: ServiceLogsParams) -> Result<Vec<String>, YardError> {
    let lines = p.lines.unwrap_or(LOG_LINES);
    if emailulator::is_emailulator(&p.name) {
        return emailulator::logs(ctx, lines).await;
    }
    engine_of(ctx, &p.name).await?;
    let path = ctx.paths.log(&p.name);
    tokio::task::spawn_blocking(move || services::tail(&path, lines)).await.map_err(YardError::internal)?
}

/// `services.port`: moves the instance's first port (MinIO's API port; the console is
/// `yard ports set minio-console`), then reconciles, which restarts a running instance on
/// it. A stopped instance's port moves too and it stays stopped.
pub(super) async fn port(ctx: &Context, p: ServicePortParams) -> Result<ServiceInfo, YardError> {
    let engine = engine_of(ctx, &p.name).await?;
    if p.port == 0 {
        return Err(bad("0 is not a port".into())
            .with_hint(format!("yard service port {} <port>, such as 3307", p.name)));
    }
    let (paths, name, port) = (ctx.paths.clone(), p.name.clone(), p.port);
    // The probe binds a socket and the holder lookup runs lsof.
    tokio::task::spawn_blocking(move || services::move_port(&paths, engine, &name, port))
        .await
        .map_err(YardError::internal)??;
    reconcile(ctx).await?;
    info(ctx, &p.name).await
}
