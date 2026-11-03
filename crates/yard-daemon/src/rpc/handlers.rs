//! One function per method. Mutations edit a TOML through `TomlFile`, then reconcile;
//! they never write generated files themselves.

use super::{ext, info, node, php, project, services, sources, tunnels};
use crate::daemon::{BUILD_DATE, COMMIT, Context, VERSION};
use crate::doctor::{self, DoctorInput};
use crate::managed::edit_managed;
use crate::project::read_project_file;
use crate::reconcile::Reconciler;
use crate::state::Snapshot;
use crate::{holder, portscan, probe};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use toml_edit::value;
use yard_core::config::yard::DEFAULT_TLD;
use yard_core::config::{TomlFile, load_sites, load_yard};
use yard_core::project::{FILE as PROJECT_FILE, FileKind, relative_docroot};

use yard_core::ports::{NGINX_HTTP, NGINX_HTTPS, PortRange, Registry};
use yard_core::sites::{host_label, parse_proxy_target};
use yard_proto::methods::{
    self, DocrootParams, DoctorParams, JobParams, LinkParams, NameParams, PortSetParams, ProfileParams,
    ProxyParams, RangeParams, SiteParams, UnitParams,
};
use yard_proto::{
    DoctorReport, ErrorCode, Holder, PROTOCOL_VERSION, PortInfo, Profile, SiteInfo, SleepingInstance,
    StatusInfo, UnitState, VersionInfo, YardError,
};

pub async fn handle(ctx: &Context, method: &str, params: Value) -> Result<Value, YardError> {
    match method {
        methods::STATUS => json(status(ctx).await),
        methods::DOCTOR => json(Ok(doctor(ctx, doctor_params(params)?).await)),
        methods::SITES_LIST => {
            let mut sites = snapshot(ctx).await?.resolution.sites;
            crate::node::fill_node(&mut sites, &ctx.paths);
            json(Ok(sites))
        }
        methods::SITES_LINK => json(link(ctx, parse(params)?).await),
        methods::SITES_INFO => json(info::site_detail(ctx, parse(params)?).await),
        methods::ENV_PRINT => json(info::env_print(ctx, parse(params)?).await),
        methods::SITES_INIT => json(project::init(ctx, parse(params)?).await),
        methods::SITES_UNLINK => json(unlink(ctx, parse(params)?, "path").await),
        methods::SITES_PROXY => json(proxy(ctx, parse(params)?).await),
        methods::SITES_UNPROXY => json(unlink(ctx, parse(params)?, "proxy").await),
        methods::SITES_DOCROOT => json(docroot(ctx, parse(params)?).await),
        methods::SITES_ISOLATE => json(php::isolate(ctx, parse(params)?).await),
        methods::SITES_SECURE => json(secure(ctx, parse(params)?, true).await),
        methods::SITES_UNSECURE => json(secure(ctx, parse(params)?, false).await),
        methods::CA_INFO => json(ctx.reconciler.ca_info(&ctx.security).await),
        methods::PORTS_LIST => json(Ok(port_infos(ctx).await)),
        // A check is about the file as it is now, so an edit that broke it fails here.
        methods::PORTS_CHECK => {
            Registry::load(&ctx.paths)?;
            json(Ok(port_infos(ctx).await))
        }
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
        methods::EXT_ADD => json(ext::add(ctx, parse(params)?).await),
        methods::EXT_LIST => json(ext::list(ctx, parse(params)?)),
        methods::EXT_ENABLE => json(ext::toggle(ctx, parse(params)?, true).await),
        methods::EXT_DISABLE => json(ext::toggle(ctx, parse(params)?, false).await),
        methods::EXT_REMOVE => json(ext::remove(ctx, parse(params)?)),
        methods::XDEBUG_SET => json(ext::xdebug(ctx, parse(params)?).await),
        methods::PHP_LIST => json(php::list(ctx).await),
        methods::PHP_INSTALL => json(php::install(ctx, parse(params)?).await),
        methods::PHP_UNINSTALL => json(php::uninstall(ctx, parse(params)?).await),
        methods::PHP_DEFAULT => json(php::set_default(ctx, parse(params)?).await),
        methods::NODE_LIST => json(node::list(ctx, parse_or_default(params)?).await),
        methods::NODE_INSTALL => json(node::install(ctx, parse(params)?).await),
        methods::NODE_UNINSTALL => json(node::uninstall(ctx, parse(params)?).await),
        methods::NODE_DEFAULT => json(node::set_default(ctx, parse(params)?).await),
        methods::NODE_USE => json(node::use_spec(ctx, parse(params)?).await),
        methods::NODE_UNUSE => json(node::unuse(ctx, parse(params)?).await),
        methods::PHP_CONFIG_GET => json(php::config_get(ctx, parse(params)?)),
        methods::PHP_CONFIG_SET => json(php::config_edit(ctx, parse(params)?, true).await.map(|_| ())),
        methods::PHP_CONFIG_UNSET => json(php::config_edit(ctx, parse(params)?, false).await),
        methods::SOURCES_LIST => json(sources::list(ctx).await),
        methods::SOURCES_TRUST => json(sources::change(ctx, parse(params)?, true)),
        methods::SOURCES_UNTRUST => json(sources::change(ctx, parse(params)?, false)),
        methods::SERVICES_LIST => json(services::list(ctx).await),
        methods::TUNNELS_LIST => json(tunnels::list(ctx).await),
        methods::TUNNELS_ADD => json(tunnels::add(ctx, parse(params)?).await),
        methods::TUNNELS_REMOVE => json(tunnels::remove(ctx, parse(params)?).await),
        methods::TUNNELS_DOMAINS => json(tunnels::domains(ctx).await),
        methods::TUNNELS_DOMAIN_ADD => json(tunnels::domain_add(ctx, parse(params)?).await),
        methods::TUNNELS_DOMAIN_REMOVE => json(tunnels::domain_remove(ctx, parse(params)?).await),
        methods::SERVICES_ADD => json(services::add(ctx, parse(params)?).await),
        methods::SERVICES_REMOVE => json(services::remove(ctx, parse(params)?).await),
        methods::SERVICES_START => json(services::set_stopped(ctx, parse(params)?, false).await),
        methods::SERVICES_STOP => json(services::set_stopped(ctx, parse(params)?, true).await),
        methods::SERVICES_RESTART => json(services::restart(ctx, parse(params)?).await),
        methods::SERVICES_LOGS => json(services::logs(ctx, parse(params)?).await),
        methods::SERVICES_PORT => json(services::port(ctx, parse(params)?).await),
        methods::SERVICES_SLEEP_SET => json(services::set_sleep(ctx, parse(params)?).await),
        methods::JOBS_CANCEL => {
            let p: JobParams = parse(params)?;
            json(ctx.jobs.cancel(&p.job_id))
        }
        methods::UNITS_RESTART => {
            let p: UnitParams = parse(params)?;
            json(restart(ctx, &p.unit).await)
        }
        _ => Err(YardError::new(ErrorCode::MethodNotFound, format!("yardd has no method {method:?}"))),
    }
}

pub(super) fn parse<T: DeserializeOwned>(params: Value) -> Result<T, YardError> {
    serde_json::from_value(params)
        .map_err(|e| YardError::new(ErrorCode::InvalidArgument, format!("bad parameters: {e}")))
}

/// `parse`, with `null` params as the type's defaults.
fn parse_or_default<T: DeserializeOwned + Default>(params: Value) -> Result<T, YardError> {
    if params.is_null() { Ok(T::default()) } else { parse(params) }
}

pub(super) fn json<T: Serialize>(result: Result<T, YardError>) -> Result<Value, YardError> {
    serde_json::to_value(result?).map_err(YardError::internal)
}

/// The last good state, or why there is none yet.
pub(super) async fn snapshot(ctx: &Context) -> Result<Snapshot, YardError> {
    snapshot_of(&ctx.reconciler).await
}

/// [`snapshot`] for a job, which holds the reconciler rather than the context.
pub(super) async fn snapshot_of(reconciler: &Reconciler) -> Result<Snapshot, YardError> {
    match reconciler.snapshot().await {
        Some(s) => Ok(s),
        None => Err(reconciler
            .last_error()
            .await
            .unwrap_or_else(|| YardError::internal("yardd has not finished its first reconcile"))),
    }
}

pub(super) async fn reconcile(ctx: &Context) -> Result<(), YardError> {
    let report = ctx.reconciler.reconcile().await?;
    report.warnings.iter().for_each(|w| tracing::warn!("{w}"));
    Ok(())
}

/// The site as `sites.list` shows it, its Node part filled.
pub(super) async fn site(ctx: &Context, name: &str) -> Result<SiteInfo, YardError> {
    let site = snapshot(ctx)
        .await?
        .resolution
        .sites
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| not_found(name))?;
    let mut one = [site];
    crate::node::fill_node(&mut one, &ctx.paths);
    let [site] = one;
    Ok(site)
}

pub(super) fn not_found(name: &str) -> YardError {
    YardError::new(ErrorCode::SiteNotFound, format!("no site named {name:?}"))
        .with_hint("yard sites lists them")
}

/// Where `isolate`, `secure` and `docroot` make their change for a site.
pub(super) enum Target {
    SitesToml,
    /// The site is served from its managed `.yard`, which they edit (managed-file spec §4).
    Managed(PathBuf),
}

/// `SITE_HAS_PROJECT_FILE` when the site's project has a `.yard`, which owns every
/// setting `isolate`, `secure` and `docroot` would change: `change` is the edit to make
/// in the file instead, since Yard never edits a project's own file. A site served from
/// its managed `.yard` is changed there; one that does not parse is its error.
pub(super) fn project_file_target(ctx: &Context, site: &str, change: &str) -> Result<Target, YardError> {
    let sites = load_sites(&ctx.paths)?;
    let Some(path) = sites.sites.get(site).and_then(|e| e.path.as_ref()) else {
        return Ok(Target::SitesToml);
    };
    let file = path.join(PROJECT_FILE);
    if file.symlink_metadata().is_ok() {
        return Err(YardError::new(
            ErrorCode::SiteHasProjectFile,
            format!("{site} is configured by {}", file.display()),
        )
        .with_hint(format!("edit {}: {change}", file.display())));
    }
    let managed = ctx.paths.managed_project_file(site);
    let Some(text) = read_project_file(&managed) else { return Ok(Target::SitesToml) };
    match yard_core::project::parse_managed(&text?, &managed)? {
        (_, Some(project)) if project == *path => Ok(Target::Managed(managed)),
        // Written for another directory: not this site's, so not used and not edited.
        _ => Ok(Target::SitesToml),
    }
}

pub(super) fn valid_name(name: &str) -> Result<(), YardError> {
    if host_label(name).as_deref() == Some(name) {
        Ok(())
    } else {
        Err(YardError::new(ErrorCode::InvalidArgument, format!("{name:?} is not a valid site name"))
            .with_hint("use lowercase letters, digits and dashes"))
    }
}

pub(super) fn directory(path: &str) -> Result<PathBuf, YardError> {
    std::fs::canonicalize(path)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| YardError::new(ErrorCode::InvalidArgument, format!("{path} is not a directory")))
}

async fn status(ctx: &Context) -> Result<StatusInfo, YardError> {
    let snapshot = ctx.reconciler.snapshot().await;
    let helper = ctx.helper.status().await.ok().map(|s| s.version);
    let profile = snapshot.as_ref().map(|s| s.registry.profile).unwrap_or_default();
    let asleep = ctx.sleeper.asleep();
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
        tld: snapshot.as_ref().map_or_else(
            || {
                load_yard(&ctx.paths)
                    .map_or_else(|_| DEFAULT_TLD.to_string(), |(y, _)| y.effective_tld().to_string())
            },
            |s| s.tld.clone(),
        ),
        // An instance is marked asleep before its unit has finished stopping: it is reported
        // once, as sleeping, never also as a unit on its way down.
        units: ctx.supervisor().list().into_iter().filter(|u| !asleep.contains_key(&u.name)).collect(),
        sites: snapshot.as_ref().map_or(0, |s| s.resolution.sites.len()),
        config_error: ctx.reconciler.last_error().await,
        sleeping: asleep
            .into_iter()
            .filter_map(|(name, since)| {
                Some(SleepingInstance { name, since: crate::services::rfc3339(since)? })
            })
            .collect(),
    })
}

/// Links the directory. When the name's managed `.yard` is for it, the site is served from
/// that file, which owns its settings: `php`, `secure` and `docroot` leave `sites.toml`,
/// as `init` moves them (managed-file spec §5).
async fn link(ctx: &Context, p: LinkParams) -> Result<SiteInfo, YardError> {
    valid_name(&p.name)?;
    let dir = directory(&p.path)?;
    TomlFile::edit(&ctx.paths.sites_toml(), |file| {
        file.remove(&["sites", &p.name, "proxy"]);
        file.set(&["sites", &p.name, "path"], value(dir.display().to_string()));
        Ok(())
    })?;
    reconcile(ctx).await?;
    let managed = snapshot(ctx).await?.projects.get(&p.name).is_some_and(|l| l.kind == FileKind::Managed);
    if managed {
        TomlFile::edit(&ctx.paths.sites_toml(), |file| {
            for key in ["php", "secure", "docroot"] {
                file.remove(&["sites", &p.name, key]);
            }
            Ok(())
        })?;
        reconcile(ctx).await?;
    }
    site(ctx, &p.name).await
}

async fn proxy(ctx: &Context, p: ProxyParams) -> Result<SiteInfo, YardError> {
    valid_name(&p.name)?;
    parse_proxy_target(&p.url)?;
    TomlFile::edit(&ctx.paths.sites_toml(), |file| {
        file.remove(&["sites", &p.name, "path"]);
        file.set(&["sites", &p.name, "proxy"], value(p.url.as_str()));
        Ok(())
    })?;
    reconcile(ctx).await?;
    site(ctx, &p.name).await
}

/// `unlink` and `unproxy`: the entry must exist with that `key`.
async fn unlink(ctx: &Context, p: NameParams, key: &str) -> Result<(), YardError> {
    TomlFile::edit(&ctx.paths.sites_toml(), |file| {
        let present = file.doc().get("sites").and_then(|s| s.get(&p.name)).and_then(|s| s.get(key)).is_some();
        if !present {
            let kind = if key == "proxy" { "proxied" } else { "linked" };
            return Err(YardError::new(
                ErrorCode::SiteNotFound,
                format!("no {kind} site named {:?}", p.name),
            )
            .with_hint("yard sites lists them"));
        }
        let tunnelled =
            file.doc().get("sites").and_then(|s| s.get(&p.name)).and_then(|s| s.get("tunnel")).is_some();
        if tunnelled {
            return Err(YardError::new(
                ErrorCode::SiteTunnelled,
                format!("{} is shared through a named tunnel", p.name),
            )
            .with_hint(format!("yard untunnel {}", p.name)));
        }
        file.remove(&["sites", &p.name]);
        Ok(())
    })?;
    // A quick tunnel ends with its site (P8).
    ctx.tunnels.remove_quick(&p.name);
    reconcile(ctx).await
}

async fn docroot(ctx: &Context, p: DocrootParams) -> Result<SiteInfo, YardError> {
    let change = match &p.path {
        Some(path) => format!("docroot = {path:?}"),
        None => "remove docroot".to_string(),
    };
    let target = project_file_target(ctx, &p.site, &change)?;
    let current = site(ctx, &p.site).await?;
    let (path, keys) = match &target {
        Target::Managed(file) => (file.clone(), vec!["docroot"]),
        Target::SitesToml => (ctx.paths.sites_toml(), vec!["sites", &p.site, "docroot"]),
    };
    // A managed `.yard` holds a docroot inside the project, relative to it.
    let docroot = match (&target, &p.path, &current.kind) {
        (Target::Managed(_), Some(docroot), yard_proto::SiteKind::Php { path: project, .. }) => {
            let project = Path::new(project);
            let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            let relative = relative_docroot(project, docroot).or_else(|| {
                let docroot = canonical(Path::new(docroot)).to_string_lossy().into_owned();
                relative_docroot(&canonical(project), &docroot)
            });
            Some(relative.ok_or_else(|| {
                YardError::new(
                    ErrorCode::InvalidArgument,
                    format!("docroot {docroot:?} is outside {}", project.display()),
                )
                .with_hint("yard docroot <path inside the project>, such as public")
            })?)
        }
        _ => p.path.clone(),
    };
    let set = |file: &mut TomlFile| {
        match &docroot {
            Some(path) => file.set(&keys, value(path.as_str())),
            None => {
                file.remove(&keys);
            }
        }
        Ok(())
    };
    match target {
        Target::Managed(_) => edit_managed(&path, set)?,
        Target::SitesToml => TomlFile::edit(&path, set)?,
    }
    reconcile(ctx).await?;
    site(ctx, &p.site).await
}

/// `sites.toml` keeps `secure` only where it differs from `secure_by_default`, so
/// securing then unsecuring a site leaves the file as it was. The default is read from
/// `yard.toml` as it is now: the last good state can predate an edit to it.
async fn secure(ctx: &Context, p: SiteParams, on: bool) -> Result<SiteInfo, YardError> {
    let target = project_file_target(ctx, &p.site, &format!("secure = {on}"))?;
    site(ctx, &p.site).await?;
    if let Target::Managed(file) = target {
        // The file says it either way, so it never depends on `secure_by_default`.
        edit_managed(&file, |toml| {
            toml.set(&["secure"], value(on));
            Ok(())
        })?;
        reconcile(ctx).await?;
        return site(ctx, &p.site).await;
    }
    let default = load_yard(&ctx.paths)?.0.secure_by_default;
    TomlFile::edit(&ctx.paths.sites_toml(), |file| {
        if on == default {
            file.remove(&["sites", &p.site, "secure"]);
        } else {
            file.set(&["sites", &p.site, "secure"], value(on));
        }
        Ok(())
    })?;
    reconcile(ctx).await?;
    site(ctx, &p.site).await
}

/// A restarted nginx loads every override, so a broken one is refused here instead of
/// taking every site down. Without a helper the restart itself reports that.
async fn restart(ctx: &Context, unit: &str) -> Result<(), YardError> {
    // A config nginx rejects, or an nginx that cannot run, would turn a restart into an outage.
    if unit == "nginx"
        && let Ok(Err(e)) | Err(e) = ctx.reconciler.test_nginx().await
    {
        return Err(e);
    }
    ctx.supervisor().restart(unit).await
}

/// `doctor`'s params: older clients send none (`null`) and get an empty `client_env`.
fn doctor_params(params: Value) -> Result<DoctorParams, YardError> {
    if params.is_null() { Ok(DoctorParams::default()) } else { parse(params) }
}

async fn doctor(ctx: &Context, params: DoctorParams) -> DoctorReport {
    let state = ctx.reconciler.state().await;
    doctor::run(DoctorInput {
        paths: &ctx.paths,
        brew: &ctx.brew,
        helper: &ctx.helper,
        state: Some(&state),
        units: ctx.supervisor().list(),
        daemon_running: true,
        resolver_dir: &ctx.resolver_dir,
        security: &ctx.security,
        client_env: params.client_env,
        sleeping: ctx.sleeper.asleep(),
        home: ctx.reconciler.home(),
        tunnels: tunnels::infos(&ctx.reconciler, &ctx.tunnels).await,
        resolve_host: ctx.resolve_host,
    })
    .await
}

/// Every registered port with its status; `portscan::infos` does the probing.
async fn port_infos(ctx: &Context) -> Vec<PortInfo> {
    let registry = match ctx.reconciler.snapshot().await {
        Some(s) => s.registry,
        None => match Registry::load(&ctx.paths) {
            Ok(r) => r,
            Err(_) => return vec![],
        },
    };
    let running = portscan::own_units(&ctx.supervisor().list());
    tokio::task::spawn_blocking(move || portscan::infos(&registry, &running)).await.unwrap_or_default()
}

async fn edit_registry(
    ctx: &Context,
    change: impl FnOnce(&mut Registry) -> Result<(), YardError>,
) -> Result<(), YardError> {
    TomlFile::edit(&ctx.paths.ports_toml(), |file| {
        let mut registry = Registry::load(&ctx.paths)?;
        change(&mut registry)?;
        registry.save(file);
        Ok(())
    })?;
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
    let privileged = profile == Profile::Privileged;
    // Only the helper can open 80/443: without one the switch would leave nothing serving.
    if privileged {
        ctx.helper.status().await?;
    }
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
    // Take the sockets before saving, so an old helper or a lost race refuses the switch.
    let ports = [profile.http_port(), profile.https_port()];
    if privileged && ports.iter().all(|p| is_free(*p)) {
        ctx.supervisor().sockets().get(&ports).await?;
    }
    match edit_registry(ctx, |r| r.set_profile(profile, &is_free)).await {
        Err(e) if e.code == ErrorCode::PortInUse => {
            tokio::task::spawn_blocking(move || Err(name_holders(e, &holder::find_holder)))
                .await
                .map_err(YardError::internal)?
        }
        result => result,
    }
}

/// Adds who holds each refused port (`data.ports`) to the message and to `data.holders`,
/// so a refused profile switch says "Herd" rather than only a port number.
fn name_holders(e: YardError, find: &dyn Fn(u16) -> Option<Holder>) -> YardError {
    let mut data = e.data.clone().unwrap_or_else(|| serde_json::json!({}));
    let ports: Vec<u16> = data["ports"]
        .as_array()
        .map(|a| a.iter().filter_map(|p| p.as_u64()?.try_into().ok()).collect())
        .unwrap_or_default();
    if ports.is_empty() {
        return e;
    }
    let found: Vec<(u16, Option<Holder>)> = ports.into_iter().map(|p| (p, find(p))).collect();
    let named: Vec<String> =
        found.iter().map(|(p, h)| format!("port {p} is held by {}", holder::label(h.as_ref()))).collect();
    data["holders"] = serde_json::json!(
        found.iter().map(|(p, h)| serde_json::json!({ "port": p, "holder": h })).collect::<Vec<_>>()
    );
    YardError { message: format!("{}: {}", e.message, named.join("; ")), data: Some(data), ..e }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yard_proto::{Holder, HolderKind};

    #[test]
    fn profile_refusal_names_each_holder() {
        let refused = YardError::new(
            ErrorCode::PortInUse,
            "cannot switch to the privileged profile: port 80, 443 in use",
        )
        .with_hint("free those ports first, or stay on the standard profile")
        .with_data(json!({"ports": [80, 443]}));
        let herd =
            Holder { pid: 4534, name: "nginx-arm".into(), path: None, ppid: Some(1), kind: HolderKind::Herd };

        let e = name_holders(refused, &|p| (p == 80).then(|| herd.clone()));

        assert_eq!(e.code, ErrorCode::PortInUse);
        assert!(e.message.contains("port 80 is held by nginx-arm (pid 4534, Herd)"), "{}", e.message);
        assert!(
            e.message.contains("port 443 is held by a process not visible to this user"),
            "{}",
            e.message
        );
        let data = e.data.unwrap();
        assert_eq!(data["ports"], json!([80, 443]));
        assert_eq!(data["holders"][0]["port"], 80);
        assert_eq!(data["holders"][0]["holder"]["kind"], "herd");
        assert!(data["holders"][1]["holder"].is_null());
    }
}
