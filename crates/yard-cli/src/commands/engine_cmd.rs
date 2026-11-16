//! `yard <engine> [verb] …`: each engine's own command (recipes spec §9.1). The engine is any
//! the CLI knows, recipes included, read without yardd; a database verb is parsed exactly as
//! `yard db <verb>` is and runs limited to that engine's instances.

use super::{Ctx, Outcome, db_action};
use crate::cli::{Cli, Command};
use clap::Parser;
use serde_json::Value;
use std::ffi::OsString;
use yard_core::services::engines::{Verb, engine_verbs};
use yard_proto::{ServiceInfo, YardError, methods};

pub async fn run(ctx: &Ctx, args: Vec<OsString>) -> Outcome {
    let mut args = args.into_iter().map(|a| a.to_string_lossy().into_owned());
    let written = args.next().unwrap_or_default();
    // Laravel's name for the driver.
    let key = if written == "pgsql" { "postgres".to_string() } else { written.clone() };
    let engines = yard_daemon::engines::load_once(&ctx.paths, &Default::default());
    let Some(engine) = engines.get(&key).cloned() else {
        eprintln!("error: unrecognized subcommand '{written}'\n\nFor more information, try 'yard --help'.");
        return Ok(2);
    };
    let rest: Vec<String> = args.collect();
    let (verb, tail) = match rest.first().and_then(|w| Verb::parse(w)) {
        Some(verb) => (verb, rest[1..].to_vec()),
        None => (Verb::Shell, rest),
    };
    let verbs = engine_verbs(engine.as_ref());
    if !verbs.contains(&verb) {
        let has: Vec<&str> = verbs.iter().map(|v| v.name()).collect();
        let has = if has.is_empty() { "none".to_string() } else { has.join(", ") };
        eprintln!("error: {key} has no {}: it has {has}", verb.name());
        return Ok(2);
    }
    // `open` on an engine with a console opens the console.
    if verb == Verb::Open && engine.databases().is_none() && engine.console().is_some() {
        return open_console(ctx, &key).await;
    }
    let argv = ["yard".to_string(), "db".to_string(), verb.name().to_string()].into_iter().chain(tail);
    let parsed = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return Ok(e.exit_code());
        }
    };
    let Some(Command::Db { action }) = parsed.command else { return Ok(2) };
    let ctx = Ctx {
        paths: ctx.paths.clone(),
        json: ctx.json || parsed.json,
        root_overridden: ctx.root_overridden,
        security: ctx.security.clone(),
    };
    db_action(&ctx, action, Some(key)).await
}

/// Opens the console of `key`'s instance (`meilisearch.<tld>`).
async fn open_console(ctx: &Ctx, key: &str) -> Outcome {
    let services: Vec<ServiceInfo> = ctx.client().await?.call(methods::SERVICES_LIST, Value::Null).await?;
    let url = services.iter().find(|s| s.engine == key).and_then(|s| s.console.clone()).ok_or_else(|| {
        YardError::new(yard_proto::ErrorCode::ServiceNotFound, format!("no {key} instance serves a console"))
            .with_hint(format!("yard service add {key}"))
    })?;
    let opened = std::process::Command::new("/usr/bin/open").arg(&url).status().is_ok_and(|s| s.success());
    if !opened {
        return Err(YardError::internal(format!("cannot open {url}")).into());
    }
    ctx.emit(&url, || format!("Opened {url}.\n"));
    Ok(0)
}
