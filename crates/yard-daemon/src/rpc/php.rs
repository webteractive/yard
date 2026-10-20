//! `php.*`: the PHP versions brew has installed, installing and removing them as jobs,
//! and the global default (spec §4.1).

use super::handlers::{reconcile, snapshot};
use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::reconcile::fpm_unit;
use std::path::Path;
use toml_edit::value;
use yard_core::PhpVersion;
use yard_core::brew::Brew;
use yard_core::config::{TomlFile, load_yard};
use yard_core::php::formula;
use yard_proto::methods::PhpParams;
use yard_proto::{ErrorCode, JobStarted, PhpInfo, SiteKind, YardError};

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

/// Refused, before any brew runs, while the version is the default.
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
