//! `engines.list` and `engines.install` (protocol 2.13, recipes spec §9, §11).

use super::handlers::snapshot;
use crate::daemon::Context;
use yard_core::config::{TomlFile, load_services};
use yard_core::services::engines::{Origin, ServiceEngine, SourceSpec};
use yard_proto::methods::EngineInstallParams;
use yard_proto::{
    EngineInfo, EngineOrigin, EnginePort, EngineSourceKind, EnginesListResult, ErrorCode, JobStarted,
    YardError,
};

fn info(ctx: &Context, engine: &dyn ServiceEngine, stale: Option<YardError>) -> EngineInfo {
    let source = engine.source();
    EngineInfo {
        key: engine.key().to_string(),
        name: engine.display().to_string(),
        description: engine.description().to_string(),
        source: match source {
            SourceSpec::Brew { .. } | SourceSpec::BrewLines(_) => EngineSourceKind::Brew,
            SourceSpec::Path { .. } => EngineSourceKind::Path,
            SourceSpec::Download { .. } => EngineSourceKind::Download,
            SourceSpec::Npm { .. } => EngineSourceKind::Npm,
        },
        origin: match engine.origin() {
            Origin::BuiltIn => EngineOrigin::BuiltIn,
            Origin::Carried => EngineOrigin::Carried,
            Origin::User(file) => EngineOrigin::User { file: file.display().to_string() },
        },
        installed: crate::programs::installed(engine, &ctx.brew.prefix, &ctx.paths),
        error: stale,
        ports: engine
            .ports()
            .into_iter()
            .map(|p| EnginePort {
                name: if p.suffix.is_empty() { "main".into() } else { p.suffix },
                usual: p.usual,
            })
            .collect(),
        console: engine.console().is_some(),
        databases: engine.databases().is_some(),
        connect: engine.has_connect(),
        version: match source {
            SourceSpec::Download { version, .. } | SourceSpec::Npm { version, .. } => Some(version),
            _ => None,
        },
    }
}

pub(super) async fn list(ctx: &Context) -> Result<EnginesListResult, YardError> {
    let snap = snapshot(ctx).await?;
    let engines = snap
        .engines
        .all()
        .map(|e| info(ctx, e.as_ref(), snap.engines.stale(e.key()).map(|s| s.to_yard_error())))
        .collect();
    let errors = snap.engines.errors().iter().map(|e| e.to_yard_error()).collect();
    Ok(EnginesListResult { engines, errors })
}

/// Installs what the engine's recipe pins and moves its instances onto it (recipes spec §5.1),
/// then sweeps the install roots nothing names any more.
pub(super) async fn install(ctx: &Context, p: EngineInstallParams) -> Result<JobStarted, YardError> {
    let snap = snapshot(ctx).await?;
    let engine = snap.engines.get(&p.key).cloned().ok_or_else(|| {
        YardError::new(ErrorCode::EngineUnknown, format!("no engine named {}", p.key))
            .with_hint("yard engines")
    })?;
    if let SourceSpec::Brew { formula } = engine.source() {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("brew installs {}", engine.display()),
        )
        .with_hint(format!("brew reinstall {formula}")));
    }
    if let SourceSpec::BrewLines(_) = engine.source() {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("brew installs {}", engine.display()),
        )
        .with_hint(format!("yard service add {}", engine.key())));
    }
    let (paths, reconciler, opts, brew) =
        (ctx.paths.clone(), ctx.reconciler.clone(), ctx.recipe_options.clone(), ctx.brew.clone());
    Ok(ctx.jobs.start("engine-install", move |job| async move {
        let home = reconciler.home().to_path_buf();
        let recorded = crate::programs::install(&job, &paths, &brew, &home, engine.as_ref(), &opts).await?;
        job.check_cancelled()?;
        if let Some(version) = recorded {
            let instances: Vec<String> = load_services(&paths)?
                .services
                .into_iter()
                .filter(|(_, e)| e.engine.as_str() == engine.key())
                .map(|(name, _)| name)
                .collect();
            for instance in &instances {
                TomlFile::edit(&paths.services_toml(), |file| {
                    file.set(&["services", instance, "version"], toml_edit::value(version.as_str()));
                    // A download or npm entry records its version and no formula: one switched
                    // from a brew recipe would not parse with both.
                    file.remove(&["services", instance, "formula"]);
                    Ok(())
                })?;
                job.line(format!("{instance} now runs {} {version}", engine.display()));
            }
            reconciler.reconcile().await?;
        }
        if let Some(snap) = reconciler.snapshot().await {
            for gone in crate::programs::sweep(&paths, &snap.services, &snap.engines) {
                job.line(format!("removed {}", gone.display()));
            }
        }
        Ok(())
    }))
}
