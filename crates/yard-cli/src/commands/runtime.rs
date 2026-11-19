//! `yard runtimes` and `yard runtime`: every runtime through one command (runtimes spec §12).
//! For php and node it is `yard php`, `yard isolate` and `yard node`, with their output;
//! Composer's are here.

use super::info::source_label;
use super::sites::{containing, set_in};
use super::{Ctx, Outcome, cwd, cwd_absolute, node, php, run_job, sites};
use crate::output::{for_agents, table};
use serde_json::Value;
use yard_proto::methods::{
    self, RuntimeInstallParams, RuntimeKeyParams, RuntimeSiteParams, RuntimeSpecParams, RuntimeUseParams,
    RuntimeVersionParams,
};
use yard_proto::{RuntimeDefaultSet, RuntimeList, RuntimesList, SiteInfo};

fn dash(list: &[String]) -> String {
    if list.is_empty() { "-".to_string() } else { list.join(", ") }
}

pub async fn list_all(ctx: &Ctx) -> Outcome {
    let list: RuntimesList = ctx.client().await?.call(methods::RUNTIMES_LIST, Value::Null).await?;
    ctx.emit(&list, || {
        let rows: Vec<Vec<String>> = list
            .runtimes
            .iter()
            .map(|r| {
                vec![
                    r.key.clone(),
                    r.file.clone().unwrap_or_else(|| r.origin.clone()),
                    r.shims.join(", "),
                    dash(&r.installed),
                    r.default.clone().unwrap_or_else(|| "none".into()),
                ]
            })
            .collect();
        let mut out = table(&["RUNTIME", "FROM", "SHIMS", "INSTALLED", "DEFAULT"], &rows);
        for e in &list.errors {
            out.push_str(&format!("error: {}\n", e.message));
        }
        out.push('\n');
        out.push_str(&for_agents("yard runtimes"));
        out
    });
    Ok(0)
}

pub async fn list(ctx: &Ctx, key: String) -> Outcome {
    let list: RuntimeList =
        ctx.client().await?.call(methods::RUNTIME_LIST, RuntimeKeyParams { key: key.clone() }).await?;
    ctx.emit(&list, || {
        let mut out = if list.versions.is_empty() {
            format!("No {key} installed. yard runtime install {key} installs the one this directory runs.\n")
        } else {
            let rows: Vec<Vec<String>> = list
                .versions
                .iter()
                .map(|v| vec![v.version.clone(), if v.default { "yes" } else { "" }.into(), dash(&v.sites)])
                .collect();
            table(&["VERSION", "DEFAULT", "SITES"], &rows)
        };
        out.push_str(&format!("default: {}\n\n", list.default.as_deref().unwrap_or("none")));
        out.push_str(&for_agents(&format!("yard runtime list {key}")));
        out
    });
    Ok(0)
}

/// With no spec, yardd installs what the current directory runs.
pub async fn install(ctx: &Ctx, key: String, spec: Option<String>) -> Outcome {
    match (key.as_str(), &spec) {
        ("php", Some(v)) => return php::install(ctx, v.clone()).await,
        ("node", _) => return node::install(ctx, spec).await,
        _ => {}
    }
    let (done, params) = match spec {
        Some(spec) => (
            format!("Installed {key} {spec}.\n"),
            RuntimeInstallParams { key: key.clone(), spec: Some(spec), path: None },
        ),
        None => (
            format!("Installed {key} for this directory.\n"),
            RuntimeInstallParams { key: key.clone(), spec: None, path: Some(cwd_absolute()?) },
        ),
    };
    run_job(ctx, methods::RUNTIME_INSTALL, params, || done).await
}

pub async fn uninstall(ctx: &Ctx, key: String, version: String) -> Outcome {
    match key.as_str() {
        "php" => return php::uninstall(ctx, version).await,
        "node" => return node::uninstall(ctx, version).await,
        _ => {}
    }
    let done = format!("Removed {key} {version}.\n");
    run_job(ctx, methods::RUNTIME_UNINSTALL, RuntimeVersionParams { key, version }, || done).await
}

pub async fn set_default(ctx: &Ctx, key: String, spec: String) -> Outcome {
    match key.as_str() {
        "php" => return php::set_default(ctx, spec).await,
        "node" => return node::set_default(ctx, spec).await,
        _ => {}
    }
    let params = RuntimeSpecParams { key: key.clone(), spec: spec.clone() };
    // A yardd before protocol 2.15 answers null.
    let set: Option<RuntimeDefaultSet> = ctx.client().await?.call(methods::RUNTIME_DEFAULT, params).await?;
    let set = set.unwrap_or_default();
    ctx.emit(&set, || {
        let mut out = format!("{key} {spec} is now the default.\n");
        if !set.unfit.is_empty() {
            out.push_str(&format!("These sites' PHP cannot run {key} {spec}:\n"));
            for u in &set.unfit {
                out.push_str(&format!("  {}: {}\n", u.site, u.hint));
            }
        }
        out
    });
    Ok(0)
}

/// `yard runtime use <key> <spec>` (`spec` set) and `yard runtime unuse <key>` (`None`).
pub async fn pin(ctx: &Ctx, key: String, spec: Option<String>, site: Option<String>) -> Outcome {
    match key.as_str() {
        "php" => return sites::isolate(ctx, spec, site).await,
        "node" => return node::pin(ctx, spec, site).await,
        _ => {}
    }
    let mut client = ctx.client().await?;
    let site = match site {
        Some(s) => s,
        None => {
            let list: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
            let hint = match &spec {
                Some(s) => format!("yard runtime use {key} {s} --site <name>"),
                None => format!("yard runtime unuse {key} --site <name>"),
            };
            containing(&list, &cwd()?, &hint)?
        }
    };
    let updated: SiteInfo = match &spec {
        Some(spec) => {
            let params = RuntimeUseParams { key: key.clone(), site, spec: spec.clone() };
            client.call(methods::RUNTIME_USE, params).await?
        }
        None => client.call(methods::RUNTIME_UNUSE, RuntimeSiteParams { key: key.clone(), site }).await?,
    };
    ctx.emit(&updated, || {
        let name = &updated.name;
        let rt = updated.runtimes.iter().find(|r| r.key == key);
        let display = rt.map_or(key.as_str(), |r| r.name.as_str());
        match (&spec, rt.and_then(|r| r.spec.as_ref())) {
            (Some(spec), _) => {
                let mut out = format!("{name} now runs {display} {spec}{}\n", set_in(&updated));
                if rt.is_some_and(|r| r.version.is_none() && r.error.is_none()) {
                    out.push_str(&format!(
                        "{display} {spec} is not installed: yard runtime install {key} {spec}\n"
                    ));
                }
                out
            }
            (None, Some(now)) => {
                let source = source_label(rt.map(|r| &r.source));
                format!("{name} runs {display} {now} again ({source})\n")
            }
            (None, None) => format!("{name} has no {key} pin of its own now.\n"),
        }
    });
    Ok(0)
}
