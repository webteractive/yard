//! `yard init`, and `settle`: installing what a project's `.yard` needs, after asking.
//! `yard link` settles too, since linking a cloned project applies its file.

use super::info::missing_node;
use super::{Ctx, Outcome, confirm, cwd, finished, interactive, run_job, services, sources};
use crate::client::CliError;
use serde_json::{Value, json};
use yard_proto::methods::{self, ExtAddParams, InitParams, NodeInstallParams, PhpParams};
use yard_proto::{ErrorCode, InitResult, Need, SiteInfo, YardError};

/// A `major.minor` protocol version.
fn version(protocol: &str) -> Option<(u32, u32)> {
    let (major, minor) = protocol.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// `yard init`'s flags.
pub struct InitArgs {
    pub name: Option<String>,
    pub force: bool,
    pub replace: bool,
    pub yes: bool,
    pub local: bool,
}

pub async fn init(ctx: &Ctx, args: InitArgs) -> Outcome {
    let InitArgs { name, force, replace, yes, local } = args;
    let mut params = InitParams { path: cwd()?.display().to_string(), name, force, replace, local };
    let mut client = ctx.client().await?;
    // A yardd before 2.6 knows no managed .yard and would write this one into the project.
    let protocol = &client.hello.protocol;
    if !local && version(protocol) < Some((2, 6)) {
        return Err(YardError::new(
            ErrorCode::ProtocolMismatch,
            format!("yardd speaks protocol {protocol}, so yard init would write .yard into the project"),
        )
        .with_hint("yard stop && yard start")
        .into());
    }
    let mut result: InitResult = match client.call(methods::SITES_INIT, &params).await {
        // The name serves another directory and this one has its own .yard, or its managed
        // .yard was written for another directory: ask first.
        Err(e) if e.code == ErrorCode::SiteNameTaken && interactive(ctx) => {
            println!("{}.", e.message);
            if !confirm("Replace it with this directory? [y/N] ") {
                return Err(e.into());
            }
            params.replace = true;
            client.call(methods::SITES_INIT, &params).await?
        }
        other => other?,
    };
    if !ctx.json {
        if let Some(from) = &result.moved_from {
            println!("Moved {} here from {from}.", result.site.name);
        }
        if let Some(backup) = &result.backup {
            println!("Kept the old file as {backup}.");
        }
        match (&result.from_managed, result.written) {
            (Some(from), _) => println!("Moved {from} to {}:", result.file),
            (None, true) => println!("Wrote {}:", result.file),
            (None, false) => println!("{} is already there:", result.file),
        }
        for line in result.text.lines() {
            println!("  {line}");
        }
    }
    result.site = settle(ctx, result.site, yes).await?;
    if ctx.json {
        ctx.emit(&result, String::new);
    }
    Ok(0)
}

/// Installs what `site` needs once the user agrees (or with `yes`), and returns the site
/// as it is afterwards. Without a terminal to ask on, and without `yes`, it is
/// `NEEDS_INSTALL` and nothing is installed. A `.yard` that does not parse is its error.
///
/// A Node nothing installed matches is offered too, and installed last, but it never
/// stops the site serving: alone, declining it leaves a hint and serves (M4c spec §8).
pub async fn settle(ctx: &Ctx, site: SiteInfo, yes: bool) -> Result<SiteInfo, CliError> {
    if let Some(e) = &site.project_error {
        return Err(e.clone().into());
    }
    let node = missing_node(site.node.as_ref()).map(str::to_string);
    if site.needs.is_empty() && node.is_none() {
        if !ctx.json {
            println!("Serving {}", site.url);
        }
        return Ok(site);
    }
    if !ctx.json {
        println!("{} needs:", site.name);
        for need in &site.needs {
            println!("  {}  ({})", need.describe(), need.command());
        }
        if let Some(spec) = &node {
            println!("  Node {spec}  (yard node install {spec})");
        }
    }
    let agreed = yes || (interactive(ctx) && confirm("Install these? [y/N] "));
    if !agreed && site.needs.is_empty() {
        let spec = node.as_deref().unwrap_or_default();
        eprintln!("{} runs Node {spec}, which is not installed: yard node install {spec}", site.name);
        if !ctx.json {
            println!("Serving {}", site.url);
        }
        return Ok(site);
    }
    if !agreed {
        return Err(YardError::new(
            ErrorCode::NeedsInstall,
            format!("{} needs {}", site.name, Need::describe_all(&site.needs)),
        )
        .with_hint("yard init --yes")
        .with_data(json!({ "needs": site.needs }))
        .into());
    }

    let mut site = site;
    // PHP first, then extensions (they need a PHP), then services. A formula brew does not
    // trust is the install's error: `--yes` skips the question above, never the trust one.
    let php: Vec<String> = site
        .needs
        .iter()
        .filter_map(|n| match n {
            Need::Php { php } => Some(php.clone()),
            Need::Extension { .. } | Need::Service { .. } => None,
        })
        .collect();
    for v in &php {
        let done = format!("Installed PHP {v}.\n");
        let params = PhpParams { php: v.clone() };
        let install = || run_job(ctx, methods::PHP_INSTALL, params.clone(), || done.clone());
        finished(sources::with_trust(ctx, install).await)?;
    }
    if !php.is_empty() {
        // The new PHP may have some of the extensions compiled in.
        site = refetch(ctx, &site.name).await?;
    }
    let exts: Vec<(String, String)> = site
        .needs
        .iter()
        .filter_map(|n| match n {
            Need::Extension { name, php } => Some((name.clone(), php.clone())),
            Need::Php { .. } | Need::Service { .. } => None,
        })
        .collect();
    for (name, php) in &exts {
        let done = format!("Added {name} to PHP {php}.\n");
        let params = ExtAddParams { name: name.clone(), php: Some(php.clone()) };
        let add = || run_job(ctx, methods::EXT_ADD, params.clone(), || done.clone());
        finished(sources::with_trust(ctx, add).await)?;
    }
    if !exts.is_empty() {
        site = refetch(ctx, &site.name).await?;
    }
    let wanted: Vec<(String, Option<String>)> = site
        .needs
        .iter()
        .filter_map(|n| match n {
            Need::Service { engine, version } => Some((engine.clone(), version.clone())),
            Need::Php { .. } | Need::Extension { .. } => None,
        })
        .collect();
    for (engine, version) in &wanted {
        services::add_instance(ctx, engine, version.as_deref(), None).await?;
    }
    if !wanted.is_empty() {
        site = refetch(ctx, &site.name).await?;
    }
    // Node last: the site already serves without it. The spec is the one first offered.
    if let Some(spec) = &node {
        let done = format!("Installed Node {spec}.\n");
        let params = NodeInstallParams { spec: Some(spec.clone()), path: None };
        finished(run_job(ctx, methods::NODE_INSTALL, params, || done).await)?;
        site = refetch(ctx, &site.name).await?;
    }
    if let Some(e) = &site.project_error {
        return Err(e.clone().into());
    }
    // Installed, yet still missing: a module PHP lists under another name, say.
    if !site.needs.is_empty() {
        return Err(YardError::new(
            ErrorCode::NeedsInstall,
            format!("{} still needs {}", site.name, Need::describe_all(&site.needs)),
        )
        .with_hint("yard doctor")
        .with_data(json!({ "needs": site.needs }))
        .into());
    }
    if !ctx.json {
        println!("Serving {}", site.url);
    }
    Ok(site)
}

async fn refetch(ctx: &Ctx, name: &str) -> Result<SiteInfo, CliError> {
    let sites: Vec<SiteInfo> = ctx.client().await?.call(methods::SITES_LIST, Value::Null).await?;
    sites.into_iter().find(|s| s.name == name).ok_or_else(|| {
        YardError::new(ErrorCode::SiteNotFound, format!("no site named {name:?}"))
            .with_hint("yard sites lists them")
            .into()
    })
}
