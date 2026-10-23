//! `services.*`: the service instances in `services.toml` (M4b spec §4 to §8). `add` is a
//! job; the handler checks what it can first, so the CLI gets ordinary errors it can act on
//! (`FORMULA_UNTRUSTED` in particular), and the job re-checks what a job queued behind
//! another can no longer rely on (`crate::services::Add`).

use super::handlers::{reconcile, snapshot};
use crate::daemon::Context;
use crate::services::{self, Add, Resolved};
use crate::sources::require_trusted;
use yard_core::config::load_services;
use yard_core::project::FILE as PROJECT_FILE;
use yard_core::services::{Engine, instance_name};
use yard_core::sources::tap_of;
use yard_proto::methods::{
    ServiceAddParams, ServiceLogsParams, ServiceParams, ServicePortParams, ServiceRemoveParams,
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
    let engine = Engine::from_key(&p.engine).ok_or_else(|| {
        bad(format!("{:?} is not a service Yard runs", p.engine))
            .with_hint("the engines are mysql, postgres, redis and minio")
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
    Ok(snap
        .services
        .services
        .keys()
        .filter_map(|name| services::service_info(&snap, supervisor, name))
        .collect())
}

/// Refused with `SERVICE_IN_USE` while a linked site's `.yard` asks for the instance: the
/// hint is the edit that frees it. Confirming the delete is the CLI's.
pub(super) async fn remove(ctx: &Context, p: ServiceRemoveParams) -> Result<(), YardError> {
    let services = load_services(&ctx.paths)?;
    let Some(entry) = services.services.get(&p.name) else {
        return Err(services::not_found(&p.name));
    };
    let snap = snapshot(ctx).await?;
    let sites = services::used_by(&snap, entry.engine, &p.name);
    if !sites.is_empty() {
        let files: Vec<String> = sites
            .iter()
            .filter_map(|site| snap.sites_cfg.sites.get(site)?.path.as_ref())
            .map(|path| path.join(PROJECT_FILE).display().to_string())
            .collect();
        let message = format!("{} is used by {}, from its .yard", p.name, sites.join(", "));
        let mut refused = YardError::new(ErrorCode::ServiceInUse, message)
            .with_data(serde_json::json!({ "sites": sites, "files": files }));
        if let Some(file) = files.first() {
            refused = refused.with_hint(format!("edit {file}"));
        }
        return Err(refused);
    }
    services::remove(&ctx.paths, &ctx.reconciler, entry.engine, &p.name, p.keep_data).await
}

/// The lines `services.logs` returns when the caller names no count.
const LOG_LINES: usize = 100;

/// The `ServiceInfo` of `name` as it is now. A unit spawned a moment ago has not left its
/// initial `Stopped` yet, and an instance the user did not stop is not stopped: it is
/// starting.
async fn info(ctx: &Context, name: &str) -> Result<ServiceInfo, YardError> {
    let snap = snapshot(ctx).await?;
    let supervisor = ctx.supervisor();
    let mut info =
        services::service_info(&snap, supervisor, name).ok_or_else(|| services::not_found(name))?;
    if info.state == ServiceState::Stopped
        && !info.stopped_by_user
        && supervisor.list().iter().any(|u| u.name == name)
    {
        info.state = ServiceState::Starting;
    }
    Ok(info)
}

/// The engine of the instance `services.toml` lists as `name`, read from the file as it is now.
fn engine_of(ctx: &Context, name: &str) -> Result<Engine, YardError> {
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
    engine_of(ctx, &p.name)?;
    services::set_stopped(&ctx.paths, &p.name, stopped)?;
    reconcile(ctx).await?;
    info(ctx, &p.name).await
}

/// `services.restart`: `units.restart` for the instance. A stopped one is the user's to
/// start, and a restart never clears `stopped`.
pub(super) async fn restart(ctx: &Context, p: ServiceParams) -> Result<ServiceInfo, YardError> {
    engine_of(ctx, &p.name)?;
    let current = info(ctx, &p.name).await?;
    if current.stopped_by_user {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{} is stopped, so there is nothing to restart", p.name),
        )
        .with_hint(format!("yard service start {}", p.name)));
    }
    ctx.supervisor().restart(&p.name).await?;
    info(ctx, &p.name).await
}

/// `services.logs`: the tail of `logs/<instance>.log`, empty while there is no file.
pub(super) async fn logs(ctx: &Context, p: ServiceLogsParams) -> Result<Vec<String>, YardError> {
    engine_of(ctx, &p.name)?;
    let (path, lines) = (ctx.paths.log(&p.name), p.lines.unwrap_or(LOG_LINES));
    tokio::task::spawn_blocking(move || services::tail(&path, lines)).await.map_err(YardError::internal)?
}

/// `services.port`: moves the instance's first port (MinIO's API port; the console is
/// `yard ports set minio-console`), then reconciles, which restarts a running instance on
/// it. A stopped instance's port moves too and it stays stopped.
pub(super) async fn port(ctx: &Context, p: ServicePortParams) -> Result<ServiceInfo, YardError> {
    let engine = engine_of(ctx, &p.name)?;
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
