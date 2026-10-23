//! `services.*`: the service instances in `services.toml` (M4b spec §4 to §8). `add` is a
//! job; the handler checks what it can first, so the CLI gets ordinary errors it can act on
//! (`FORMULA_UNTRUSTED` in particular), and the job re-checks what a job queued behind
//! another can no longer rely on (`crate::services::Add`).

use super::handlers::snapshot;
use crate::daemon::Context;
use crate::services::{self, Add, Resolved};
use crate::sources::require_trusted;
use yard_core::config::load_services;
use yard_core::project::FILE as PROJECT_FILE;
use yard_core::services::{Engine, instance_name};
use yard_core::sources::tap_of;
use yard_proto::methods::{ServiceAddParams, ServiceRemoveParams};
use yard_proto::{ErrorCode, JobStarted, ServiceInfo, YardError};

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
