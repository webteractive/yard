//! `tunnels.*` (tunnels spec §8).

use super::handlers::site;
use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::reconcile::Reconciler;
use crate::services::{missing_program, tail};
use crate::tunnel::named::{self, AddNamed};
use crate::tunnel::{self, Runtime};
use std::path::Path;
use std::sync::PoisonError;
use std::time::{Duration, Instant};
use yard_core::Paths;
use yard_core::config::{TomlFile, load_yard};
use yard_core::tunnel::cloudflared::cert_path;
use yard_core::tunnel::{domain_for, unit, valid_host};
use yard_proto::methods::{TunnelAddParams, TunnelDomainAddParams, TunnelDomainParams, TunnelRemoveParams};
use yard_proto::{
    ErrorCode, JobStarted, TunnelDomain, TunnelInfo, TunnelProvider, TunnelRemoved, TunnelState, YardError,
};

/// `INVALID_ARGUMENT` for a provider other than cloudflared, the only one built.
fn check_provider(provider: Option<&str>) -> Result<(), YardError> {
    match provider.filter(|name| *name != "cloudflared") {
        Some(name) => Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("no tunnel provider {name}; the providers are: cloudflared"),
        )),
        None => Ok(()),
    }
}

/// How often `tunnels.add` looks whether its tunnel is ready.
const READY_POLL: Duration = Duration::from_millis(100);

/// Log lines `TUNNEL_NOT_READY` carries in `data.tail`.
const TAIL_LINES: usize = 20;

/// Every tunnel the last reconcile ran, by site.
pub async fn list(ctx: &Context) -> Result<Vec<TunnelInfo>, YardError> {
    Ok(infos(&ctx.reconciler, &ctx.tunnels).await)
}

async fn infos(reconciler: &Reconciler, runtime: &Runtime) -> Vec<TunnelInfo> {
    let Some(snapshot) = reconciler.snapshot().await else { return vec![] };
    let units = reconciler.supervisor().list();
    let live = runtime.live.lock().unwrap_or_else(PoisonError::into_inner);
    snapshot
        .tunnels
        .iter()
        .map(|(site, planned)| {
            let u = units.iter().find(|u| u.name == unit(site));
            tunnel::info(site, planned, u, live.get(site))
        })
        .collect()
}

/// Whether `site` has a tunnel: a quick one, or a `[sites.<site>.tunnel]`.
fn has_tunnel(ctx: &Context, site: &str) -> bool {
    let named = ctx
        .reconciler
        .read(|s| s.sites_cfg.sites.get(site).is_some_and(|e| e.tunnel.is_some()))
        .unwrap_or(false);
    named || ctx.tunnels.quick_sites().contains(site)
}

/// `tunnels.add`: checks what can be checked at once, then a job starts the tunnel and waits
/// for it to reach Cloudflare. A quick tunnel that does not is removed again.
pub async fn add(ctx: &Context, p: TunnelAddParams) -> Result<JobStarted, YardError> {
    site(ctx, &p.site).await?;
    check_provider(p.provider.as_deref())?;
    if has_tunnel(ctx, &p.site) {
        return Err(YardError::new(ErrorCode::InvalidArgument, format!("{} already has a tunnel", p.site))
            .with_hint(format!("yard untunnel {}", p.site)));
    }
    if let Some(missing) =
        missing_program(&ctx.brew, tunnel::cloudflared::FORMULA, &ctx.brew.cloudflared_bin())
    {
        return Err(missing);
    }
    if let Some(host) = &p.host {
        let named = check_named(ctx, &p, host)?;
        return Ok(start_named(ctx, named));
    }
    if p.tunnel.is_some() {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            "a tunnel to adopt needs the host to route to it",
        )
        .with_hint(format!("yard tunnel {} --host <host> --tunnel <name>", p.site)));
    }
    let (reconciler, runtime, paths, wait) =
        (ctx.reconciler.clone(), ctx.tunnels.clone(), ctx.paths.clone(), ctx.tunnel_ready_wait);
    let site = p.site;
    Ok(ctx.jobs.start("tunnel-add", move |job: JobCtx| async move {
        job.line(format!("Starting a quick tunnel for {site}…"));
        runtime.add_quick(&site);
        let started = async {
            reconciler.reconcile().await?;
            ready(&job, &reconciler, &runtime, &paths, &site, wait).await
        }
        .await;
        match started {
            Ok(info) => {
                job.set_result(serde_json::to_value(info).map_err(YardError::internal)?);
                Ok(())
            }
            Err(e) => {
                runtime.remove_quick(&site);
                if let Err(again) = reconciler.reconcile().await {
                    tracing::warn!("removing {site}'s quick tunnel: {}", crate::describe(&again));
                }
                Err(e)
            }
        }
    }))
}

/// The checks a named tunnel gets before its job: a valid host under a domain that is set up and
/// signed in, and no other site's tunnel on it.
fn check_named(ctx: &Context, p: &TunnelAddParams, host: &str) -> Result<AddNamed, YardError> {
    if !valid_host(host) {
        return Err(YardError::new(ErrorCode::InvalidArgument, format!("{host:?} is not a host name"))
            .with_hint("give a lowercase name such as blog.example.com"));
    }
    let (yard, _) = load_yard(&ctx.paths)?;
    let Some(domain) = domain_for(host, &yard.tunnel_domains) else {
        let labels: Vec<&str> = host.rsplitn(3, '.').take(2).collect();
        let zone = format!("{}.{}", labels[1], labels[0]);
        return Err(YardError::new(
            ErrorCode::TunnelDomainNotSetUp,
            format!("{host} is not under a domain set up for tunnels"),
        )
        .with_hint(format!("yard tunnels domain add {zone}")));
    };
    if !cert_path(ctx.reconciler.home(), domain).exists() {
        return Err(tunnel::cloudflared::not_logged_in(domain));
    }
    let taken = ctx
        .reconciler
        .read(|s| {
            s.sites_cfg
                .sites
                .iter()
                .find(|(name, e)| *name != &p.site && e.tunnel.as_ref().is_some_and(|t| t.host() == host))
                .map(|(name, _)| name.clone())
        })
        .flatten();
    if let Some(other) = taken {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{host} is already {other}'s tunnel host"),
        )
        .with_hint(format!("yard untunnel {other}")));
    }
    Ok(AddNamed {
        site: p.site.clone(),
        host: host.to_string(),
        domain: domain.to_string(),
        tunnel: p.tunnel.clone(),
        overwrite_dns: p.overwrite_dns,
    })
}

/// The `tunnel-add` job for a named tunnel: set it up, then wait for it to be ready. A tunnel
/// that is set up but not ready yet keeps its config; its unit keeps trying.
fn start_named(ctx: &Context, a: AddNamed) -> JobStarted {
    let (reconciler, runtime, brew, paths, wait) = (
        ctx.reconciler.clone(),
        ctx.tunnels.clone(),
        ctx.brew.clone(),
        ctx.paths.clone(),
        ctx.tunnel_ready_wait,
    );
    ctx.jobs.start("tunnel-add", move |job: JobCtx| async move {
        named::add(&job, &reconciler, &brew, &paths, &a).await?;
        let info = ready(&job, &reconciler, &runtime, &paths, &a.site, wait).await?;
        job.set_result(serde_json::to_value(info).map_err(YardError::internal)?);
        Ok(())
    })
}

/// The tunnel's info once it is ready and has its URL; `TUNNEL_NOT_READY` after `wait`, and
/// `JOB_CANCELLED` when the job is cancelled meanwhile.
async fn ready(
    job: &JobCtx,
    reconciler: &Reconciler,
    runtime: &Runtime,
    paths: &Paths,
    site: &str,
    wait: Duration,
) -> Result<TunnelInfo, YardError> {
    let deadline = Instant::now() + wait;
    loop {
        job.check_cancelled()?;
        let info = infos(reconciler, runtime).await.into_iter().find(|t| t.site == site);
        if let Some(info) = info.filter(|t| t.state == TunnelState::Ready && t.url.is_some()) {
            return Ok(info);
        }
        if Instant::now() >= deadline {
            return Err(not_ready(paths, site, wait));
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

fn not_ready(paths: &Paths, site: &str, wait: Duration) -> YardError {
    let name = unit(site);
    let log = paths.log(&name);
    let lines = tail(&log, TAIL_LINES).unwrap_or_default();
    YardError::new(
        ErrorCode::TunnelNotReady,
        format!("{name} did not connect to Cloudflare within {} s", wait.as_secs()),
    )
    .with_hint(format!("tail -n 50 {}", log.display()))
    .with_data(serde_json::json!({ "tail": lines }))
}

/// `tunnels.remove`: a site with no tunnel is refused at once; a job ends the tunnel.
pub async fn remove(ctx: &Context, p: TunnelRemoveParams) -> Result<JobStarted, YardError> {
    if !has_tunnel(ctx, &p.site) {
        return Err(YardError::new(ErrorCode::TunnelNotFound, format!("{} has no tunnel", p.site))
            .with_hint(format!("yard tunnel {}", p.site)));
    }
    if !ctx.tunnels.quick_sites().contains(&p.site) {
        let (reconciler, brew, paths) = (ctx.reconciler.clone(), ctx.brew.clone(), ctx.paths.clone());
        return Ok(ctx.jobs.start("tunnel-remove", move |job: JobCtx| async move {
            let removed = named::remove(&job, &reconciler, &brew, &paths, &p.site, p.keep_tunnel).await?;
            job.set_result(serde_json::to_value(removed).map_err(YardError::internal)?);
            Ok(())
        }));
    }
    let (reconciler, runtime) = (ctx.reconciler.clone(), ctx.tunnels.clone());
    let site = p.site;
    Ok(ctx.jobs.start("tunnel-remove", move |job: JobCtx| async move {
        job.line(format!("Ending {site}'s quick tunnel…"));
        runtime.remove_quick(&site);
        reconciler.reconcile().await?;
        let removed = TunnelRemoved { site, deleted_tunnel: false, dns_left: vec![] };
        job.set_result(serde_json::to_value(removed).map_err(YardError::internal)?);
        Ok(())
    }))
}

const DOMAINS: [&str; 3] = ["tunnels", "cloudflared", "domains"];

fn domain_info(home: &Path, domain: &str) -> TunnelDomain {
    TunnelDomain {
        provider: TunnelProvider::Cloudflared,
        domain: domain.to_string(),
        logged_in: cert_path(home, domain).exists(),
    }
}

/// `tunnels.domains`: each domain in `yard.toml`, and whether cloudflared is signed in for it.
pub async fn domains(ctx: &Context) -> Result<Vec<TunnelDomain>, YardError> {
    let (yard, _) = load_yard(&ctx.paths)?;
    let home = ctx.reconciler.home();
    Ok(yard.tunnel_domains.iter().map(|d| domain_info(home, d)).collect())
}

/// `tunnels.domain_add`: a job signs cloudflared in for the domain, then records it in
/// `yard.toml`. A failed sign-in records nothing.
pub async fn domain_add(ctx: &Context, p: TunnelDomainAddParams) -> Result<JobStarted, YardError> {
    check_provider(p.provider.as_deref())?;
    if !valid_host(&p.domain) {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{:?} is not a domain name", p.domain),
        )
        .with_hint("give the zone as Cloudflare shows it, e.g. example.com"));
    }
    if let Some(missing) =
        missing_program(&ctx.brew, tunnel::cloudflared::FORMULA, &ctx.brew.cloudflared_bin())
    {
        return Err(missing);
    }
    let (reconciler, brew, paths) = (ctx.reconciler.clone(), ctx.brew.clone(), ctx.paths.clone());
    let domain = p.domain;
    Ok(ctx.jobs.start("tunnel-login", move |job: JobCtx| async move {
        let home = reconciler.home().to_path_buf();
        tunnel::cloudflared::login(&job, &brew, &home, &domain).await?;
        TomlFile::edit(&paths.yard_toml(), |file| {
            file.array_push_unique(&DOMAINS, &domain);
            Ok(())
        })?;
        reconciler.reconcile().await?;
        job.set_result(serde_json::to_value(domain_info(&home, &domain)).map_err(YardError::internal)?);
        Ok(())
    }))
}

/// `tunnels.domain_remove`: forgets a domain; its certificate stays.
pub async fn domain_remove(ctx: &Context, p: TunnelDomainParams) -> Result<Vec<TunnelDomain>, YardError> {
    TomlFile::edit(&ctx.paths.yard_toml(), |file| {
        if file.array_remove(&DOMAINS, &p.domain) {
            Ok(())
        } else {
            Err(YardError::new(ErrorCode::InvalidArgument, format!("{} is not a tunnel domain", p.domain))
                .with_hint("yard tunnels domains"))
        }
    })?;
    super::handlers::reconcile(ctx).await?;
    domains(ctx).await
}
