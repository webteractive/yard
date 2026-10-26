//! `yard sites`, `link`/`unlink`, `proxy`/`unproxy`, `docroot`, `isolate` and
//! `secure`/`unsecure`.

use super::info::missing_node;
use super::{Ctx, Outcome, cwd, init};
use crate::output::{for_agents, table};
use serde_json::Value;
use std::path::Path;
use yard_core::sites::host_label;
use yard_proto::methods::{
    self, DocrootParams, IsolateParams, LinkParams, NameParams, ProxyParams, SiteParams,
};
use yard_proto::{
    CaInfo, ErrorCode, IgnoredBecause, ProjectFileKind, SiteInfo, SiteKind, SiteSource, YardError,
};

pub async fn list(ctx: &Ctx) -> Outcome {
    let sites: Vec<SiteInfo> = ctx.client().await?.call(methods::SITES_LIST, Value::Null).await?;
    ctx.emit(&sites, || {
        let mut out = if sites.is_empty() {
            "No sites yet. Serve a project with yard link, run inside it.\n".to_string()
        } else {
            let rows: Vec<Vec<String>> = sites.iter().map(row).collect();
            table(&["SITE", "URL", "SOURCE", "TARGET", "PHP", "STATUS"], &rows)
        };
        out.push('\n');
        out.push_str(&for_agents("yard sites"));
        out
    });
    Ok(0)
}

fn row(site: &SiteInfo) -> Vec<String> {
    let source = match site.source {
        SiteSource::Linked => "linked",
        SiteSource::Proxy => "proxy",
    };
    let (target, php) = match &site.kind {
        // The directory nginx serves, so a `public/` that appears later shows up here.
        SiteKind::Php { docroot, php, .. } => (docroot.clone(), php.clone()),
        SiteKind::Proxy { target } => (target.clone(), "-".into()),
    };
    let status = if site.needs_setup() { "needs setup" } else { "serving" };
    vec![site.name.clone(), site.url.clone(), source.into(), target, php, status.into()]
}

/// A project with a `.yard` may need PHP or extensions installed first, and any project
/// may pin a Node that is not installed: `settle` asks.
pub async fn link(ctx: &Ctx, name: Option<String>, yes: bool) -> Outcome {
    let dir = cwd()?;
    let name = name.map_or_else(|| name_of(&dir), Ok)?;
    let params = LinkParams { name, path: dir.display().to_string() };
    let site: SiteInfo = ctx.client().await?.call(methods::SITES_LINK, params).await?;
    let note = managed_note(&site);
    if !site.needs_setup() && missing_node(site.node.as_ref()).is_none() {
        ctx.emit(&site, || format!("Linked {} → {}\n{note}", dir.display(), site.url));
        return Ok(0);
    }
    if !ctx.json {
        print!("Linked {}.\n{note}", dir.display());
    }
    let site = init::settle(ctx, site, yes).await?;
    if ctx.json {
        ctx.emit(&site, String::new);
    }
    Ok(0)
}

/// The managed `.yard` a link serves from, or one for another directory it leaves alone
/// (managed-file spec §5); empty otherwise.
fn managed_note(site: &SiteInfo) -> String {
    match (&site.project_file_kind, &site.project_file, &site.ignored_managed_file) {
        (Some(ProjectFileKind::Managed), Some(file), _) => format!("Using {file}\n"),
        (_, _, Some(i)) if i.because == IgnoredBecause::OtherProject => {
            let project = i.project.as_deref().unwrap_or("no project");
            format!("{} is for {project}, so it is not used here.\n", i.file)
        }
        _ => String::new(),
    }
}

/// ` (set in <file>)` for a site whose managed `.yard` the command edited, else nothing.
pub(super) fn set_in(site: &SiteInfo) -> String {
    match (&site.project_file_kind, &site.project_file) {
        (Some(ProjectFileKind::Managed), Some(file)) => format!(" (set in {file})"),
        _ => String::new(),
    }
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
            containing(&sites, &cwd()?, &format!("yard docroot {path} --site <name>"))?
        }
    };
    let params = DocrootParams { site, path: Some(path) };
    let updated: SiteInfo = client.call(methods::SITES_DOCROOT, params).await?;
    ctx.emit(&updated, || match &updated.kind {
        SiteKind::Php { docroot, .. } => {
            format!("{} now serves {docroot}{}\n", updated.name, set_in(&updated))
        }
        SiteKind::Proxy { target } => format!("{} proxies to {target}\n", updated.name),
    });
    Ok(0)
}

/// The PHP site whose directory holds `dir`, the innermost when sites nest. `hint` is
/// the command with `--site` for when there is none.
pub(super) fn containing(sites: &[SiteInfo], dir: &Path, hint: &str) -> Result<String, YardError> {
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
                .with_hint(hint)
        })
}

/// A site name from a directory name: lowercased, spaces and `_` as `-`.
fn name_of(dir: &Path) -> Result<String, YardError> {
    let base = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    host_label(&base).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, format!("{base:?} does not make a site name"))
            .with_hint("pass one: yard link <name>")
    })
}

/// `yard isolate <ver>` (`php` set) and `yard unisolate` (`None`).
pub async fn isolate(ctx: &Ctx, php: Option<String>, site: Option<String>) -> Outcome {
    let mut client = ctx.client().await?;
    let site = match site {
        Some(s) => s,
        None => {
            let sites: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
            let hint = match &php {
                Some(v) => format!("yard isolate {v} --site <name>"),
                None => "yard unisolate --site <name>".to_string(),
            };
            containing(&sites, &cwd()?, &hint)?
        }
    };
    let updated: SiteInfo = client.call(methods::SITES_ISOLATE, IsolateParams { site, php }).await?;
    ctx.emit(&updated, || match &updated.kind {
        SiteKind::Php { php, .. } => format!("{} now runs PHP {php}{}\n", updated.name, set_in(&updated)),
        SiteKind::Proxy { target } => format!("{} proxies to {target}\n", updated.name),
    });
    Ok(0)
}

pub async fn secure(ctx: &Ctx, site: Option<String>, on: bool) -> Outcome {
    let mut client = ctx.client().await?;
    let verb = if on { "secure" } else { "unsecure" };
    let site = match site {
        Some(s) => s,
        None => {
            let sites: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
            containing(&sites, &cwd()?, &format!("yard {verb} <site>"))?
        }
    };
    let method = if on { methods::SITES_SECURE } else { methods::SITES_UNSECURE };
    let updated: SiteInfo = client.call(method, SiteParams { site }).await?;
    if ctx.json {
        ctx.emit(&updated, String::new);
        return Ok(0);
    }
    if !on {
        println!("{} is back on {}{}", updated.name, updated.url, set_in(&updated));
        return Ok(0);
    }
    println!("Secured {} → {}{}", updated.name, updated.url, set_in(&updated));
    let ca: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    if !ca.trusted {
        println!(
            "The Yard CA is not trusted yet, so browsers will warn. Run yard trust (macOS asks for your password once)."
        );
    }
    Ok(0)
}
