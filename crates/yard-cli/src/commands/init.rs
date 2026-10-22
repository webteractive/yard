//! `yard init`, and `settle`: installing what a project's `.yard` needs, after asking.
//! `yard link` settles too, since linking a cloned project applies its file.

use super::{Ctx, Outcome, cwd, run_job};
use crate::client::CliError;
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use yard_proto::methods::{self, ExtAddParams, InitParams, PhpParams};
use yard_proto::{ErrorCode, InitResult, Need, SiteInfo, YardError};

pub async fn init(ctx: &Ctx, name: Option<String>, force: bool, yes: bool) -> Outcome {
    let params = InitParams { path: cwd()?.display().to_string(), name, force };
    let mut result: InitResult = ctx.client().await?.call(methods::SITES_INIT, params).await?;
    if !ctx.json {
        match result.written {
            true => println!("Wrote {}:", result.file),
            false => println!("{} is already there:", result.file),
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
pub async fn settle(ctx: &Ctx, site: SiteInfo, yes: bool) -> Result<SiteInfo, CliError> {
    if let Some(e) = &site.project_error {
        return Err(e.clone().into());
    }
    if site.needs.is_empty() {
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
    }
    let interactive = !ctx.json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let agreed = yes || (interactive && confirm("Install these? [y/N] "));
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
    let php: Vec<String> = site
        .needs
        .iter()
        .filter_map(|n| match n {
            Need::Php { php } => Some(php.clone()),
            Need::Extension { .. } => None,
        })
        .collect();
    for v in &php {
        let done = format!("Installed PHP {v}.\n");
        finished(run_job(ctx, methods::PHP_INSTALL, PhpParams { php: v.clone() }, || done).await)?;
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
            Need::Php { .. } => None,
        })
        .collect();
    for (name, php) in &exts {
        let done = format!("Added {name} to PHP {php}.\n");
        let params = ExtAddParams { name: name.clone(), php: Some(php.clone()) };
        finished(run_job(ctx, methods::EXT_ADD, params, || done).await)?;
    }
    if !exts.is_empty() {
        site = refetch(ctx, &site.name).await?;
    }
    if !ctx.json {
        println!("Serving {}", site.url);
    }
    Ok(site)
}

/// A job `run_job` stopped waiting for (Ctrl-C twice) ends the command with its code.
fn finished(outcome: Outcome) -> Result<(), CliError> {
    match outcome? {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

async fn refetch(ctx: &Ctx, name: &str) -> Result<SiteInfo, CliError> {
    let sites: Vec<SiteInfo> = ctx.client().await?.call(methods::SITES_LIST, Value::Null).await?;
    sites.into_iter().find(|s| s.name == name).ok_or_else(|| {
        YardError::new(ErrorCode::SiteNotFound, format!("no site named {name:?}"))
            .with_hint("yard sites lists them")
            .into()
    })
}

fn confirm(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}
