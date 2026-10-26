//! `yard node`: list, install, uninstall, default, use and unuse (M4c spec §7).

use super::info::source_label;
use super::sites::{containing, set_in};
use super::{Ctx, Outcome, cwd, cwd_absolute, run_job};
use crate::output::{for_agents, table};
use serde_json::Value;
use yard_core::node::{install_hint, shell_word};
use yard_proto::methods::{
    self, NodeInstallParams, NodeListParams, NodeSpecParams, NodeUseParams, NodeVersionParams, SiteParams,
};
use yard_proto::{NodeList, SiteInfo};

/// `available` is `--available`: `Some(None)` for every line, `Some(Some(major))` for one.
pub async fn list(ctx: &Ctx, available: Option<Option<u32>>) -> Outcome {
    let params = NodeListParams { available: available.is_some(), major: available.flatten() };
    let list: NodeList = ctx.client().await?.call(methods::NODE_LIST, params).await?;
    ctx.emit(&list, || {
        let mut out = if list.versions.is_empty() {
            format!("No Node installed. {} installs the default.\n", install_hint(&list.default))
        } else {
            let rows: Vec<Vec<String>> = list
                .versions
                .iter()
                .map(|n| {
                    let sites = if n.sites.is_empty() { "-".to_string() } else { n.sites.join(", ") };
                    vec![n.version.clone(), if n.default { "yes" } else { "" }.into(), sites]
                })
                .collect();
            table(&["VERSION", "DEFAULT", "SITES"], &rows)
        };
        if !list.available.is_empty() {
            let rows: Vec<Vec<String>> = list
                .available
                .iter()
                .map(|r| vec![r.version.clone(), r.lts.clone().unwrap_or_else(|| "-".into())])
                .collect();
            out.push('\n');
            out.push_str(&table(&["VERSION", "LTS"], &rows));
        }
        out.push('\n');
        out.push_str(&for_agents("yard node list"));
        out
    });
    Ok(0)
}

/// With no spec, yardd installs what the current directory asks for.
pub async fn install(ctx: &Ctx, spec: Option<String>) -> Outcome {
    let (done, params) = match spec {
        Some(spec) => {
            (format!("Installed Node {spec}.\n"), NodeInstallParams { spec: Some(spec), path: None })
        }
        None => (
            "Installed Node for this directory.\n".to_string(),
            NodeInstallParams { spec: None, path: Some(cwd_absolute()?) },
        ),
    };
    run_job(ctx, methods::NODE_INSTALL, params, || done).await
}

pub async fn uninstall(ctx: &Ctx, version: String) -> Outcome {
    let done = format!("Removed Node {version}.\n");
    run_job(ctx, methods::NODE_UNINSTALL, NodeVersionParams { version }, || done).await
}

pub async fn set_default(ctx: &Ctx, spec: String) -> Outcome {
    ctx.client().await?.call::<_, ()>(methods::NODE_DEFAULT, NodeSpecParams { spec: spec.clone() }).await?;
    ctx.emit(&(), || format!("Node {spec} is now the default.\n"));
    Ok(0)
}

/// `yard node use <spec>` (`spec` set) and `yard node unuse` (`None`).
pub async fn pin(ctx: &Ctx, spec: Option<String>, site: Option<String>) -> Outcome {
    let mut client = ctx.client().await?;
    let site = match site {
        Some(s) => s,
        None => {
            let sites: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
            let hint = match &spec {
                Some(s) => format!("yard node use {} --site <name>", shell_word(s)),
                None => "yard node unuse --site <name>".to_string(),
            };
            containing(&sites, &cwd()?, &hint)?
        }
    };
    let updated: SiteInfo = match &spec {
        Some(spec) => client.call(methods::NODE_USE, NodeUseParams { site, spec: spec.clone() }).await?,
        None => client.call(methods::NODE_UNUSE, SiteParams { site }).await?,
    };
    ctx.emit(&updated, || {
        let name = &updated.name;
        let node = updated.node.as_ref();
        match (&spec, node.and_then(|n| n.spec.as_ref())) {
            (Some(spec), _) => {
                let mut out = format!("{name} now runs Node {spec}{}\n", set_in(&updated));
                if node.is_some_and(|n| n.version.is_none()) {
                    out.push_str(&format!("Node {spec} is not installed: {}\n", install_hint(spec)));
                }
                out
            }
            (None, Some(now)) => {
                let source = source_label(node.map(|n| &n.source));
                format!("{name} runs Node {now} again ({source})\n")
            }
            (None, None) => format!("{name} has no Node pin of its own now.\n"),
        }
    });
    Ok(0)
}
