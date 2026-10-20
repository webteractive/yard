//! `yard sites`, `park`/`unpark`, `link`/`unlink`, `proxy`/`unproxy` and `docroot`.

use super::{Ctx, Outcome, cwd};
use crate::output::{for_agents, table};
use serde_json::Value;
use std::path::{Path, PathBuf};
use yard_core::sites::host_label;
use yard_proto::methods::{self, DocrootParams, LinkParams, NameParams, PathParams, ProxyParams};
use yard_proto::{ErrorCode, SiteInfo, SiteKind, SiteSource, YardError};

pub async fn list(ctx: &Ctx) -> Outcome {
    let sites: Vec<SiteInfo> = ctx.client().await?.call(methods::SITES_LIST, Value::Null).await?;
    ctx.emit(&sites, || {
        let mut out = if sites.is_empty() {
            "No sites yet. Park a directory with yard park <path> or link one with yard link.\n".to_string()
        } else {
            let rows: Vec<Vec<String>> = sites.iter().map(row).collect();
            table(&["SITE", "URL", "SOURCE", "TARGET", "PHP"], &rows)
        };
        out.push('\n');
        out.push_str(&for_agents("yard sites"));
        out
    });
    Ok(0)
}

fn row(site: &SiteInfo) -> Vec<String> {
    let source = match site.source {
        SiteSource::Parked => "parked",
        SiteSource::Linked => "linked",
        SiteSource::Proxy => "proxy",
    };
    let (target, php) = match &site.kind {
        // The directory nginx serves, so a `public/` that appears later shows up here.
        SiteKind::Php { docroot, php, .. } => (docroot.clone(), php.clone()),
        SiteKind::Proxy { target } => (target.clone(), "-".into()),
    };
    vec![site.name.clone(), site.url.clone(), source.into(), target, php]
}

pub async fn park(ctx: &Ctx, path: Option<PathBuf>, add: bool) -> Outcome {
    let dir = absolute(path)?;
    let method = if add { methods::SITES_PARK } else { methods::SITES_UNPARK };
    let params = PathParams { path: dir.display().to_string() };
    ctx.client().await?.call::<_, ()>(method, params).await?;
    ctx.emit(&(), || {
        if add {
            format!("Parked {}. yard sites lists the sites it serves.\n", dir.display())
        } else {
            format!("Unparked {}.\n", dir.display())
        }
    });
    Ok(0)
}

pub async fn link(ctx: &Ctx, name: Option<String>) -> Outcome {
    let dir = cwd()?;
    let name = name.map_or_else(|| name_of(&dir), Ok)?;
    let params = LinkParams { name, path: dir.display().to_string() };
    let site: SiteInfo = ctx.client().await?.call(methods::SITES_LINK, params).await?;
    ctx.emit(&site, || format!("Linked {} → {}\n", dir.display(), site.url));
    Ok(0)
}

pub async fn unlink(ctx: &Ctx, name: Option<String>) -> Outcome {
    let name = match name {
        Some(n) => n,
        None => name_of(&cwd()?)?,
    };
    ctx.client().await?.call::<_, ()>(methods::SITES_UNLINK, NameParams { name: name.clone() }).await?;
    ctx.emit(&(), || format!("Unlinked {name}.\n"));
    Ok(0)
}

pub async fn proxy(ctx: &Ctx, name: String, url: String) -> Outcome {
    let site: SiteInfo =
        ctx.client().await?.call(methods::SITES_PROXY, ProxyParams { name, url: url.clone() }).await?;
    ctx.emit(&site, || format!("Proxying {} → {url}\n", site.url));
    Ok(0)
}

pub async fn unproxy(ctx: &Ctx, name: String) -> Outcome {
    ctx.client().await?.call::<_, ()>(methods::SITES_UNPROXY, NameParams { name: name.clone() }).await?;
    ctx.emit(&(), || format!("Removed the proxy {name}.\n"));
    Ok(0)
}

pub async fn docroot(ctx: &Ctx, path: String, site: Option<String>) -> Outcome {
    let mut client = ctx.client().await?;
    let site = match site {
        Some(s) => s,
        None => {
            let sites: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
            containing(&sites, &cwd()?)?
        }
    };
    let params = DocrootParams { site, path: Some(path) };
    let updated: SiteInfo = client.call(methods::SITES_DOCROOT, params).await?;
    ctx.emit(&updated, || match &updated.kind {
        SiteKind::Php { docroot, .. } => format!("{} now serves {docroot}\n", updated.name),
        SiteKind::Proxy { target } => format!("{} proxies to {target}\n", updated.name),
    });
    Ok(0)
}

/// The PHP site whose directory holds `dir`, the innermost when sites nest.
fn containing(sites: &[SiteInfo], dir: &Path) -> Result<String, YardError> {
    sites
        .iter()
        .filter_map(|s| match &s.kind {
            SiteKind::Php { path, .. } if dir.starts_with(path) => Some((path.len(), &s.name)),
            _ => None,
        })
        .max()
        .map(|(_, name)| name.clone())
        .ok_or_else(|| {
            YardError::new(ErrorCode::SiteNotFound, format!("{} is not inside a site", dir.display()))
                .with_hint("yard docroot <path> --site <name>")
        })
}

/// A site name from a directory name, the way parked directories get theirs.
fn name_of(dir: &Path) -> Result<String, YardError> {
    let base = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    host_label(&base).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, format!("{base:?} does not make a site name"))
            .with_hint("pass one: yard link <name>")
    })
}

fn absolute(path: Option<PathBuf>) -> Result<PathBuf, YardError> {
    let path = match path {
        Some(p) => p,
        None => return cwd(),
    };
    std::path::absolute(&path).map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot resolve {}: {e}", path.display()))
    })
}
