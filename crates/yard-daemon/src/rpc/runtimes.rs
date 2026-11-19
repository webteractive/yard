//! `runtimes.list` and `runtime.*` (runtimes spec §12, §13): every runtime through one set of
//! methods. PHP's and Node's go to their own `php.*`, `sites.isolate` and `node.*` handlers;
//! a recipe runtime's (Composer's) are here.

use super::handlers::{directory, reconcile, site, snapshot, snapshot_of};
use super::{node, php};
use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::runtimes::{fill_runtimes, in_use, installed, php_full, resolve_at, spec_for, unknown};
use std::path::Path;
use std::sync::Arc;
use toml_edit::value;
use yard_core::config::{TomlFile, load_sites, load_yard};
use yard_core::runtimes::recipe::{RuntimeRecipe, SourceDef};
use yard_core::runtimes::select::index_version;
use yard_core::runtimes::version::{RuntimeVersion, VersionSpec, pick};
use yard_core::runtimes::{Origin, Runtime, RuntimeKind};
use yard_proto::methods::{
    IsolateParams, NodeInstallParams, NodeListParams, NodeSpecParams, NodeUseParams, NodeVersionParams,
    PhpListParams, PhpParams, RuntimeInstallParams, RuntimeKeyParams, RuntimeSiteParams, RuntimeSpecParams,
    RuntimeUseParams, RuntimeVersionParams, SiteParams,
};
use yard_proto::{
    ErrorCode, JobStarted, RuntimeDefaultSet, RuntimeList, RuntimeSummary, RuntimeVersionInfo, RuntimesList,
    SiteInfo, YardError,
};

fn runtime(ctx: &Context, key: &str) -> Result<Arc<dyn Runtime>, YardError> {
    ctx.runtimes
        .get(key)
        .cloned()
        .ok_or_else(|| YardError { hint: Some("yard runtimes lists them".into()), ..unknown(key) })
}

fn spec_arg(spec: &str, key: &str) -> Result<VersionSpec, YardError> {
    VersionSpec::parse(spec).map_err(|why| {
        YardError::new(ErrorCode::InvalidArgument, why).with_hint(format!("yard runtime install {key} 2"))
    })
}

fn versions(ctx: &Context, key: &str) -> Vec<RuntimeVersion> {
    installed(&ctx.paths, key).into_iter().map(|(v, _)| v).collect()
}

pub(super) async fn list_all(ctx: &Context) -> Result<RuntimesList, YardError> {
    let (yard, _) = load_yard(&ctx.paths)?;
    let runtimes = ctx
        .runtimes
        .iter()
        .map(|rt| {
            let (installed, default) = match rt.kind() {
                RuntimeKind::Php => (
                    ctx.brew
                        .installed_php(&|p: &Path| p.exists(), &|p: &Path| std::fs::read_link(p).ok())
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    yard.default_php.map(|v| v.to_string()),
                ),
                RuntimeKind::Node => (
                    crate::node::installed(&ctx.paths).iter().map(ToString::to_string).collect(),
                    yard.default_node.as_ref().map(ToString::to_string),
                ),
                RuntimeKind::Recipe => (
                    versions(ctx, rt.key()).iter().map(ToString::to_string).collect(),
                    yard.runtime_defaults.get(rt.key()).map(ToString::to_string),
                ),
            };
            let (origin, file) = match rt.origin() {
                Origin::Carried => ("carried".to_string(), None),
                Origin::User(f) => ("user".to_string(), Some(f.display().to_string())),
            };
            RuntimeSummary {
                key: rt.key().to_string(),
                name: rt.display().to_string(),
                origin,
                file,
                shims: rt.shims(),
                installed,
                default,
            }
        })
        .collect();
    Ok(RuntimesList { runtimes, errors: vec![] })
}

pub(super) async fn list(ctx: &Context, p: RuntimeKeyParams) -> Result<RuntimeList, YardError> {
    let rt = runtime(ctx, &p.key)?;
    match rt.kind() {
        RuntimeKind::Php => {
            let list = php::list(ctx, PhpListParams { available: false }).await?;
            let default = list.iter().find(|i| i.default).map(|i| i.version.clone()).unwrap_or_default();
            let versions = list
                .into_iter()
                .filter(|i| i.installed)
                .map(|i| RuntimeVersionInfo { version: i.version, default: i.default, sites: i.sites })
                .collect();
            Ok(RuntimeList { key: p.key, default, versions })
        }
        RuntimeKind::Node => {
            let list = node::list(ctx, NodeListParams { available: false, major: None }).await?;
            let versions = list
                .versions
                .into_iter()
                .map(|i| RuntimeVersionInfo { version: i.version, default: i.default, sites: i.sites })
                .collect();
            let default = list.default;
            Ok(RuntimeList { key: p.key, default, versions })
        }
        RuntimeKind::Recipe => {
            let (yard, _) = load_yard(&ctx.paths)?;
            let configured = yard.runtime_defaults.get(&p.key);
            let installed = versions(ctx, &p.key);
            // What a project without a pin runs: the written default.
            let default_version = configured.and_then(|spec| pick(spec, &installed));
            let mut sites = snapshot(ctx).await?.resolution.sites;
            fill_runtimes(&mut sites, &ctx.paths, &ctx.runtimes, &ctx.brew);
            let versions = installed
                .iter()
                .rev()
                .map(|v| {
                    let version = v.to_string();
                    let sites = sites
                        .iter()
                        .filter(|s| {
                            s.runtimes
                                .iter()
                                .any(|r| r.key == p.key && r.version.as_deref() == Some(version.as_str()))
                        })
                        .map(|s| s.name.clone())
                        .collect();
                    RuntimeVersionInfo { default: default_version.as_ref() == Some(v), version, sites }
                })
                .collect();
            Ok(RuntimeList {
                default: configured.map(ToString::to_string).unwrap_or_default(),
                key: p.key,
                versions,
            })
        }
    }
}

/// The spec a recipe runtime installs for directory `dir`: its pin, else `yard.toml`'s
/// default where the PHP there runs it, else what Yard offers that PHP ([`spec_for`]).
fn spec_at(ctx: &Context, recipe: &RuntimeRecipe, dir: &Path) -> Result<Option<VersionSpec>, YardError> {
    let (yard, _) = load_yard(&ctx.paths)?;
    let sites = load_sites(&ctx.paths)?;
    let pin = resolve_at(dir, recipe, &sites, &ctx.paths)?;
    if let Some((spec, _)) = &pin {
        return Ok(Some(spec.clone()));
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (v, _) =
        yard_core::shim::resolve_version(dir, &yard, &sites, &ctx.paths, &crate::node::read_pin, &canonical)?;
    let php = php_full(&ctx.brew, v);
    Ok(spec_for(recipe, pin, &yard, &versions(ctx, &recipe.key), &php).map(|(spec, _)| spec))
}

pub(super) async fn install(ctx: &Context, p: RuntimeInstallParams) -> Result<JobStarted, YardError> {
    let rt = runtime(ctx, &p.key)?;
    match rt.kind() {
        RuntimeKind::Php => {
            let Some(spec) = p.spec else {
                return Err(YardError::new(ErrorCode::InvalidArgument, "name a PHP version")
                    .with_hint("yard php install 8.4"));
            };
            return php::install(ctx, PhpParams { php: spec }).await;
        }
        RuntimeKind::Node => {
            return node::install(ctx, NodeInstallParams { spec: p.spec, path: p.path }).await;
        }
        RuntimeKind::Recipe => {}
    }
    let recipe = rt.recipe().clone();
    let spec = match (&p.spec, &p.path) {
        (Some(spec), _) => Some(spec_arg(spec, &p.key)?),
        (None, Some(path)) => spec_at(ctx, &recipe, &directory(path)?)?,
        (None, None) => None,
    };
    let Some(spec) = spec else {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("name a {} version, or a directory whose pin to install", recipe.name),
        )
        .with_hint(format!("yard runtime install {} 2", p.key)));
    };
    if let SourceDef::Download { index: Some(_), .. } = &recipe.source
        && let Err(why) = index_version(&spec, &Default::default())
    {
        return Err(YardError::new(ErrorCode::InvalidArgument, why)
            .with_hint(format!("yard runtime install {} 2.7.7", p.key)));
    }
    let (paths, opts, reconciler) = (ctx.paths.clone(), ctx.recipe_options.clone(), ctx.reconciler.clone());
    Ok(ctx.jobs.start("runtime-install", move |job: JobCtx| async move {
        crate::runtimes::install(&job, &paths, &recipe, spec, &opts).await?;
        crate::defaults::reconcile_after_install(&job, &reconciler).await
    }))
}

/// Refused before any job while the default or a linked site has nothing else to run; the
/// job checks again when it starts, since it may wait behind another.
pub(super) async fn uninstall(ctx: &Context, p: RuntimeVersionParams) -> Result<JobStarted, YardError> {
    let rt = runtime(ctx, &p.key)?;
    match rt.kind() {
        RuntimeKind::Php => return php::uninstall(ctx, PhpParams { php: p.version }).await,
        RuntimeKind::Node => return node::uninstall(ctx, NodeVersionParams { version: p.version }).await,
        RuntimeKind::Recipe => {}
    }
    let recipe = rt.recipe().clone();
    let v: RuntimeVersion = p.version.parse().map_err(|why: String| {
        YardError::new(ErrorCode::InvalidArgument, why).with_hint(format!("yard runtime list {}", p.key))
    })?;
    if !versions(ctx, &p.key).contains(&v) {
        return Err(YardError::new(
            ErrorCode::RuntimeMissing,
            format!("{} {v} is not installed", recipe.name),
        )
        .with_hint(format!("yard runtime list {}", p.key)));
    }
    let check =
        |paths: &yard_core::Paths, runtimes, brew, mut sites: Vec<SiteInfo>| -> Result<(), YardError> {
            fill_runtimes(&mut sites, paths, runtimes, brew);
            let (yard, _) = load_yard(paths)?;
            in_use(paths, &recipe, &v, &sites, yard.runtime_defaults.get(&recipe.key))
        };
    check(&ctx.paths, &ctx.runtimes, &ctx.brew, snapshot(ctx).await?.resolution.sites)?;
    let (paths, runtimes, brew, reconciler) =
        (ctx.paths.clone(), ctx.runtimes.clone(), ctx.brew.clone(), ctx.reconciler.clone());
    let recipe = rt.recipe().clone();
    Ok(ctx.jobs.start("runtime-uninstall", move |job: JobCtx| async move {
        let mut sites = snapshot_of(&reconciler).await?.resolution.sites;
        fill_runtimes(&mut sites, &paths, &runtimes, &brew);
        let (yard, _) = load_yard(&paths)?;
        in_use(&paths, &recipe, &v, &sites, yard.runtime_defaults.get(&recipe.key))?;
        crate::runtimes::uninstall(&job, &paths, &recipe, &v).await
    }))
}

/// `yard runtime default`: only to a spec something installed meets.
pub(super) async fn set_default(ctx: &Context, p: RuntimeSpecParams) -> Result<RuntimeDefaultSet, YardError> {
    // What the move makes yardd write (pins for a `default_php` below 7.2.5) is reported, so
    // one command never edits sites' files unannounced.
    let before = ctx.reconciler.state().await.defaults_written.len();
    let mut set = set_default_only(ctx, p).await?;
    set.written =
        ctx.reconciler.state().await.defaults_written.iter().skip(before).map(|w| w.describe()).collect();
    Ok(set)
}

async fn set_default_only(ctx: &Context, p: RuntimeSpecParams) -> Result<RuntimeDefaultSet, YardError> {
    let rt = runtime(ctx, &p.key)?;
    match rt.kind() {
        RuntimeKind::Php => {
            php::set_default(ctx, PhpParams { php: p.spec }).await?;
            return Ok(RuntimeDefaultSet::default());
        }
        RuntimeKind::Node => {
            node::set_default(ctx, NodeSpecParams { spec: p.spec }).await?;
            reconcile(ctx).await?;
            return Ok(RuntimeDefaultSet::default());
        }
        RuntimeKind::Recipe => {}
    }
    let recipe = rt.recipe();
    let spec = spec_arg(&p.spec, &p.key)?;
    let installed = versions(ctx, &p.key);
    // A default is one exact version, never a prefix (explicit-defaults spec §2.4).
    let matching: Vec<String> =
        installed.iter().rev().filter(|v| v.prefix_matches(&spec)).map(ToString::to_string).collect();
    let Some(newest) = matching.first() else {
        return Err(crate::runtimes::missing_spec(recipe, &spec));
    };
    if !spec.is_exact() {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!(
                "a default is an exact version, not {spec}; installed {} {spec}: {}",
                recipe.name,
                matching.join(", ")
            ),
        )
        .with_hint(format!("yard runtime default {} {newest}", p.key)));
    }
    TomlFile::edit(&ctx.paths.yard_toml(), |file| {
        file.set(&["runtimes", &p.key, "default"], value(spec.to_string()));
        Ok(())
    })?;
    let unfit = crate::defaults::unfit(&ctx.paths, &ctx.brew, recipe, Some(&spec));
    reconcile(ctx).await?;
    Ok(RuntimeDefaultSet { unfit, written: vec![] })
}

pub(super) async fn use_spec(ctx: &Context, p: RuntimeUseParams) -> Result<SiteInfo, YardError> {
    let rt = runtime(ctx, &p.key)?;
    match rt.kind() {
        RuntimeKind::Php => {
            return php::isolate(ctx, IsolateParams { site: p.site, php: Some(p.spec) }).await;
        }
        RuntimeKind::Node => return node::use_spec(ctx, NodeUseParams { site: p.site, spec: p.spec }).await,
        RuntimeKind::Recipe => {}
    }
    let spec = spec_arg(&p.spec, &p.key)?;
    set_site_runtime(ctx, &p.site, &p.key, Some(spec)).await
}

pub(super) async fn unuse(ctx: &Context, p: RuntimeSiteParams) -> Result<SiteInfo, YardError> {
    let rt = runtime(ctx, &p.key)?;
    match rt.kind() {
        RuntimeKind::Php => return php::isolate(ctx, IsolateParams { site: p.site, php: None }).await,
        RuntimeKind::Node => return node::unuse(ctx, SiteParams { site: p.site }).await,
        RuntimeKind::Recipe => {}
    }
    set_site_runtime(ctx, &p.site, &p.key, None).await
}

/// `[runtimes] <key>` in the site's managed `.yard`, else `[sites.<s>.runtimes] <key>` in
/// `sites.toml`; a committed `.yard` is the user's to edit.
async fn set_site_runtime(
    ctx: &Context,
    name: &str,
    key: &str,
    spec: Option<VersionSpec>,
) -> Result<SiteInfo, YardError> {
    let spec = spec.map(|s| s.to_string());
    crate::managed::set_site_runtime(&ctx.paths, name, key, spec.as_deref())?;
    site(ctx, name).await
}
