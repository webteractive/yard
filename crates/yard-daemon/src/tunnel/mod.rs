//! Tunnels (M5, tunnels spec): which tunnels a reconcile runs, the two loopback ports and the
//! files each one takes, and its `tunnel-<site>` unit. Quick tunnels live only in yardd's memory
//! (`Runtime`); named ones come from `[sites.<site>.tunnel]` and the facts in
//! `state/tunnels.json`.

pub mod cloudflared;
pub mod monitor;

use crate::describe;
use crate::probe::Proto;
use crate::services::missing_program;
use crate::supervisor::{Health, Reload, UnitSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::SitesConfig;
use yard_core::ports::Registry;
use yard_core::services::render::UnitCommand;
use yard_core::tunnel::cloudflared::{Origin, named_yml, quick_yml};
use yard_core::tunnel::facts::{Facts, NamedFacts};
use yard_core::tunnel::{UNIT_PREFIX, origin_entry, unit};
use yard_proto::{
    ErrorCode, PortMode, SiteInfo, TunnelInfo, TunnelKind, TunnelProvider, TunnelRef, TunnelState, UnitInfo,
    UnitState, YardError,
};

/// How long a tunnel may take to reach Cloudflare before its start counts as failed (P2).
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

const ORIGIN_PREFIX: &str = "nginx-tunnel-";

/// What a running tunnel last said about itself, from its metrics server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    pub ready: bool,
    /// A quick tunnel's `*.trycloudflare.com` host.
    pub hostname: Option<String>,
}

/// Tunnel state that is not in a file. yardd's `Context` and the reconciler share one.
#[derive(Debug, Default)]
pub struct Runtime {
    /// The sites with a quick tunnel; they end with yardd.
    quick: Mutex<BTreeSet<String>>,
    pub live: Mutex<BTreeMap<String, Live>>,
}

impl Runtime {
    pub fn add_quick(&self, site: &str) {
        self.quick.lock().unwrap_or_else(PoisonError::into_inner).insert(site.to_string());
    }

    /// Whether `site` had a quick tunnel.
    pub fn remove_quick(&self, site: &str) -> bool {
        self.quick.lock().unwrap_or_else(PoisonError::into_inner).remove(site)
    }

    pub fn quick_sites(&self) -> BTreeSet<String> {
        self.quick.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A tunnel provider, as far as running one goes (P11): the command its unit runs and the
/// header its edge puts the visitor's address in.
pub trait Provider {
    fn unit_command(&self, brew: &Brew, home: &Path, yml: &Path, kind: TunnelKind) -> UnitCommand;
    fn real_ip_header(&self) -> &'static str;
}

/// The provider a tunnel runs with; cloudflared is the only one built.
pub fn provider() -> &'static dyn Provider {
    &cloudflared::Cloudflared
}

/// A tunnel a reconcile runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub kind: TunnelKind,
    pub origin_port: u16,
    pub metrics_port: u16,
    /// A named tunnel's host.
    pub host: Option<String>,
    pub facts: Option<NamedFacts>,
}

impl Planned {
    fn origin(&self) -> Origin {
        Origin { origin_port: self.origin_port, metrics_port: self.metrics_port }
    }
}

/// How a tunnel looks from outside: `ready` once its unit runs and its metrics server says it
/// reached the edge, `connecting` while it runs or starts otherwise, and the unit's state after
/// that (no unit is `stopped`). A quick tunnel has a URL once its hostname is known.
pub fn info(site: &str, planned: &Planned, unit_info: Option<&UnitInfo>, live: Option<&Live>) -> TunnelInfo {
    let ready = live.is_some_and(|l| l.ready);
    let state = match unit_info.map(|u| &u.state) {
        Some(UnitState::Running) if ready => TunnelState::Ready,
        Some(UnitState::Running | UnitState::Starting) => TunnelState::Connecting,
        Some(UnitState::Backoff { .. }) => TunnelState::Backoff,
        Some(UnitState::Failed { .. }) => TunnelState::Failed,
        Some(UnitState::Stopped) | None => TunnelState::Stopped,
    };
    let url = match planned.kind {
        TunnelKind::Named => planned.host.as_ref(),
        _ => live.and_then(|l| l.hostname.as_ref()),
    }
    .map(|host| format!("https://{host}"));
    TunnelInfo {
        site: site.to_string(),
        provider: TunnelProvider::Cloudflared,
        kind: planned.kind,
        url,
        host: planned.host.clone(),
        tunnel: planned.facts.as_ref().map(|f| TunnelRef {
            name: f.name.clone(),
            id: f.id.clone(),
            created_by_yard: f.created_by_yard,
        }),
        state,
        unit: unit(site),
        origin_port: planned.origin_port,
        metrics_port: planned.metrics_port,
        last_error: unit_info.and_then(|u| u.last_error.clone()),
    }
}

/// `TUNNEL_CREDENTIALS_MISSING` for a named tunnel whose credentials file is gone.
pub fn credentials_missing(site: &str, host: &str, facts: &NamedFacts) -> YardError {
    YardError::new(
        ErrorCode::TunnelCredentialsMissing,
        format!(
            "the credentials file {} of the Cloudflare tunnel {} is gone",
            facts.credentials.display(),
            facts.name
        ),
    )
    .with_hint(format!("yard untunnel {site} && yard tunnel {site} --host {host}"))
}

/// The tunnels this reconcile runs, with their ports in `registry` (both `auto`). A tunnel
/// runs for a site that resolved, named when `sites.toml` gives it a `[tunnel]`, else quick when
/// `runtime` has one. A named tunnel with no recorded facts or no credentials file, or any
/// tunnel while cloudflared is missing, is a warning and runs nothing (P12). The port entries
/// of every tunnel not run are dropped.
#[allow(clippy::too_many_arguments)]
pub fn settle_ports(
    sites_cfg: &SitesConfig,
    sites: &[SiteInfo],
    runtime: &Runtime,
    paths: &Paths,
    brew: &Brew,
    registry: &mut Registry,
    is_free: &dyn Fn(u16) -> bool,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Planned> {
    let facts = Facts::load(&paths.tunnels_state()).unwrap_or_else(|e| {
        warnings.push(format!("tunnels: {}", describe(&e)));
        Facts::default()
    });
    for (site, named) in &facts.named {
        if sites_cfg.sites.get(site).is_none_or(|e| e.tunnel.is_none()) {
            warnings.push(format!(
                "{}: the Cloudflare tunnel {} and its DNS record remain; yard doctor names them",
                unit(site),
                named.name
            ));
        }
    }
    let quick = runtime.quick_sites();
    let missing = missing_program(brew, cloudflared::FORMULA, &brew.cloudflared_bin());
    let mut planned = BTreeMap::new();
    for site in sites {
        let name = site.name.as_str();
        let entry = sites_cfg.sites.get(name).and_then(|e| e.tunnel.as_ref());
        let (kind, host, named) = match entry {
            Some(t) => (TunnelKind::Named, Some(t.host().to_string()), facts.named.get(name).cloned()),
            None if quick.contains(name) => (TunnelKind::Quick, None, None),
            None => continue,
        };
        if let Some(e) = &missing {
            warnings.push(format!("{}: {}", unit(name), describe(e)));
            continue;
        }
        if let Some(host) = &host {
            let Some(f) = &named else {
                warnings.push(format!(
                    "{}: no Cloudflare tunnel is recorded for it; run yard tunnel {name} --host {host}",
                    unit(name)
                ));
                continue;
            };
            if !f.credentials.exists() {
                warnings.push(format!("{}: {}", unit(name), describe(&credentials_missing(name, host, f))));
                continue;
            }
        }
        let ports = registry
            .ensure(&origin_entry(name), PortMode::Auto, None, is_free)
            .and_then(|o| Ok((o, registry.ensure(&unit(name), PortMode::Auto, None, is_free)?)));
        match ports {
            Ok((origin_port, metrics_port)) => {
                planned.insert(
                    name.to_string(),
                    Planned { kind, origin_port, metrics_port, host, facts: named },
                );
            }
            Err(e) => warnings.push(format!("{}: {}", unit(name), describe(&e))),
        }
    }
    registry.entries.retain(|entry, _| {
        let site = entry.strip_prefix(ORIGIN_PREFIX).or_else(|| entry.strip_prefix(UNIT_PREFIX));
        site.is_none_or(|site| planned.contains_key(site))
    });
    planned
}

/// Each planned tunnel's yml.
pub fn render(paths: &Paths, tunnels: &BTreeMap<String, Planned>, rendered: &mut BTreeMap<PathBuf, String>) {
    for (site, t) in tunnels {
        let yml = match (&t.facts, &t.host) {
            (Some(f), Some(host)) => named_yml(t.origin(), &f.id, &f.credentials, host),
            _ => quick_yml(t.origin()),
        };
        rendered.insert(paths.tunnel_yml(site), yml);
    }
}

/// Each planned tunnel's unit, and the names of those whose yml `changed`: cloudflared does not
/// reload a config, so those restart.
pub fn units(
    paths: &Paths,
    brew: &Brew,
    home: &Path,
    tunnels: &BTreeMap<String, Planned>,
    changed: &BTreeSet<PathBuf>,
) -> (Vec<UnitSpec>, Vec<String>) {
    let mut specs = vec![];
    let mut restart = vec![];
    for (site, t) in tunnels {
        let name = unit(site);
        let yml = paths.tunnel_yml(site);
        if changed.contains(&yml) {
            restart.push(name.clone());
        }
        let cmd = provider().unit_command(brew, home, &yml, t.kind);
        specs.push(UnitSpec {
            pid_file: Some(paths.run_dir().join(format!("{name}.pid"))),
            name,
            program: cmd.program,
            args: cmd.args,
            env: cmd.env,
            health: Health::Tcp(t.metrics_port),
            ports: vec![(t.metrics_port, Proto::Tcp)],
            reload: Reload::Restart,
            stop: cmd.stop_signal,
            inherit: vec![],
            stop_timeout: None,
            start_timeout: Some(START_TIMEOUT),
        });
    }
    (specs, restart)
}
