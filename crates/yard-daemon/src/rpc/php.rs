//! `php.*`: the PHP versions brew has installed, installing and removing them as jobs,
//! and the global default (spec §4.1).

use super::handlers::{not_found, reconcile, site, snapshot};
use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::reconcile::fpm_unit;
use std::path::Path;
use toml_edit::value;
use yard_core::PhpVersion;
use yard_core::brew::Brew;
use yard_core::config::{TomlFile, load_sites, load_yard};
use yard_core::php::formula;
use yard_proto::methods::{IsolateParams, PhpParams};
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
            job.run(&brew, &["install", &formula(v)], &[]).await?;
        }
        reconciler.reconcile().await.map(|_| ())
    }))
}

/// Refused, before any brew runs, while the version is the default or a site's.
pub(super) async fn uninstall(ctx: &Context, p: PhpParams) -> Result<JobStarted, YardError> {
    let v: PhpVersion = p.php.parse()?;
    let versions = installed(&ctx.brew);
    if !versions.contains(&v) {
        return Err(not_installed(v).with_hint("yard php list shows the installed versions"));
    }
    let (yard, _) = load_yard(&ctx.paths)?;
    if yard.default_php == v {
        let other = versions.iter().rev().find(|o| **o != v);
        let hint = match other {
            Some(o) => format!("yard php default {o}"),
            None => format!("install another version first, then yard php default <it>; PHP {v} stays"),
        };
        return Err(
            YardError::new(ErrorCode::PhpInUse, format!("PHP {v} is the default version")).with_hint(hint)
        );
    }
    let isolated: Vec<String> = load_sites(&ctx.paths)?
        .sites
        .into_iter()
        .filter(|(_, e)| e.php == Some(v))
        .map(|(name, _)| name)
        .collect();
    if let Some(first) = isolated.first() {
        let message = format!("PHP {v} runs {}, isolated to it", isolated.join(", "));
        return Err(YardError::new(ErrorCode::PhpInUse, message)
            .with_hint(format!("yard isolate <version> --site {first}")));
    }
    let brew = ctx.brew.brew_bin();
    let reconciler = ctx.reconciler.clone();
    Ok(ctx.jobs.start("php-uninstall", move |job: JobCtx| async move {
        job.run(&brew, &["uninstall", &formula(v)], &[]).await?;
        reconciler.reconcile().await.map(|_| ())
    }))
}

pub(super) async fn set_default(ctx: &Context, p: PhpParams) -> Result<(), YardError> {
    let v: PhpVersion = p.php.parse()?;
    if !installed(&ctx.brew).contains(&v) {
        return Err(not_installed(v));
    }
    let mut file = TomlFile::load(&ctx.paths.yard_toml())?;
    file.set(&["default_php"], value(v.to_string()));
    file.save()?;
    reconcile(ctx).await
}

/// `yard isolate` and `unisolate`: a linked site's own version, or back to the default.
pub(super) async fn isolate(ctx: &Context, p: IsolateParams) -> Result<SiteInfo, YardError> {
    let version = p.php.as_deref().map(str::parse::<PhpVersion>).transpose()?;
    let mut file = TomlFile::load(&ctx.paths.sites_toml())?;
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
    file.save()?;
    reconcile(ctx).await?;
    site(ctx, &p.site).await
}
