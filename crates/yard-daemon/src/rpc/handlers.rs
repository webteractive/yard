//! One function per method. Mutations edit a TOML through `TomlFile`, then reconcile;
//! they never write generated files themselves.

use crate::daemon::{BUILD_DATE, COMMIT, Context, VERSION};
use crate::doctor::{self, DoctorInput};
use crate::state::Snapshot;
use crate::{portscan, probe};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use toml_edit::{Array, value};
use yard_core::config::{TomlFile, load_yard};
use yard_core::ports::{NGINX_HTTP, NGINX_HTTPS, PortRange, Registry};
use yard_core::sites::{host_label, parse_proxy_target};
use yard_proto::methods::{
    self, DocrootParams, LinkParams, NameParams, PathParams, PortSetParams, ProfileParams, ProxyParams,
    RangeParams, UnitParams,
};
use yard_proto::{
    DoctorReport, ErrorCode, PROTOCOL_VERSION, PortInfo, Profile, SiteInfo, StatusInfo, UnitState,
    VersionInfo, YardError,
};

pub async fn handle(ctx: &Context, method: &str, params: Value) -> Result<Value, YardError> {
    match method {
        methods::STATUS => json(status(ctx).await),
        methods::DOCTOR => json(Ok(doctor(ctx).await)),
        methods::SITES_LIST => json(Ok(snapshot(ctx).await?.resolution.sites)),
        methods::SITES_LINK => json(link(ctx, parse(params)?).await),
        methods::SITES_UNLINK => json(unlink(ctx, parse(params)?, "path").await),
        methods::SITES_PROXY => json(proxy(ctx, parse(params)?).await),
        methods::SITES_UNPROXY => json(unlink(ctx, parse(params)?, "proxy").await),
        methods::SITES_DOCROOT => json(docroot(ctx, parse(params)?).await),
        methods::SITES_PARK => json(park(ctx, parse(params)?, true).await),
        methods::SITES_UNPARK => json(park(ctx, parse(params)?, false).await),
        methods::PORTS_LIST | methods::PORTS_CHECK => json(Ok(port_infos(ctx).await)),
        methods::PORTS_SET => {
            let p: PortSetParams = parse(params)?;
            json(edit_ports(ctx, &p.unit, |r| r.set(&p.unit, p.port)).await)
        }
        methods::PORTS_PIN => {
            let p: UnitParams = parse(params)?;
            json(edit_ports(ctx, &p.unit, |r| r.pin(&p.unit)).await)
        }
        methods::PORTS_UNPIN => {
            let p: UnitParams = parse(params)?;
            json(edit_ports(ctx, &p.unit, |r| r.unpin(&p.unit)).await)
        }
        methods::PORTS_RANGE => {
            let p: RangeParams = parse(params)?;
            let range: PortRange = p.range.parse()?;
            json(
                edit_registry(ctx, |r| {
                    r.set_range(range);
                    Ok(())
                })
                .await,
            )
        }
        methods::PORTS_PROFILE => {
            let p: ProfileParams = parse(params)?;
            json(profile(ctx, p.profile).await)
        }
        methods::UNITS_RESTART => {
            let p: UnitParams = parse(params)?;
            json(ctx.supervisor().restart(&p.unit).await)
        }
        _ => Err(YardError::new(ErrorCode::MethodNotFound, format!("yardd has no method {method:?}"))),
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, YardError> {
    serde_json::from_value(params)
        .map_err(|e| YardError::new(ErrorCode::InvalidArgument, format!("bad parameters: {e}")))
}

fn json<T: Serialize>(result: Result<T, YardError>) -> Result<Value, YardError> {
    serde_json::to_value(result?).map_err(YardError::internal)
}

/// The last good state, or why there is none yet.
async fn snapshot(ctx: &Context) -> Result<Snapshot, YardError> {
    match ctx.reconciler.snapshot().await {
        Some(s) => Ok(s),
        None => Err(ctx
            .reconciler
            .config_error()
            .await
            .unwrap_or_else(|| YardError::internal("yardd has not finished its first reconcile"))),
    }
}

async fn reconcile(ctx: &Context) -> Result<(), YardError> {
    let report = ctx.reconciler.reconcile().await?;
    report.warnings.iter().for_each(|w| tracing::warn!("{w}"));
    Ok(())
}

async fn site(ctx: &Context, name: &str) -> Result<SiteInfo, YardError> {
    snapshot(ctx).await?.resolution.sites.into_iter().find(|s| s.name == name).ok_or_else(|| not_found(name))
}

fn not_found(name: &str) -> YardError {
    YardError::new(ErrorCode::SiteNotFound, format!("no site named {name:?}"))
        .with_hint("yard sites lists them")
}

fn valid_name(name: &str) -> Result<(), YardError> {
    if host_label(name).as_deref() == Some(name) {
        Ok(())
    } else {
        Err(YardError::new(ErrorCode::InvalidArgument, format!("{name:?} is not a valid site name"))
            .with_hint("use lowercase letters, digits and dashes"))
    }
}

fn directory(path: &str) -> Result<PathBuf, YardError> {
    std::fs::canonicalize(path)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| YardError::new(ErrorCode::InvalidArgument, format!("{path} is not a directory")))
}

async fn status(ctx: &Context) -> Result<StatusInfo, YardError> {
    let snapshot = ctx.reconciler.snapshot().await;
    let helper = ctx.helper.status().await.ok().map(|s| s.version);
    let profile = snapshot.as_ref().map(|s| s.registry.profile).unwrap_or_default();
    Ok(StatusInfo {
        version: VersionInfo {
            cli: None,
            daemon: Some(VERSION.into()),
            helper,
            protocol: PROTOCOL_VERSION.into(),
            commit: COMMIT.into(),
            build_date: BUILD_DATE.into(),
        },
        profile,
        tld: snapshot.as_ref().map_or_else(|| profile.tld().to_string(), |s| s.tld.clone()),
        parked: snapshot
            .as_ref()
            .map(|s| s.yard.parked.iter().map(|p| p.display().to_string()).collect())
            .unwrap_or_default(),
        units: ctx.supervisor().list(),
        sites: snapshot.as_ref().map_or(0, |s| s.resolution.sites.len()),
        config_error: ctx.reconciler.config_error().await,
    })
}

async fn link(ctx: &Context, p: LinkParams) -> Result<SiteInfo, YardError> {
    valid_name(&p.name)?;
    let dir = directory(&p.path)?;
    let mut file = TomlFile::load(&ctx.paths.sites_toml())?;
    file.remove(&["sites", &p.name, "proxy"]);
    file.set(&["sites", &p.name, "path"], value(dir.display().to_string()));
    file.save()?;
    reconcile(ctx).await?;
    site(ctx, &p.name).await
}

async fn proxy(ctx: &Context, p: ProxyParams) -> Result<SiteInfo, YardError> {
    valid_name(&p.name)?;
    parse_proxy_target(&p.url)?;
    let mut file = TomlFile::load(&ctx.paths.sites_toml())?;
    file.remove(&["sites", &p.name, "path"]);
    file.set(&["sites", &p.name, "proxy"], value(p.url.as_str()));
    file.save()?;
    reconcile(ctx).await?;
    site(ctx, &p.name).await
}

/// `unlink` and `unproxy`: the entry must exist with that `key`.
async fn unlink(ctx: &Context, p: NameParams, key: &str) -> Result<(), YardError> {
    let mut file = TomlFile::load(&ctx.paths.sites_toml())?;
    let present = file.doc().get("sites").and_then(|s| s.get(&p.name)).and_then(|s| s.get(key)).is_some();
    if !present {
        let kind = if key == "proxy" { "proxied" } else { "linked" };
        return Err(YardError::new(ErrorCode::SiteNotFound, format!("no {kind} site named {:?}", p.name))
            .with_hint("yard sites lists them"));
    }
    file.remove(&["sites", &p.name]);
    file.save()?;
    reconcile(ctx).await
}

async fn docroot(ctx: &Context, p: DocrootParams) -> Result<SiteInfo, YardError> {
    site(ctx, &p.site).await?;
    let mut file = TomlFile::load(&ctx.paths.sites_toml())?;
    match &p.path {
        Some(path) => file.set(&["sites", &p.site, "docroot"], value(path.as_str())),
        None => {
            file.remove(&["sites", &p.site, "docroot"]);
        }
    }
    file.save()?;
    reconcile(ctx).await?;
    site(ctx, &p.site).await
}

/// `park` adds and `unpark` removes. With no `parked` key the effective list is the
/// default (`~/Herd`); it is written out first so editing it does not drop the default.
async fn park(ctx: &Context, p: PathParams, add: bool) -> Result<(), YardError> {
    let dir = if add {
        directory(&p.path)?
    } else {
        std::fs::canonicalize(&p.path).unwrap_or_else(|_| PathBuf::from(&p.path))
    };
    let dir = dir.display().to_string();
    let mut file = TomlFile::load(&ctx.paths.yard_toml())?;
    if file.doc().get("parked").is_none() {
        let mut current = Array::new();
        for path in load_yard(&ctx.paths)?.parked {
            current.push(path.display().to_string());
        }
        file.set(&["parked"], value(current));
    }
    let changed =
        if add { file.array_push_unique(&["parked"], &dir) } else { file.array_remove(&["parked"], &dir) };
    if !add && !changed {
        return Err(YardError::new(ErrorCode::InvalidArgument, format!("{dir} is not parked"))
            .with_hint("yard status lists the parked paths"));
    }
    file.save()?;
    reconcile(ctx).await
}

/// Status for every registered port: `yard` when its unit runs, otherwise a probe.
/// External entries are checked by connecting, since Yard does not run them.
async fn doctor(ctx: &Context) -> DoctorReport {
    let state = ctx.reconciler.state().await;
    doctor::run(DoctorInput {
        paths: &ctx.paths,
        brew: &ctx.brew,
        helper: &ctx.helper,
        state: Some(&state),
        units: ctx.supervisor().list(),
        daemon_running: true,
        resolver_dir: &ctx.resolver_dir,
    })
    .await
}

async fn port_infos(ctx: &Context) -> Vec<PortInfo> {
    let registry = match ctx.reconciler.snapshot().await {
        Some(s) => s.registry,
        None => match Registry::load(&ctx.paths) {
            Ok(r) => r,
            Err(_) => return vec![],
        },
    };
    let running: BTreeSet<String> = ctx
        .supervisor()
        .list()
        .into_iter()
        .filter(|u| u.state == UnitState::Running)
        .map(|u| u.name)
        .collect();
    tokio::task::spawn_blocking(move || portscan::infos(&registry, &running)).await.unwrap_or_default()
}

async fn edit_registry(
    ctx: &Context,
    change: impl FnOnce(&mut Registry) -> Result<(), YardError>,
) -> Result<(), YardError> {
    let mut registry = Registry::load(&ctx.paths)?;
    change(&mut registry)?;
    let mut file = TomlFile::load(&ctx.paths.ports_toml())?;
    registry.save(&mut file);
    file.save()?;
    reconcile(ctx).await
}

async fn edit_ports(
    ctx: &Context,
    unit: &str,
    change: impl FnOnce(&mut Registry) -> Result<(), YardError>,
) -> Result<PortInfo, YardError> {
    edit_registry(ctx, change).await?;
    port_infos(ctx).await.into_iter().find(|p| p.unit == unit).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, format!("no port registered for {unit:?}"))
            .with_hint("yard ports lists the registered units")
    })
}

/// A profile switch may land on the ports nginx already holds; those count as free.
async fn profile(ctx: &Context, profile: Profile) -> Result<(), YardError> {
    let nginx_running =
        ctx.supervisor().list().iter().any(|u| u.name == "nginx" && u.state == UnitState::Running);
    let current = Registry::load(&ctx.paths)?;
    let own: BTreeSet<u16> = [NGINX_HTTP, NGINX_HTTPS]
        .iter()
        .filter(|_| nginx_running)
        .filter_map(|unit| current.entries.get(*unit).map(|e| e.port))
        .collect();
    let free = tokio::task::spawn_blocking(move || {
        [profile.http_port(), profile.https_port()]
            .into_iter()
            .map(|p| (p, own.contains(&p) || probe::is_free(p, probe::Proto::Tcp)))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(YardError::internal)?;
    let is_free = |p: u16| free.iter().any(|(port, ok)| *port == p && *ok);
    edit_registry(ctx, |r| r.set_profile(profile, &is_free)).await
}
