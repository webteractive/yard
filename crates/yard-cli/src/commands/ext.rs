//! `yard ext` and `yard xdebug`: PHP extensions.

use super::{Ctx, Outcome, cwd, run_job};
use crate::output::{for_agents, table};
use std::path::Path;
use yard_proto::ExtInfo;
use yard_proto::methods::{self, ExtAddParams, ExtListParams, ExtParams, XdebugParams};

fn shown(php: &Option<String>) -> String {
    php.as_ref().map_or_else(|| "the default PHP".to_string(), |v| format!("PHP {v}"))
}

pub async fn list(ctx: &Ctx, php: Option<String>) -> Outcome {
    let exts: Vec<ExtInfo> = ctx.client().await?.call(methods::EXT_LIST, ExtListParams { php }).await?;
    ctx.emit(&exts, || {
        if exts.is_empty() {
            return "No extensions yet. Add one with yard ext add <name>.\n".to_string();
        }
        let rows: Vec<Vec<String>> = exts
            .iter()
            .map(|e| vec![e.name.clone(), e.source.clone(), if e.enabled { "yes" } else { "no" }.into()])
            .collect();
        let mut out = table(&["NAME", "SOURCE", "ENABLED"], &rows);
        out.push('\n');
        out.push_str(&for_agents("yard ext list"));
        out
    });
    Ok(0)
}

pub async fn toggle(ctx: &Ctx, name: String, php: Option<String>, on: bool) -> Outcome {
    let done = format!("{} {name} for {}.\n", if on { "Enabled" } else { "Disabled" }, shown(&php));
    let method = if on { methods::EXT_ENABLE } else { methods::EXT_DISABLE };
    ctx.client().await?.call::<_, ()>(method, ExtParams { name, php }).await?;
    ctx.emit(&(), || done);
    Ok(0)
}

pub async fn remove(ctx: &Ctx, name: String, php: Option<String>) -> Outcome {
    let done = format!("Removed {name} from {}.\n", shown(&php));
    run_job(ctx, methods::EXT_REMOVE, ExtParams { name, php }, || done).await
}

pub async fn xdebug(ctx: &Ctx, on: bool, php: Option<String>) -> Outcome {
    let done = format!("Xdebug is {} for {}.\n", if on { "on" } else { "off" }, shown(&php));
    ctx.client().await?.call::<_, ()>(methods::XDEBUG_SET, XdebugParams { on, php }).await?;
    ctx.emit(&(), || done);
    Ok(0)
}

pub async fn add(ctx: &Ctx, name: String, php: Option<String>) -> Outcome {
    // yardd runs elsewhere, so a .so path is sent absolute.
    let name = if name.ends_with(".so") && !Path::new(&name).is_absolute() {
        cwd()?.join(&name).display().to_string()
    } else {
        name
    };
    let stem = Path::new(&name).file_stem().map_or(name.clone(), |s| s.to_string_lossy().into_owned());
    let module = stem.strip_prefix("php_").unwrap_or(&stem);
    let done = format!("Added {module} to {}.\n", shown(&php));
    run_job(ctx, methods::EXT_ADD, ExtAddParams { name, php }, || done).await
}
