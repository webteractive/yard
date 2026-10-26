//! `node.*`: installed Node versions, the default, and a site's own spec (M4c spec §7).

use super::handlers::{Target, not_found, project_file_target, site, snapshot};
use crate::daemon::Context;
use crate::managed::edit_managed;
use crate::node::{fill_node, installed, lts};
use toml_edit::value;
use yard_core::config::{TomlFile, load_yard};
use yard_core::node::{DEFAULT_SPEC, NodeSpec, parse_spec, pick};
use yard_proto::methods::{NodeListParams, NodeSpecParams, NodeUseParams, SiteParams};
use yard_proto::{ErrorCode, NodeInfo, NodeList, SiteInfo, YardError};

/// A spec from a command, `INVALID_ARGUMENT` with `hint` when Yard refuses it.
fn spec_arg(spec: &str, hint: &str) -> Result<NodeSpec, YardError> {
    parse_spec(spec).map_err(|why| YardError::new(ErrorCode::InvalidArgument, why).with_hint(hint))
}

pub(super) async fn list(ctx: &Context, _p: NodeListParams) -> Result<NodeList, YardError> {
    let default = load_yard(&ctx.paths)?.0.default_node.unwrap_or(DEFAULT_SPEC);
    let mut sites = snapshot(ctx).await?.resolution.sites;
    fill_node(&mut sites, &ctx.paths);
    let installed = installed(&ctx.paths);
    let default_version = pick(&default, &installed, &lts(&ctx.paths));
    let versions = installed
        .iter()
        .rev()
        .map(|v| {
            let version = v.to_string();
            let sites = sites
                .iter()
                .filter(|s| s.node.as_ref().and_then(|n| n.version.as_deref()) == Some(version.as_str()))
                .map(|s| s.name.clone())
                .collect();
            NodeInfo { default: Some(*v) == default_version, version, sites }
        })
        .collect();
    Ok(NodeList { default: default.to_string(), versions, available: vec![] })
}

/// `yard node default`: only to a spec something installed meets, so nothing without a
/// pin stops running.
pub(super) async fn set_default(ctx: &Context, p: NodeSpecParams) -> Result<(), YardError> {
    let spec = spec_arg(&p.spec, "yard node default 22")?;
    if pick(&spec, &installed(&ctx.paths), &lts(&ctx.paths)).is_none() {
        return Err(YardError::new(ErrorCode::NodeMissing, format!("Node {spec} is not installed"))
            .with_hint(format!("yard node install {spec}")));
    }
    TomlFile::edit(&ctx.paths.yard_toml(), |file| {
        file.set(&["default_node"], value(spec.to_string()));
        Ok(())
    })
}

/// `yard node use`: the site's managed `.yard`, else `sites.toml`. The spec need not be
/// installed yet; the returned site's `node.version` says whether it is.
pub(super) async fn use_spec(ctx: &Context, p: NodeUseParams) -> Result<SiteInfo, YardError> {
    let spec = spec_arg(&p.spec, "yard node use 22")?;
    set_site_node(ctx, &p.site, Some(spec)).await
}

/// `yard node unuse`: the site follows its `.nvmrc`, `.node-version` or the default again.
pub(super) async fn unuse(ctx: &Context, p: SiteParams) -> Result<SiteInfo, YardError> {
    set_site_node(ctx, &p.site, None).await
}

async fn set_site_node(ctx: &Context, name: &str, spec: Option<NodeSpec>) -> Result<SiteInfo, YardError> {
    let change = match &spec {
        Some(spec) => format!("node = \"{spec}\""),
        None => "remove node".to_string(),
    };
    let set = |toml: &mut TomlFile, key: &[&str]| match &spec {
        Some(spec) => toml.set(key, value(spec.to_string())),
        None => {
            toml.remove(key);
        }
    };
    match project_file_target(ctx, name, &change)? {
        Target::Managed(file) => edit_managed(&file, |toml| {
            set(toml, &["node"]);
            Ok(())
        })?,
        Target::SitesToml => TomlFile::edit(&ctx.paths.sites_toml(), |toml| {
            let entry = toml.doc().get("sites").and_then(|s| s.get(name));
            if entry.is_none() {
                return Err(not_found(name));
            }
            if entry.and_then(|e| e.get("path")).is_none() {
                return Err(YardError::new(
                    ErrorCode::InvalidArgument,
                    format!("{name} is a proxy, so no Node runs it"),
                )
                .with_hint("yard sites shows which sites run PHP"));
            }
            set(toml, &["sites", name, "node"]);
            Ok(())
        })?,
    }
    site(ctx, name).await
}
