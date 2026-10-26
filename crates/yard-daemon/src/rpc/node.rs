//! `node.*`: installed Node versions, the default, and a site's own spec (M4c spec §7).

use super::handlers::{Target, directory, not_found, project_file_target, site, snapshot};
use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::managed::edit_managed;
use crate::node::{available, fill_node, fnm_env, in_use, installed, lts, resolve_at, write_lts};
use toml_edit::value;
use yard_core::brew::Brew;
use yard_core::config::{TomlFile, load_sites, load_yard};
use yard_core::node::{
    DEFAULT_SPEC, NodeSpec, NodeVersion, install_hint, lts_from, newest, parse_ls_remote, parse_spec, pick,
};
use yard_proto::methods::{
    NodeInstallParams, NodeListParams, NodeSpecParams, NodeUseParams, NodeVersionParams, SiteParams,
};
use yard_proto::{ErrorCode, JobStarted, NodeInfo, NodeList, SiteInfo, YardError};

/// A spec from a command, `INVALID_ARGUMENT` with `hint` when Yard refuses it.
fn spec_arg(spec: &str, hint: &str) -> Result<NodeSpec, YardError> {
    parse_spec(spec).map_err(|why| YardError::new(ErrorCode::InvalidArgument, why).with_hint(hint))
}

pub(super) async fn list(ctx: &Context, p: NodeListParams) -> Result<NodeList, YardError> {
    let available = match p.available {
        true => available(&ctx.brew, &ctx.paths, p.major).await?,
        false => vec![],
    };
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
    Ok(NodeList { default: default.to_string(), versions, available })
}

/// `yard node install`: the spec given, or the one `path` resolves to. The job resolves it
/// against nodejs.org's list itself and hands fnm an exact `v<x.y.z>`, so a version already
/// there is said to be, and an exact pin installs even while the list cannot be fetched.
pub(super) async fn install(ctx: &Context, p: NodeInstallParams) -> Result<JobStarted, YardError> {
    let spec = match (&p.spec, &p.path) {
        (Some(spec), _) => spec_arg(spec, "yard node install 22")?,
        (None, Some(path)) => {
            let dir = directory(path)?;
            let (yard, _) = load_yard(&ctx.paths)?;
            resolve_at(&dir, &yard, &load_sites(&ctx.paths)?, &ctx.paths)?.0
        }
        (None, None) => {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                "name a Node version, or a directory whose pin to install",
            )
            .with_hint("yard node install 22"));
        }
    };
    let (paths, brew) = (ctx.paths.clone(), ctx.brew.clone());
    Ok(ctx.jobs.start("node-install", move |job: JobCtx| async move {
        if let NodeSpec::Exact(v) = spec
            && installed(&paths).contains(&v)
        {
            job.line(format!("Node {v} is already installed"));
            return Ok(());
        }
        let fnm = brew.fnm_bin();
        if !fnm.exists() {
            job.run(&brew.brew_bin(), &["install", "fnm"], &Brew::job_env()).await?;
            if !fnm.exists() {
                return Err(YardError::new(
                    ErrorCode::FormulaMissing,
                    format!("brew install fnm left no {}", fnm.display()),
                )
                .with_hint("brew install fnm"));
            }
        }
        std::fs::create_dir_all(paths.node_dir())
            .map_err(|e| YardError::internal(format!("cannot create {}: {e}", paths.node_dir().display())))?;
        let env = fnm_env(&paths);
        let (listed, lines) = job.capture(&fnm, &["ls-remote"], &env).await?;
        let target = if listed {
            let releases = parse_ls_remote(&lines.join("\n"));
            let lts = lts_from(&releases);
            if let Err(e) = write_lts(&paths, &lts) {
                job.line(format!("cannot write {}: {e}", paths.lts_toml().display()));
            }
            newest(&spec, &releases, &lts).ok_or_else(|| {
                YardError::new(ErrorCode::InvalidArgument, format!("nodejs.org has no Node {spec}"))
                    .with_hint("yard node list --available")
            })?
        } else if let NodeSpec::Exact(v) = spec {
            job.line(format!("could not list nodejs.org's releases; installing {v} anyway"));
            v
        } else {
            let tail: Vec<&String> = lines.iter().skip(lines.len().saturating_sub(20)).collect();
            return Err(YardError::new(
                ErrorCode::JobFailed,
                format!("fnm ls-remote failed, so Node {spec} cannot be resolved to a version"),
            )
            .with_hint("check the network, or name an exact version such as 22.18.0")
            .with_data(serde_json::json!({ "tail": tail })));
        };
        if installed(&paths).contains(&target) {
            job.line(format!("Node {target} is already installed"));
            return Ok(());
        }
        // A download that failed or was cancelled leaves the directory without bin/node;
        // fnm would call that installed and refuse, so it goes first.
        let partial = paths.node_versions_dir().join(format!("v{target}"));
        if partial.exists() {
            job.line(format!("Removing {}, which an earlier install left unfinished", partial.display()));
            std::fs::remove_dir_all(&partial)
                .map_err(|e| YardError::internal(format!("cannot remove {}: {e}", partial.display())))?;
        }
        job.line(format!("Installing Node {target}"));
        job.run(&fnm, &["install", "--progress", "never", &format!("v{target}")], &env).await
    }))
}

/// Refused before any job while the default or a linked site has nothing else to run; the
/// job checks again when it starts, since it may wait behind another.
pub(super) async fn uninstall(ctx: &Context, p: NodeVersionParams) -> Result<JobStarted, YardError> {
    let v: NodeVersion = p.version.parse().map_err(|e: YardError| e.with_hint("yard node list"))?;
    if !installed(&ctx.paths).contains(&v) {
        return Err(YardError::new(ErrorCode::NodeMissing, format!("Node {v} is not installed"))
            .with_hint("yard node list shows the installed versions"));
    }
    in_use(v, snapshot(ctx).await?.resolution.sites, &ctx.paths)?;
    let (paths, brew, reconciler) = (ctx.paths.clone(), ctx.brew.clone(), ctx.reconciler.clone());
    Ok(ctx.jobs.start("node-uninstall", move |job: JobCtx| async move {
        let sites = reconciler.snapshot().await.map(|s| s.resolution.sites).unwrap_or_default();
        in_use(v, sites, &paths)?;
        let fnm = brew.fnm_bin();
        if !fnm.exists() {
            return Err(YardError::new(ErrorCode::FormulaMissing, "fnm is not installed")
                .with_hint("brew install fnm"));
        }
        job.run(&fnm, &["uninstall", &format!("v{v}")], &fnm_env(&paths)).await
    }))
}

/// `yard node default`: only to a spec something installed meets, so nothing without a
/// pin stops running.
pub(super) async fn set_default(ctx: &Context, p: NodeSpecParams) -> Result<(), YardError> {
    let spec = spec_arg(&p.spec, "yard node default 22")?;
    if pick(&spec, &installed(&ctx.paths), &lts(&ctx.paths)).is_none() {
        return Err(YardError::new(ErrorCode::NodeMissing, format!("Node {spec} is not installed"))
            .with_hint(install_hint(&spec)));
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
