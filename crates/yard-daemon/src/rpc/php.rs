//! `php.*`: the PHP versions brew has installed, installing and removing them as jobs,
//! and the global default (spec §4.1).

use super::handlers::{not_found, reconcile, refuse_project_file, site, snapshot};
use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::reconcile::{USER_OWNED_INI, fpm_unit};
use std::path::Path;
use toml_edit::value;
use yard_core::brew::Brew;
use yard_core::config::yard::ExtSource;
use yard_core::config::{TomlFile, load_sites, load_yard};
use yard_core::php::{SUPPORTED, formula};
use yard_core::{Paths, PhpVersion};
use yard_core::{generated, ini};
use yard_proto::methods::{IsolateParams, PhpConfigParams, PhpParams};
use yard_proto::{ErrorCode, JobStarted, PhpInfo, SiteInfo, SiteKind, YardError};

fn installed(brew: &Brew) -> Vec<PhpVersion> {
    brew.installed_php(&|p: &Path| p.exists(), &|p: &Path| std::fs::read_link(p).ok())
}

fn not_installed(v: PhpVersion) -> YardError {
    YardError::new(ErrorCode::PhpNotInstalled, format!("PHP {v} is not installed"))
        .with_hint(format!("yard php install {v}"))
}

pub(super) async fn list(ctx: &Context) -> Result<Vec<PhpInfo>, YardError> {
    let snap = snapshot(ctx).await?;
    let units = ctx.supervisor().list();
    let mut versions = installed(&ctx.brew);
    let default = snap.yard.default_php;
    if !versions.contains(&default) {
        versions.push(default);
        versions.sort();
    }
    let installed = installed(&ctx.brew);
    Ok(versions
        .into_iter()
        .map(|v| PhpInfo {
            version: v.to_string(),
            default: v == default,
            installed: installed.contains(&v),
            sites: snap
                .resolution
                .sites
                .iter()
                .filter(|s| matches!(&s.kind, SiteKind::Php { php, .. } if *php == v.to_string()))
                .map(|s| s.name.clone())
                .collect(),
            pool: units.iter().find(|u| u.name == fpm_unit(v)).map(|u| u.state.name().to_string()),
        })
        .collect())
}

pub(super) fn install(ctx: &Context, p: PhpParams) -> Result<JobStarted, YardError> {
    let v: PhpVersion = p.php.parse()?;
    let already = installed(&ctx.brew).contains(&v);
    let brew = ctx.brew.brew_bin();
    let reconciler = ctx.reconciler.clone();
    Ok(ctx.jobs.start("php-install", move |job: JobCtx| async move {
        if already {
            job.line(format!("PHP {v} is already installed"));
        } else {
            job.run(&brew, &["install", &formula(v)], &Brew::job_env()).await?;
        }
        reconciler.reconcile().await.map(|_| ())
    }))
}

/// Refused, before any brew runs, while the version is the default or a site's. The job
/// checks again when it starts: it may wait behind another, and isolating a site or
/// changing the default does not.
pub(super) async fn uninstall(ctx: &Context, p: PhpParams) -> Result<JobStarted, YardError> {
    let v: PhpVersion = p.php.parse()?;
    let versions = installed(&ctx.brew);
    if !versions.contains(&v) {
        return Err(not_installed(v).with_hint("yard php list shows the installed versions"));
    }
    refuse_in_use(&ctx.paths, v, &versions)?;
    let (paths, brew) = (ctx.paths.clone(), ctx.brew.clone());
    let reconciler = ctx.reconciler.clone();
    Ok(ctx.jobs.start("php-uninstall", move |job: JobCtx| async move {
        refuse_in_use(&paths, v, &installed(&brew))?;
        job.run(&brew.brew_bin(), &["uninstall", &formula(v)], &Brew::job_env()).await?;
        // Bundled toggles are all that can be left: they go with the version.
        TomlFile::edit(&paths.yard_toml(), |file| Ok(file.remove(&["php", &v.to_string()])))?;
        reconciler.reconcile().await.map(|_| ())
    }))
}

/// `PHP_IN_USE` while `v` is the default, a site is isolated to it, or an extension Yard
/// installed for it is recorded.
fn refuse_in_use(paths: &Paths, v: PhpVersion, versions: &[PhpVersion]) -> Result<(), YardError> {
    let (yard, _) = load_yard(paths)?;
    if yard.default_php == v {
        let hint = match versions.iter().rev().find(|o| **o != v) {
            Some(o) => format!("yard php default {o}"),
            None => {
                let other =
                    SUPPORTED.iter().rev().find(|o| **o != v).expect("Yard supports more than one PHP");
                format!("yard php install {other}")
            }
        };
        return Err(
            YardError::new(ErrorCode::PhpInUse, format!("PHP {v} is the default version")).with_hint(hint)
        );
    }
    let isolated: Vec<String> = load_sites(paths)?
        .sites
        .into_iter()
        .filter(|(_, e)| e.php == Some(v))
        .map(|(name, _)| name)
        .collect();
    if let Some(first) = isolated.first() {
        let message = format!("PHP {v} runs {}, isolated to it", isolated.join(", "));
        return Err(
            YardError::new(ErrorCode::PhpInUse, message).with_hint(format!("yard unisolate --site {first}"))
        );
    }
    let added: Vec<&String> = yard
        .extensions
        .get(&v)
        .into_iter()
        .flatten()
        .filter(|(_, e)| e.source != ExtSource::Bundled)
        .map(|(name, _)| name)
        .collect();
    if let Some(first) = added.first() {
        let names: Vec<&str> = added.iter().map(|n| n.as_str()).collect();
        let message = format!("PHP {v} has extensions Yard added: {}", names.join(", "));
        return Err(YardError::new(ErrorCode::PhpInUse, message)
            .with_hint(format!("yard ext remove {first} --php {v}")));
    }
    Ok(())
}

pub(super) async fn set_default(ctx: &Context, p: PhpParams) -> Result<(), YardError> {
    let v: PhpVersion = p.php.parse()?;
    if !installed(&ctx.brew).contains(&v) {
        return Err(not_installed(v));
    }
    TomlFile::edit(&ctx.paths.yard_toml(), |file| {
        file.set(&["default_php"], value(v.to_string()));
        Ok(())
    })?;
    reconcile(ctx).await
}

/// `yard isolate` and `unisolate`: a linked site's own version, or back to the default.
pub(super) async fn isolate(ctx: &Context, p: IsolateParams) -> Result<SiteInfo, YardError> {
    let change = match &p.php {
        Some(v) => format!("php = {v:?}"),
        None => "remove php".to_string(),
    };
    refuse_project_file(ctx, &p.site, &change)?;
    let version = p.php.as_deref().map(str::parse::<PhpVersion>).transpose()?;
    TomlFile::edit(&ctx.paths.sites_toml(), |file| {
        let entry = file.doc().get("sites").and_then(|s| s.get(&p.site));
        if entry.is_none() {
            return Err(not_found(&p.site));
        }
        if entry.and_then(|e| e.get("path")).is_none() {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!("{} is a proxy, so no PHP runs it", p.site),
            )
            .with_hint("yard sites shows which sites run PHP"));
        }
        match version {
            Some(v) if !installed(&ctx.brew).contains(&v) => return Err(not_installed(v)),
            Some(v) => file.set(&["sites", &p.site, "php"], value(v.to_string())),
            None => {
                file.remove(&["sites", &p.site, "php"]);
            }
        }
        Ok(())
    })?;
    reconcile(ctx).await?;
    site(ctx, &p.site).await
}

/// The version a `php.config.*` call means: the one given, else the default. Installed.
pub(super) fn config_version(ctx: &Context, php: Option<&str>) -> Result<PhpVersion, YardError> {
    let v = match php {
        Some(v) => v.parse()?,
        None => load_yard(&ctx.paths)?.0.default_php,
    };
    if installed(&ctx.brew).contains(&v) { Ok(v) } else { Err(not_installed(v)) }
}

fn bad(message: String) -> YardError {
    YardError::new(ErrorCode::InvalidArgument, message)
}

pub(super) fn config_get(ctx: &Context, p: PhpConfigParams) -> Result<Option<String>, YardError> {
    let v = config_version(ctx, p.php.as_deref())?;
    let text = std::fs::read_to_string(ctx.paths.php_overrides(v)).unwrap_or_default();
    Ok(ini::get(&text, &p.key))
}

/// `set` (with `p.value`) or `unset` on `99-overrides.ini`, then a graceful reload of
/// the version's pool if it runs. The only place Yard writes that file. Whether it
/// changed: unsetting a key that is not set is `false`, and touches nothing.
pub(super) async fn config_edit(ctx: &Context, p: PhpConfigParams, set: bool) -> Result<bool, YardError> {
    if !ini::valid_key(&p.key) {
        return Err(bad(format!("{:?} is not a php.ini directive name", p.key))
            .with_hint("names are letters, digits, _ and ., e.g. memory_limit"));
    }
    let value = match (set, p.value.as_deref()) {
        (true, Some(value)) if !ini::valid_value(value) => {
            return Err(bad(format!("{value:?} cannot be a php.ini value: it holds a newline or a quote")));
        }
        (true, None) => return Err(bad("php.config.set needs a value".into())),
        (_, value) => value.unwrap_or_default(),
    };
    let v = config_version(ctx, p.php.as_deref())?;
    let file = ctx.paths.php_overrides(v);
    let io = |e: std::io::Error| YardError::internal(format!("{}: {e}", file.display()));
    generated::ensure_user_file(&file, USER_OWNED_INI).map_err(io)?;
    let text = std::fs::read_to_string(&file).map_err(io)?;
    let edited = if set {
        ini::set(&text, &p.key, value)
    } else {
        match ini::unset(&text, &p.key) {
            (edited, true) => edited,
            (_, false) => return Ok(false),
        }
    };
    generated::write_preserving(&file, &edited).map_err(io)?;
    let unit = fpm_unit(v);
    if ctx.supervisor().list().iter().any(|u| u.name == unit && u.pid.is_some()) {
        ctx.supervisor().reload(&unit).await?;
    }
    Ok(true)
}
