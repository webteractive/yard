//! `yard php`: list, install, uninstall and default.

use super::{Ctx, Outcome, run_job};
use crate::cli::ConfigAction;
use crate::output::{for_agents, table};
use serde_json::Value;
use yard_proto::PhpInfo;
use yard_proto::methods::{self, PhpConfigParams, PhpParams};

pub async fn list(ctx: &Ctx) -> Outcome {
    let versions: Vec<PhpInfo> = ctx.client().await?.call(methods::PHP_LIST, Value::Null).await?;
    ctx.emit(&versions, || {
        let rows: Vec<Vec<String>> = versions.iter().map(row).collect();
        let mut out = table(&["VERSION", "DEFAULT", "SITES", "POOL"], &rows);
        out.push('\n');
        out.push_str(&for_agents("yard php list"));
        out
    });
    Ok(0)
}

fn row(php: &PhpInfo) -> Vec<String> {
    let sites = if php.sites.is_empty() { "-".to_string() } else { php.sites.join(", ") };
    let pool = match (&php.pool, php.installed) {
        (_, false) => format!("not installed → yard php install {}", php.version),
        (Some(state), true) => state.clone(),
        (None, true) => "-".into(),
    };
    vec![php.version.clone(), if php.default { "yes" } else { "" }.into(), sites, pool]
}

pub async fn install(ctx: &Ctx, version: String) -> Outcome {
    let done = format!("Installed PHP {version}.\n");
    run_job(ctx, methods::PHP_INSTALL, PhpParams { php: version }, || done).await
}

pub async fn uninstall(ctx: &Ctx, version: String) -> Outcome {
    let done = format!("Removed PHP {version}.\n");
    run_job(ctx, methods::PHP_UNINSTALL, PhpParams { php: version }, || done).await
}

pub async fn set_default(ctx: &Ctx, version: String) -> Outcome {
    let params = PhpParams { php: version.clone() };
    ctx.client().await?.call::<_, ()>(methods::PHP_DEFAULT, params).await?;
    ctx.emit(&(), || format!("PHP {version} is now the default.\n"));
    Ok(0)
}

pub async fn config(ctx: &Ctx, action: ConfigAction) -> Outcome {
    let mut client = ctx.client().await?;
    match action {
        ConfigAction::Get { key, php } => {
            let params = PhpConfigParams { php, key: key.clone(), value: None };
            let value: Option<String> = client.call(methods::PHP_CONFIG_GET, params).await?;
            ctx.emit(&value, || match &value {
                Some(v) => format!("{v}\n"),
                None => format!("{key} is not overridden; php.ini's default applies.\n"),
            });
        }
        ConfigAction::Set { key, value, php } => {
            let shown = php.clone().map_or_else(|| "the default PHP".to_string(), |v| format!("PHP {v}"));
            let params = PhpConfigParams { php, key: key.clone(), value: Some(value.clone()) };
            client.call::<_, ()>(methods::PHP_CONFIG_SET, params).await?;
            ctx.emit(&(), || format!("{key} = {value} for {shown}.\n"));
        }
        ConfigAction::Unset { key, php } => {
            let params = PhpConfigParams { php, key: key.clone(), value: None };
            let removed: bool = client.call(methods::PHP_CONFIG_UNSET, params).await?;
            ctx.emit(&removed, || match removed {
                true => format!("Removed the {key} override.\n"),
                false => format!("{key} was not overridden.\n"),
            });
        }
    }
    Ok(0)
}
