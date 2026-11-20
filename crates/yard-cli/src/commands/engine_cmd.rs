//! `yard <engine> [verb] …`: each engine's own command (recipes spec §9.1). The engine is any
//! the CLI knows, recipes included, read without yardd; a database verb is parsed by
//! `EngineArgs` and runs limited to that engine's instances.

use super::{Ctx, Outcome, db_action};
use crate::cli::EngineArgs;
use clap::{CommandFactory, FromArgMatches};
use serde_json::Value;
use std::ffi::OsString;
use yard_core::services::engines::{Verb, engine_verbs};
use yard_proto::{ServiceInfo, YardError, methods};

pub async fn run(ctx: &Ctx, args: Vec<OsString>) -> Outcome {
    let mut args = args.into_iter().map(|a| a.to_string_lossy().into_owned());
    let written = args.next().unwrap_or_default();
    // Laravel's name for the driver.
    let key = if written == "pgsql" { "postgres".to_string() } else { written.clone() };
    let rest: Vec<String> = args.collect();
    // A `--root` written after the engine is the root, as it is anywhere else: the engines are
    // read from it and every call goes to its yardd, never the real one.
    let root_overridden = ctx.root_overridden || trailing_root(&rest).is_some();
    let paths = trailing_root(&rest).map_or_else(|| ctx.paths.clone(), yard_core::Paths::new);
    let engines = yard_daemon::engines::load_once(&paths, &Default::default());
    let Some(engine) = engines.get(&key).cloned() else {
        if key == "db" {
            eprintln!(
                "error: yard db is gone: use yard <engine> <verb> (yard mysql list, yard postgres dump …)"
            );
            return Ok(2);
        }
        eprintln!("error: unrecognized subcommand '{written}'\n\nFor more information, try 'yard --help'.");
        return Ok(2);
    };
    let (verb, tail) = split_verb(rest);
    let verbs = engine_verbs(engine.as_ref());
    if !verbs.contains(&verb) {
        let has: Vec<&str> = verbs.iter().map(|v| v.name()).collect();
        let has = if has.is_empty() { "none".to_string() } else { has.join(", ") };
        eprintln!("error: {key} has no {}: it has {has}", verb.name());
        return Ok(2);
    }
    // `open` on an engine with a console opens the console.
    if verb == Verb::Open && engine.databases().is_none() && engine.console().is_some() {
        let ctx = Ctx { paths, json: ctx.json, root_overridden, security: ctx.security.clone() };
        return open_console(&ctx, &key).await;
    }
    // Named after what was typed, so help and usage errors say `yard mysql list`.
    let name = format!("yard {written}");
    let mut cmd = EngineArgs::command().bin_name(name.clone());
    let argv = [name, verb.name().to_string()].into_iter().chain(tail);
    let parsed = cmd
        .try_get_matches_from_mut(argv)
        .and_then(|m| EngineArgs::from_arg_matches(&m))
        .map(|a| (a.action, a.json));
    let (action, json) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = e.print();
            return Ok(e.exit_code());
        }
    };
    let ctx = Ctx { paths, json: ctx.json || json, root_overridden, security: ctx.security.clone() };
    db_action(&ctx, action, key).await
}

/// The flags of the database verbs that take a value: the word after one is never the verb.
const VALUE_FLAGS: [&str; 3] = ["--service", "--site", "--root"];

/// The `--root` among `args` before any `--`, in either spelling.
fn trailing_root(args: &[String]) -> Option<String> {
    let mut words = args.iter().take_while(|a| *a != "--");
    while let Some(word) = words.next() {
        if word == "--root" {
            return words.next().cloned();
        }
        if let Some(root) = word.strip_prefix("--root=") {
            return Some(root.to_string());
        }
    }
    None
}

/// The verb and the other arguments: the first word before any `--` that names a verb and is not
/// a flag's value (`yard mysql --service mysql-8.0 list`), else `shell` with them all.
fn split_verb(mut args: Vec<String>) -> (Verb, Vec<String>) {
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        if VALUE_FLAGS.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if let Some(verb) = Verb::parse(&args[i]) {
            args.remove(i);
            return (verb, args);
        }
        if !args[i].starts_with('-') {
            // A positional before any verb (a database name): the verb is shell.
            break;
        }
        i += 1;
    }
    (Verb::Shell, args)
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
