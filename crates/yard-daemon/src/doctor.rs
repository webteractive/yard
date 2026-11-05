//! `yard doctor`: read-only checks, each non-Ok one with the command that fixes it.
//!
//! yardd runs them against its state. The CLI runs them in-process when yardd is down,
//! and then every check that needs config re-reads the TOML files itself.

use crate::daemon::VERSION;
use crate::emailulator::{OFFLINE_TIMEOUT, Probe, find_and_probe, is_exec, next_step, serving_agent};
use crate::helper_client::HelperClient;
use crate::reconcile::serves_mail;
use crate::sources::{self, BREW_TIMEOUT, TrustRead};
use crate::state::State;
use crate::tunnel;
use crate::{holder, portscan, services, tls};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use yard_core::brew::Brew;
use yard_core::config::{ServiceEntry, ServicesConfig, YardConfig, load_services, load_sites, load_yard};
use yard_core::constraint::{best_version, satisfies};
use yard_core::emailulator::{MAIL_HOST_LABEL, MIN_VERSION, NAME as EMAILULATOR, locate};
use yard_core::generated::{Hashes, sha256_hex};
use yard_core::node::{DEFAULT_SPEC, LtsMap, NodeSpec, NodeVersion, install_hint, pick};
use yard_core::paths::RESOLVER_MARKER;
use yard_core::ports::{DNSMASQ, DNSMASQ_PORT, NGINX_HTTP, NGINX_HTTPS, Registry};
use yard_core::project::{Loaded, Projects, ServiceWant};
use yard_core::render::overrides_for;
use yard_core::render::php::{ini_loads, is_stale};
use yard_core::services::brew_info;
use yard_core::services::render::{init_command, old_php_can_log_in};
use yard_core::services::{Engine, instance_name, plain_line};
use yard_core::sources::{tap_dir, tap_of};
use yard_core::tunnel::cloudflared::cert_path;
use yard_core::tunnel::facts::Facts;
use yard_core::{Paths, PhpVersion, sites};
use yard_proto::{
    Check, CheckStatus, DoctorReport, Holder, HolderKind, IgnoredBecause, Need, PortInfo, PortMode,
    PortStatus, SettingSource, SiteInfo, SiteKind, TunnelInfo, TunnelState, UnitInfo, UnitState, YardError,
};

/// How many of a failed unit's last log lines the `units` check quotes.
const UNIT_LINES: usize = 5;

pub struct DoctorInput<'a> {
    pub paths: &'a Paths,
    pub brew: &'a Brew,
    pub helper: &'a HelperClient,
    /// yardd's state; `None` when yardd is down.
    pub state: Option<&'a State>,
    pub units: Vec<UnitInfo>,
    pub daemon_running: bool,
    /// `/etc/resolver` outside tests.
    pub resolver_dir: &'a Path,
    /// `/usr/bin/security` outside tests.
    pub security: &'a Path,
    /// What the CLI's shell sets that brew reads (`DoctorParams::client_env`). Empty when
    /// doctor runs in the CLI itself: its environment is then yardd's too.
    pub client_env: BTreeMap<String, String>,
    /// The service instances asleep and since when (M4e); empty without yardd.
    pub sleeping: BTreeMap<String, OffsetDateTime>,
    /// The user's home, where emailulator's installer puts it (M4d).
    pub home: &'a Path,
    /// The tunnels yardd runs, as `tunnels.list` shows them; empty without yardd (M5).
    pub tunnels: Vec<TunnelInfo>,
    /// Whether a host still resolves (`DaemonOptions.resolve_host`).
    pub resolve_host: fn(&str) -> Option<bool>,
}

/// What the config-dependent checks read: yardd's last good snapshot, or the files.
/// A `None` field could not be worked out because a TOML file does not parse.
struct View {
    config_error: Option<YardError>,
    default_php: PhpVersion,
    tld: Option<String>,
    registry: Option<Registry>,
    /// The resolution warnings, and the sites.
    sites: Option<(Vec<String>, Vec<SiteInfo>)>,
    /// The `yard.toml` the sites were resolved with, and their projects' `.yard`.
    yard: Option<YardConfig>,
    projects: Projects,
    /// The service instances; `None` when `services.toml` does not parse.
    services: Option<ServicesConfig>,
    hashes: Result<Hashes, YardError>,
    /// What emailulator said about itself; `None` when it is not installed.
    emailulator: Option<Probe>,
}

impl View {
    fn from_state(state: &State) -> Option<View> {
        let snap = state.snapshot.as_ref()?;
        Some(View {
            config_error: None,
            default_php: snap.yard.default_php,
            tld: Some(snap.tld.clone()),
            registry: Some(snap.registry.clone()),
            sites: Some((snap.resolution.warnings.clone(), snap.resolution.sites.clone())),
            yard: Some(snap.yard.clone()),
            projects: snap.projects.clone(),
            services: Some(snap.services.clone()),
            hashes: Ok(state.hashes.clone()),
            emailulator: snap.emailulator.clone(),
        })
    }

    /// Asks emailulator as reconcile does, so the offline doctor sees what yardd would.
    async fn from_files(paths: &Paths, brew: &Brew, home: &Path) -> View {
        let recorded = load_services(paths).ok().and_then(|s| s.emailulator).and_then(|e| e.path);
        let emailulator = find_and_probe(recorded.as_deref(), home, None, OFFLINE_TIMEOUT).await;
        let (yard, yard_warnings) = match load_yard(paths) {
            Ok((y, w)) => (Ok(y), w),
            Err(e) => (Err(e), vec![]),
        };
        let sites_cfg = load_sites(paths);
        let registry = Registry::load(paths);
        let services = load_services(paths);
        let config_error =
            [yard.as_ref().err(), sites_cfg.as_ref().err(), registry.as_ref().err(), services.as_ref().err()]
                .into_iter()
                .flatten()
                .next()
                .cloned();
        let tld = match (&yard, &registry) {
            (Ok(y), Ok(_)) => Some(y.effective_tld().to_string()),
            _ => None,
        };
        let mut projects = Projects::new();
        let sites = match (&yard, &sites_cfg, &registry, &tld) {
            (Ok(y), Ok(s), Ok(r), Some(tld)) => {
                let mut warnings = yard_warnings.clone();
                let http = r.entries.get(NGINX_HTTP).map_or(r.profile.http_port(), |e| e.port);
                let https = r.entries.get(NGINX_HTTPS).map_or(r.profile.https_port(), |e| e.port);
                let ignored;
                (projects, ignored) = crate::project::read_projects(s, paths);
                let mut resolution =
                    sites::resolve(y, s, &projects, tld, http, https, &|p: &Path| p.is_dir());
                sites::mark_ignored(&mut resolution.sites, &ignored);
                // A services.toml that does not parse is `config_error`'s to report: reading it as
                // empty would call every `[services]` entry a missing instance.
                let services = services.as_ref().ok();
                let mail = emailulator.as_ref();
                crate::project::fill_needs(&mut resolution.sites, &projects, y, services, mail, brew, paths);
                warnings.extend(resolution.warnings);
                Some((warnings, resolution.sites))
            }
            _ => None,
        };
        View {
            config_error,
            default_php: yard.as_ref().map_or(PhpVersion::DEFAULT, |y| y.default_php),
            tld,
            registry: registry.ok(),
            sites,
            yard: yard.ok(),
            projects,
            services: services.ok(),
            hashes: Hashes::load(&paths.hashes()),
            emailulator,
        }
    }

    /// The hint for a check skipped because the config does not parse.
    fn config_hint(&self) -> String {
        self.config_error
            .as_ref()
            .and_then(|e| e.hint.clone())
            .unwrap_or_else(|| "fix the file the config check names".into())
    }
}

pub async fn run(input: DoctorInput<'_>) -> DoctorReport {
    let paths = input.paths;
    let brew = input.brew;
    let mut view = match input.state.and_then(View::from_state) {
        Some(view) => view,
        None => View::from_files(paths, brew, input.home).await,
    };
    // yardd's last failure covers more than a parse error: nginx rejecting the config, say.
    if let Some(e) = input.state.and_then(|s| s.last_error.clone()) {
        view.config_error = Some(e);
    }
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let mut checks = vec![];

    let brew_bin = brew.prefix.join("bin/brew");
    checks.push(if exists(&brew_bin) {
        ok("brew", format!("Homebrew is at {}", brew.prefix.display()))
    } else {
        fail(
            "brew",
            format!("Homebrew is not installed ({} is missing)", brew_bin.display()),
            r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#,
        )
    });
    for (name, bin) in [("nginx", brew.nginx_bin()), ("dnsmasq", brew.dnsmasq_bin())] {
        let id = format!("formula.{name}");
        checks.push(match brew.require_formula(name, &bin, &exists) {
            Ok(()) => ok(&id, format!("{name} is at {}", bin.display())),
            Err(e) => from_error(&id, &e),
        });
    }
    let v = view.default_php;
    checks.push(match brew.php_opt(v, &exists, &read_link) {
        Ok(opt) => ok("formula.php", format!("PHP {v}, the default, is at {}", opt.display())),
        Err(e) => from_error("formula.php", &e),
    });
    checks.extend(stale_brew_inis(brew, &brew.installed_php(&exists, &read_link)));

    checks.push(match &view.config_error {
        None => ok("config", "yard.toml, sites.toml and ports.toml parse"),
        Some(e) => from_error("config", e),
    });

    checks.push(if input.daemon_running {
        ok("daemon", "yardd is running")
    } else {
        fail("daemon", "yardd is not running", "yard start")
    });

    checks.push(match input.helper.status().await {
        Ok(s) if s.version == VERSION => ok("helper", format!("the helper {VERSION} is running")),
        Ok(s) => warn(
            "helper",
            format!("the helper is {}, but yard is {VERSION}", s.version),
            "sudo yard helper install",
        ),
        Err(e) => {
            let hint = e.hint.clone().unwrap_or_else(|| "sudo yard helper install".into());
            fail("helper", e.message, hint)
        }
    });

    checks.push(resolver(&view, input.resolver_dir, input.daemon_running));

    match &view.registry {
        Some(registry) => {
            let running = portscan::own_units(&input.units);
            let registry = registry.clone();
            let infos = tokio::task::spawn_blocking(move || portscan::infos(&registry, &running))
                .await
                .unwrap_or_default();
            checks.extend(ports(&infos));
        }
        None => checks.push(warn("ports", "not checked while ports.toml does not parse", view.config_hint())),
    }

    checks.push(match &view.hashes {
        Ok(hashes) => generated(paths, hashes),
        Err(e) => from_error("generated", e),
    });

    checks.push(match &view.sites {
        Some((warnings, _)) if !warnings.is_empty() => warn(
            "sites",
            warnings.join("; "),
            "rename or unlink what the warnings name; yard sites shows what is served",
        ),
        Some((_, sites)) => ok("sites", format!("{} sites resolve", sites.len())),
        None => warn("sites", "not checked while the configuration does not parse", view.config_hint()),
    });

    let secured = view.sites.as_ref().is_some_and(|(_, sites)| sites.iter().any(|s| s.secure));
    checks.push(ca(paths, secured, input.security).await);

    if let Some((_, sites)) = &view.sites {
        let installed = brew.installed_php(&exists, &read_link);
        checks.extend(sites.iter().filter_map(|site| php_constraint(site, &installed)));
        for site in sites {
            checks.extend(project_checks(site, view.yard.as_ref(), &view.projects));
        }
        checks.extend(managed_checks(paths, sites));
        let mut sites = sites.clone();
        crate::node::fill_node(&mut sites, paths);
        let default = view.yard.as_ref().and_then(|y| y.default_node.clone()).unwrap_or(DEFAULT_SPEC);
        checks.extend(node_checks(
            &sites,
            &default,
            &crate::node::installed(paths),
            &crate::node::lts(paths),
        ));
    }
    let sites = view.sites.as_ref().map_or(&[][..], |(_, sites)| sites.as_slice());
    if let Some(services) = &view.services {
        checks.extend(old_php_mysql_login(sites, &view.projects, services));
    }
    let trust = ask_trust(brew).await;
    if let Some(services) = &view.services {
        let ask = ServiceView {
            paths,
            brew,
            services,
            trust: &trust,
            units: &input.units,
            daemon_running: input.daemon_running,
            sleeping: &input.sleeping,
        };
        checks.extend(service_checks(&ask, sites, &view.projects).await);
    }
    checks.extend(emailulator_checks(&view, input.home));
    checks.extend(tunnel_checks(&input, view.yard.as_ref(), sites).await);
    checks.extend(
        sources_checks(brew, &trust, view.yard.as_ref(), view.services.as_ref(), &input.client_env).await,
    );
    checks.extend(units(&input.units, input.daemon_running));
    DoctorReport { checks }
}

/// Tunnels (tunnels spec §13): cloudflared, each domain's login, each tunnel's state, a named
/// tunnel's credentials, a tunnel Yard made that no site uses, the DNS records Yard left and a
/// tunnelled site's Vite `hot` file. Nothing is said unless a tunnel is configured or running,
/// or `state/tunnels.json` records something. A left record that no longer resolves is dropped
/// from that file by the running yardd only; the CLI's doctor never writes it.
async fn tunnel_checks(input: &DoctorInput<'_>, yard: Option<&YardConfig>, sites: &[SiteInfo]) -> Vec<Check> {
    let paths = input.paths;
    let sites_cfg = match input.state.and_then(|s| s.snapshot.as_ref()) {
        Some(snap) => Some(snap.sites_cfg.clone()),
        None => load_sites(paths).ok(),
    };
    let named: BTreeMap<String, String> = sites_cfg
        .iter()
        .flat_map(|cfg| cfg.sites.iter())
        .filter_map(|(site, entry)| entry.tunnel.as_ref().map(|t| (site.clone(), t.host.get_ref().clone())))
        .collect();
    // A broken file is said, not read as no tunnels: tunnel jobs refuse it too.
    let (facts, broken) = match Facts::load(&paths.tunnels_state()) {
        Ok(facts) => (facts, None),
        Err(e) => (Facts::default(), Some(e)),
    };
    if broken.is_none()
        && named.is_empty()
        && input.tunnels.is_empty()
        && facts.named.is_empty()
        && facts.dns_left.is_empty()
    {
        return vec![];
    }
    let mut checks = vec![];
    if let Some(e) = broken {
        checks.push(fail(
            "tunnel-state",
            e.message,
            format!("fix the JSON in {}", paths.tunnels_state().display()),
        ));
    }
    checks.push(if input.brew.cloudflared_bin().exists() {
        ok("cloudflared", "cloudflared is installed")
    } else {
        fail(
            "cloudflared",
            "cloudflared is not installed",
            format!("brew install {}", tunnel::cloudflared::FORMULA),
        )
    });
    for domain in yard.map_or(&[][..], |y| y.tunnel_domains.as_slice()) {
        if !cert_path(input.home, domain).exists() {
            let e = tunnel::cloudflared::not_logged_in(domain);
            checks.push(warn("tunnel-login", e.message, e.hint.unwrap_or_default()));
        }
    }
    for t in &input.tunnels {
        let id = format!("tunnel:{}", t.site);
        let at = t.url.as_deref().unwrap_or("no URL yet");
        match t.state {
            TunnelState::Ready => checks.push(ok(&id, format!("shared at {at}"))),
            TunnelState::Connecting => {
                let log = paths.log(&t.unit);
                checks.push(warn(&id, "connecting to Cloudflare", format!("tail -n 50 {}", log.display())))
            }
            TunnelState::Failed | TunnelState::Backoff => checks.push(fail(
                &id,
                t.last_error.clone().unwrap_or_else(|| format!("{} is not running", t.unit)),
                format!("yard restart {}", t.unit),
            )),
            TunnelState::Stopped | TunnelState::Other => {}
        }
    }
    for (site, host) in &named {
        if let Some(f) = facts.named.get(site).filter(|f| !f.credentials.exists()) {
            checks.push(from_error(
                &format!("tunnel-credentials:{site}"),
                &tunnel::credentials_missing(site, host, f),
            ));
        }
    }
    for (site, f) in &facts.named {
        if f.created_by_yard && !named.contains_key(site) {
            checks.push(warn(
                &format!("tunnel-leftover:{}", f.name),
                format!("the Cloudflare tunnel {} that Yard made for {site} is no longer used", f.name),
                format!("yard untunnel {site}"),
            ));
        }
    }
    checks.extend(left_records(input, &facts).await);
    let tunnelled: BTreeSet<&str> =
        input.tunnels.iter().map(|t| t.site.as_str()).chain(named.keys().map(String::as_str)).collect();
    for site in sites.iter().filter(|s| tunnelled.contains(&s.name.as_str())) {
        let SiteKind::Php { path, docroot, .. } = &site.kind else { continue };
        let hot = Path::new(path).join(docroot).join("hot");
        if hot.is_file() {
            checks.push(warn(
                &format!("tunnel-hot:{}", site.name),
                "Vite's dev server is running, so visitors get pages that load assets from this Mac",
                format!("stop npm run dev, or delete {}", hot.display()),
            ));
        }
    }
    checks
}

/// A warning per `dns_left` record that still resolves or could not be asked. With yardd running,
/// the records that no longer resolve are dropped from the facts file, read again just before
/// it is written so a tunnel job's change in between is kept.
async fn left_records(input: &DoctorInput<'_>, facts: &Facts) -> Vec<Check> {
    let resolve = input.resolve_host;
    let hosts: Vec<String> = facts.dns_left.iter().map(|l| l.host.clone()).collect();
    let answers =
        tokio::task::spawn_blocking(move || hosts.into_iter().map(|h| (resolve(&h), h)).collect::<Vec<_>>())
            .await
            .unwrap_or_default();
    let gone: Vec<&str> =
        answers.iter().filter(|(a, _)| *a == Some(false)).map(|(_, h)| h.as_str()).collect();
    if input.state.is_some() && !gone.is_empty() {
        let file = input.paths.tunnels_state();
        if let Ok(mut now) = Facts::load(&file) {
            now.dns_left.retain(|l| !gone.contains(&l.host.as_str()));
            if let Err(e) = now.save(&file) {
                tracing::warn!("cannot drop gone DNS records: {}", crate::describe(&e));
            }
        }
    }
    facts
        .dns_left
        .iter()
        .filter(|l| !gone.contains(&l.host.as_str()))
        .map(|l| {
            warn(
                &format!("tunnel-dns:{}", l.host),
                format!("the DNS record {} → {} is still there", l.host, l.target),
                format!("delete the CNAME {} in the Cloudflare dashboard", l.host),
            )
        })
        .collect()
}

/// Who catches mail (M4d spec §11): `emailulator`, with `emailulator-version`,
/// `emailulator-status` and `mail-host` warnings. Nothing is said unless a `.yard` asks for
/// emailulator or its LaunchAgent is installed: `[emailulator]` in `services.toml` alone stays
/// after `emailulator service uninstall`, which no Yard command undoes.
fn emailulator_checks(view: &View, home: &Path) -> Vec<Check> {
    let recorded = view.services.as_ref().and_then(|s| s.emailulator.as_ref());
    let listed = view.projects.values().any(|loaded| loaded.ok().is_some_and(|file| file.emailulator));
    let agent = view.emailulator.as_ref().and_then(|p| p.agent.as_ref()).filter(|a| a.installed);
    if !listed && agent.is_none() {
        return vec![];
    }
    let Some(probe) = &view.emailulator else {
        let path = recorded.and_then(|e| e.path.as_deref());
        return locate(path, home, &is_exec).err().map(|e| from_error(EMAILULATOR, &e)).into_iter().collect();
    };
    let mut checks = vec![];
    let old = probe.version.filter(|v| *v < MIN_VERSION);
    if let Some((major, minor, patch)) = old {
        let (a, b, c) = MIN_VERSION;
        checks.push(warn(
            "emailulator-version",
            format!("emailulator {major}.{minor}.{patch} is older than {a}.{b}.{c}"),
            "emailulator self-update",
        ));
    }
    if let Some(why) = &probe.status_error {
        let message = format!("could not ask emailulator: {why}");
        checks.push(warn("emailulator-status", message, "emailulator service status"));
        if probe.agent.is_none() {
            // Nothing known of its agent: that warning is all there is to say, not that the
            // agent is missing.
            return checks;
        }
    }
    let mail_site = view.sites.as_ref().is_some_and(|(_, sites)| !serves_mail(sites));
    // A site's .yard asking for it makes a broken emailulator Yard's failure; the agent alone,
    // with nothing in Yard using it, is a warning.
    let broken = |message: &str, hint: &str| match listed {
        true => fail(EMAILULATOR, message, hint),
        false => warn(EMAILULATOR, message, hint),
    };
    checks.push(match serving_agent(Some(probe)) {
        Some(agent) if agent.answering => {
            let who = match agent.pid {
                Some(pid) => format!("LaunchAgent {} (pid {pid})", agent.label),
                None => format!("LaunchAgent {}", agent.label),
            };
            let ui =
                mail_url(view).filter(|_| !mail_site).unwrap_or_else(|| format!("http://{}", agent.http));
            ok(EMAILULATOR, format!("mail is caught by {who}: SMTP {}, web UI {ui}", agent.smtp))
        }
        Some(_) => broken("emailulator is not answering", "emailulator service status"),
        None if old.is_none() && agent.is_none() => {
            broken("emailulator's LaunchAgent is not installed", next_step(Some(probe)))
        }
        None => broken("emailulator is not answering", next_step(Some(probe))),
    });
    if mail_site && serving_agent(Some(probe)).is_some() {
        let tld = view.tld.as_deref().unwrap_or("yard");
        checks.push(warn(
            "mail-host",
            format!("the site {MAIL_HOST_LABEL} is served at {MAIL_HOST_LABEL}.{tld}, so emailulator's web UI is not"),
            format!("yard unlink {MAIL_HOST_LABEL}"),
        ));
    }
    checks
}

/// `mail.<tld>` with nginx's port, as `services.list` gives it.
fn mail_url(view: &View) -> Option<String> {
    let (tld, registry) = (view.tld.as_deref()?, view.registry.as_ref()?);
    let port_of = |unit: &str, usual: u16| registry.entries.get(unit).map_or(usual, |e| e.port);
    let http = port_of(NGINX_HTTP, registry.profile.http_port());
    let https = port_of(NGINX_HTTPS, registry.profile.https_port());
    let secure = view.yard.as_ref().is_some_and(|y| y.secure_by_default);
    Some(services::host_url(MAIL_HOST_LABEL, tld, secure, http, https))
}

/// A site's Node that cannot run: `node-spec:<site>` for a pin that cannot be read,
/// `node:<site>` for a spec nothing installed meets, and `node-default` once when the
/// default is not installed and a site runs it. Sites without Node get nothing, and
/// nothing reports Ok, so a machine without Node sees the same report as before.
fn node_checks(
    sites: &[SiteInfo],
    default: &NodeSpec,
    installed: &[NodeVersion],
    lts: &LtsMap,
) -> Vec<Check> {
    let mut checks = vec![];
    let mut on_default = vec![];
    for site in sites {
        let Some(node) = &site.node else { continue };
        if let Some(e) = &node.error {
            let hint = e.hint.clone().unwrap_or_else(|| "yard info shows the site's Node".into());
            checks.push(warn(&format!("node-spec:{}", site.name), e.message.clone(), hint));
            continue;
        }
        if node.source == SettingSource::Default {
            on_default.push(site.name.as_str());
        }
        let Some(spec) = &node.spec else { continue };
        if node.version.is_none() {
            let from = match (&node.file, node.source) {
                (Some(file), _) => file.clone(),
                (None, SettingSource::Default) => "the default".to_string(),
                (None, _) => "yard node use".to_string(),
            };
            checks.push(warn(
                &format!("node:{}", site.name),
                format!("{} needs Node {spec} ({from}), which is not installed", site.name),
                install_hint(&spec),
            ));
        }
    }
    if !on_default.is_empty() && pick(default, installed, lts).is_none() {
        checks.push(warn(
            "node-default",
            format!("the default Node ({default}) is not installed, and {} use it", on_default.join(", ")),
            install_hint(&default),
        ));
    }
    checks
}

const FIREFOX: &str = "/Applications/Firefox.app";

/// Whether browsers trust the CA secured sites use. Asks `security` only once a CA
/// exists, so a machine with no secured site never runs it.
async fn ca(paths: &Paths, secured: bool, security: &Path) -> Check {
    let cert = paths.ca_cert();
    let trusted = if cert.exists() { tls::trusted(security, &cert).await } else { Some(false) };
    let Some(trusted) = trusted else {
        return warn(
            "ca",
            format!(
                "cannot tell whether the Yard CA is trusted: security did not answer within {} s",
                tls::SECURITY_TIMEOUT.as_secs()
            ),
            "yard doctor",
        );
    };
    if trusted {
        let mut message = format!(
            "the Yard CA is trusted in your login keychain; for Node: NODE_EXTRA_CA_CERTS={}",
            cert.display()
        );
        if Path::new(FIREFOX).exists() {
            message.push_str("; Firefox: set security.enterprise_roots.enabled to true in about:config");
        }
        return ok("ca", message);
    }
    if secured {
        warn("ca", "browsers will warn on https sites: the Yard CA is not trusted", "yard trust")
    } else {
        ok("ca", "no site uses HTTPS")
    }
}

/// The largest `composer.json` doctor reads.
const COMPOSER_JSON_MAX: u64 = 1 << 20;

/// A project's `composer.json` if it is a regular file of sane size.
fn read_composer_json(file: &Path) -> Option<String> {
    crate::project::read_small(file, COMPOSER_JSON_MAX)
}

/// A warning when a site's `composer.json` `require.php` rules out the version it runs
/// (spec §4.5: advisory only). Nothing for a constraint Yard cannot read.
fn php_constraint(site: &SiteInfo, installed: &[PhpVersion]) -> Option<Check> {
    let SiteKind::Php { path, php, .. } = &site.kind else { return None };
    let text = read_composer_json(&Path::new(path).join("composer.json"))?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let wanted = json.get("require")?.get("php")?.as_str()?;
    let runs: PhpVersion = php.parse().ok()?;
    if satisfies(wanted, runs)? {
        return None;
    }
    let hint = match best_version(wanted, installed) {
        Some((v, true)) => format!("yard isolate {v} --site {}", site.name),
        Some((v, false)) => format!("yard php install {v}"),
        None => format!("no PHP Yard supports (7.0–8.5) satisfies {wanted}; change composer.json"),
    };
    let id = format!("php-constraint:{}", site.name);
    Some(warn(&id, format!("{} wants php {wanted} (composer.json) but runs {runs}", site.name), hint))
}

/// `brew-ini:<v>/<file>` warns about an ini in brew's conf.d that loads a `.so` by a path that
/// is gone: `brew uninstall` of an extension leaves its ini, and brew keeps a user's edited
/// ini, writing its current one beside it as `<file>.default`. Yard skips such a file, but
/// brew's own `php` warns about it on every run, and nothing else ever removes it.
fn stale_brew_inis(brew: &Brew, installed: &[PhpVersion]) -> Vec<Check> {
    let exists = |p: &Path| p.exists();
    let mut checks = vec![];
    for v in installed {
        let dir = brew.php_conf_d(*v);
        for (name, text) in crate::reconcile::ini_files(&dir) {
            let Some((module, gone)) =
                ini_loads(&text).find(|(_, value)| value.starts_with('/') && !exists(Path::new(value)))
            else {
                continue;
            };
            let file = dir.join(&name);
            let current = dir.join(format!("{name}.default"));
            let replaced = std::fs::read_to_string(&current)
                .is_ok_and(|t| ini_loads(&t).any(|(m, _)| m == module) && !is_stale(&t, &exists));
            let (message, hint) = if replaced {
                (
                    format!(
                        "PHP {v}'s {name} loads {gone}, which is gone, so {module} does not load; \
                         brew's current ini is {name}.default beside it"
                    ),
                    format!("mv {} {}", current.display(), file.display()),
                )
            } else {
                (
                    format!(
                        "PHP {v}'s {name} loads {gone}, which is gone (brew uninstall leaves the ini); \
                         brew's own php warns about it on every run"
                    ),
                    format!("rm {}", file.display()),
                )
            };
            checks.push(warn(&format!("brew-ini:{v}/{name}"), message, hint));
        }
    }
    checks
}

/// `project-file:<site>` for a site served from a `.yard`: fails while the file does not
/// parse or asks for what is not installed, warns when it lists an extension switched off
/// in `yard.toml`. `yard-php-ignored:<site>` warns that a `.yard-php` beside it is unused.
fn project_checks(site: &SiteInfo, yard: Option<&YardConfig>, projects: &Projects) -> Vec<Check> {
    let (Some(file), SiteKind::Php { path, php, .. }) = (&site.project_file, &site.kind) else {
        return vec![];
    };
    let id = format!("project-file:{}", site.name);
    let off: Vec<&str> =
        match (projects.get(&site.name).and_then(Loaded::ok), yard, php.parse::<PhpVersion>()) {
            (Some(project), Some(yard), Ok(v)) => project
                .extensions
                .iter()
                .filter(|e| {
                    yard.extensions
                        .get(&v)
                        .and_then(|exts| exts.iter().find(|(k, _)| k.eq_ignore_ascii_case(&e.module)))
                        .is_some_and(|(_, entry)| !entry.enabled)
                })
                .map(|e| e.module.as_str())
                .collect(),
            _ => vec![],
        };
    let mut checks = vec![if let Some(e) = &site.project_error {
        from_error(&id, e)
    } else if !site.needs.is_empty() {
        let message = format!("{} needs {}", site.name, Need::describe_all(&site.needs));
        fail(&id, message, format!("cd {path} && yard init"))
    } else if let Some(first) = off.first() {
        let message = format!("{} lists {}, which is off for PHP {php}", site.name, off.join(", "));
        warn(&id, message, format!("yard ext enable {first} --php {php}"))
    } else {
        ok(&id, format!("{} is served as {file} says", site.name))
    }];
    let yard_php = Path::new(path).join(".yard-php");
    if yard_php.exists() {
        checks.push(warn(
            &format!("yard-php-ignored:{}", site.name),
            format!("{} has a .yard, so {} is ignored", site.name, yard_php.display()),
            format!("delete {}", yard_php.display()),
        ));
    }
    checks
}

/// The managed `.yard` files that are not used (managed-file spec §6):
/// `managed-file-ignored:<site>` warns of one a committed `.yard` hides,
/// `managed-file-mismatch:<name>` of one written for another directory, and
/// `managed-file-orphan:<name>`, information only, of one no linked project can use.
fn managed_checks(paths: &Paths, sites: &[SiteInfo]) -> Vec<Check> {
    let mut checks = vec![];
    for site in sites {
        let (Some(ignored), SiteKind::Php { path, .. }) = (&site.ignored_managed_file, &site.kind) else {
            continue;
        };
        let file = &ignored.file;
        checks.push(match ignored.because {
            IgnoredBecause::Committed => warn(
                &format!("managed-file-ignored:{}", site.name),
                format!("{} has a committed .yard, so {file} is ignored", site.name),
                format!("rm {file}"),
            ),
            IgnoredBecause::OtherProject => warn(
                &format!("managed-file-mismatch:{}", site.name),
                format!("{file} is for {}, not {path}", ignored.project.as_deref().unwrap_or("no project")),
                format!("cd {path} && yard init --replace"),
            ),
        });
    }
    let linked = |name: &str| sites.iter().any(|s| s.name == name && matches!(s.kind, SiteKind::Php { .. }));
    let Ok(dirs) = std::fs::read_dir(paths.managed_sites_dir()) else { return checks };
    let mut orphans: Vec<String> = dirs
        .flatten()
        .filter_map(|d| d.file_name().into_string().ok())
        .filter(|name| !linked(name) && paths.managed_project_file(name).symlink_metadata().is_ok())
        .collect();
    orphans.sort();
    for name in orphans {
        let file = paths.managed_project_file(&name);
        // `yard link` links the directory it runs in: the one the file was written for.
        let project = crate::project::read_project_file(&file)
            .and_then(Result::ok)
            .and_then(|text| yard_core::project::parse_managed(&text, &file).ok()?.1);
        let hint = match project {
            Some(dir) => format!("cd {} && yard link {name}", dir.display()),
            None => format!("yard link {name}"),
        };
        checks.push(Check {
            id: format!("managed-file-orphan:{name}"),
            status: CheckStatus::Ok,
            message: format!("{} belongs to no linked site", file.display()),
            hint: Some(hint),
        });
    }
    checks
}

// ---- Service instances (spec M4b §10) ----

/// What the service checks read.
struct ServiceView<'a> {
    paths: &'a Paths,
    brew: &'a Brew,
    services: &'a ServicesConfig,
    trust: &'a Trust,
    units: &'a [UnitInfo],
    daemon_running: bool,
    sleeping: &'a BTreeMap<String, OffsetDateTime>,
}

/// One group per instance (`service:<i>`, `service-data:<i>`, `service-line:<i>`), one
/// `service-need:<site>` for each site whose `.yard` asks for an instance that is not
/// running, and `service-deprecated:<i>` from one `brew info` for every instance's formula.
/// A check with nothing to say is left out, as `php-constraint` is.
async fn service_checks(view: &ServiceView<'_>, sites: &[SiteInfo], projects: &Projects) -> Vec<Check> {
    let mut checks = vec![];
    for (instance, entry) in &view.services.services {
        checks.extend(instance_state(view, instance, entry));
        checks.extend(instance_data(view.paths, instance, entry));
        checks.extend(instance_line(view.brew, instance, entry));
    }
    for site in sites {
        checks.extend(site_needs(view, site, projects));
    }
    checks.extend(deprecated(view.brew, view.services, view.trust).await);
    checks
}

/// Whether the instance runs, from yardd's units. Without yardd only what the file says
/// is known: that the user stopped it.
fn instance_state(view: &ServiceView<'_>, instance: &str, entry: &ServiceEntry) -> Option<Check> {
    let id = format!("service:{instance}");
    if entry.stopped {
        return Some(ok(&id, format!("{instance} is stopped; you stopped it")));
    }
    // Knowable without yardd, and the reason reconcile leaves an instance out.
    let program =
        services::unit_command(view.brew, view.paths, entry.engine, instance, &entry.formula, &[]).program;
    if let Some(missing) = services::missing_program(view.brew, &entry.formula, &program) {
        let hint = missing.hint.unwrap_or_default();
        return Some(warn(&id, format!("{instance} cannot run: {}", missing.message), hint));
    }
    if !view.daemon_running {
        return None;
    }
    if let Some(since) = view.sleeping.get(instance) {
        return Some(ok(
            &id,
            format!("{instance} is sleeping since {}; it wakes on use", local_time(*since)),
        ));
    }
    let unit = view.units.iter().find(|u| u.name == instance);
    Some(match unit.map(|u| &u.state) {
        Some(UnitState::Running) => ok(&id, format!("{instance} is running")),
        Some(UnitState::Starting) => ok(&id, format!("{instance} is starting")),
        Some(UnitState::Backoff { .. }) => warn(
            &id,
            format!("{instance} stopped and yardd is about to start it again"),
            format!("yard service logs {instance}"),
        ),
        Some(UnitState::Failed { last_lines }) => {
            let unit = unit?;
            fail(&id, failure(unit, last_lines), format!("yard service restart {instance}"))
        }
        Some(UnitState::Stopped) => {
            warn(&id, format!("{instance} is not running"), format!("yard service restart {instance}"))
        }
        None => warn(
            &id,
            format!("yardd runs no {instance}: reconcile left it out"),
            "yard ports check shows a port it could not have",
        ),
    })
}

/// `14:02` in the machine's time zone. The `time` crate refuses the local offset in a
/// process with threads, so this asks libc, whose `localtime_r` is safe in one.
fn local_time(time: OffsetDateTime) -> String {
    let secs: libc::time_t = time.unix_timestamp();
    // SAFETY: `tm` is plain data that `localtime_r` fills; both pointers are valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return format!("{:02}:{:02} UTC", time.hour(), time.minute());
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// `services/<i>/data/` for the engines whose init makes it. Yard never re-initializes
/// data on its own. Redis's directory is made by reconcile and may be empty, and MinIO
/// makes its own, so neither is checked.
fn instance_data(paths: &Paths, instance: &str, entry: &ServiceEntry) -> Option<Check> {
    init_command(entry.engine, instance, Path::new(""), paths, "")?;
    let id = format!("service-data:{instance}");
    let data = paths.service_data(instance);
    let problem = match std::fs::read_dir(&data) {
        Ok(mut files) => files.next().is_none().then(|| format!("{} is empty", data.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(format!("{} is missing", data.display())),
        Err(e) => Some(format!("cannot read {}: {e}", data.display())),
    };
    Some(match problem {
        None => ok(&id, format!("{instance} has its data")),
        Some(problem) => {
            let add = Need::Service { engine: entry.engine.key().into(), version: entry.version.clone() };
            fail(
                &id,
                format!("{instance} has no data: {problem}, and Yard never re-initializes it on its own"),
                format!("yard service remove {instance} && {}", add.command()),
            )
        }
    })
}

/// An instance on the plain formula (`mysql-9.7` on `mysql`) whose keg has moved to a
/// newer line than its name says (spec §5): `brew upgrade mysql` did it, and the engine
/// upgrades the instance's data in place.
fn instance_line(brew: &Brew, instance: &str, entry: &ServiceEntry) -> Option<Check> {
    let engine = entry.engine;
    if !engine.versioned() || entry.formula != engine.base_formula() {
        return None;
    }
    let line = entry.version.as_deref()?;
    let keg = std::fs::read_link(brew.prefix.join("opt").join(&entry.formula)).ok()?;
    let now = plain_line(engine, &keg.file_name()?.to_string_lossy())?;
    if now == line {
        return None;
    }
    let hint = match engine.default_line() {
        Some(default) => format!("yard service add {} --version {default}", engine.key()),
        None => format!("brew info {}", entry.formula),
    };
    Some(warn(
        &format!("service-line:{instance}"),
        format!(
            "{instance} now runs {} {now}: brew upgrade moved {} past {line}, and {} upgrades an instance's data in place and cannot go back",
            engine.display(),
            entry.formula,
            engine.display(),
        ),
        hint,
    ))
}

/// The `.yard` `[services]` entries of a site, whose instance is stopped, failed or not
/// added. A missing instance already fails the site's `project-file` check; this says
/// which one and how to add it.
fn site_needs(view: &ServiceView<'_>, site: &SiteInfo, projects: &Projects) -> Vec<Check> {
    let Some(project) = projects.get(&site.name).and_then(Loaded::ok) else { return vec![] };
    let id = format!("service-need:{}", site.name);
    let mut checks = vec![];
    for (engine, want) in &project.services {
        let (instance, entry) = match want {
            ServiceWant::Line(line) => {
                let name = instance_name(*engine, Some(line));
                let entry = view.services.services.get(&name);
                (name, entry)
            }
            ServiceWant::Default => match services::default_instance(view.services, *engine) {
                Some(name) => (name.to_string(), view.services.services.get(name)),
                None => (instance_name(*engine, None), None),
            },
        };
        let asks = format!("{}'s .yard asks for {instance}", site.name);
        let failed = view.daemon_running
            && view.units.iter().any(|u| u.name == instance && matches!(u.state, UnitState::Failed { .. }));
        match entry {
            None => {
                let version = match want {
                    ServiceWant::Line(line) => Some(line.clone()),
                    ServiceWant::Default => None,
                };
                let add = Need::Service { engine: engine.key().into(), version };
                checks.push(warn(&id, format!("{asks}, which is not added"), add.command()));
            }
            Some(entry) if entry.stopped => checks.push(warn(
                &id,
                format!("{asks}, which is stopped"),
                format!("yard service start {instance}"),
            )),
            Some(_) if failed => checks.push(warn(
                &id,
                format!("{asks}, which has failed"),
                format!("yard service restart {instance}"),
            )),
            Some(_) => {}
        }
    }
    checks
}

/// `php-mysql-login:<site>`: PHP 7.0 to 7.3's mysqlnd logs in only with
/// `mysql_native_password`, which Yard turns on for MySQL 8 and MySQL 9 no longer has, so such
/// a site whose `.yard` asks for a MySQL 9 instance cannot connect (M4b spec §16).
fn old_php_mysql_login(sites: &[SiteInfo], projects: &Projects, services: &ServicesConfig) -> Vec<Check> {
    sites
        .iter()
        .filter_map(|site| {
            let SiteKind::Php { php, .. } = &site.kind else { return None };
            let version = php.parse::<PhpVersion>().ok()?;
            if version >= (PhpVersion { major: 7, minor: 4 }) {
                return None;
            }
            let project = projects.get(&site.name).and_then(Loaded::ok)?;
            let instance = match project.services.get(&Engine::Mysql)? {
                ServiceWant::Line(line) => instance_name(Engine::Mysql, Some(line)),
                ServiceWant::Default => services::default_instance(services, Engine::Mysql)?.to_string(),
            };
            if old_php_can_log_in(&instance) {
                return None;
            }
            let file = site.project_file.as_deref().unwrap_or(".yard");
            Some(warn(
                &format!("php-mysql-login:{}", site.name),
                format!(
                    "{} runs PHP {php}, which logs in to MySQL only with mysql_native_password, \
                     and {instance} has none (MySQL 9 removed it)",
                    site.name
                ),
                format!("use mysql = \"8.4\" in {file}"),
            ))
        })
        .collect()
}

/// `service-deprecated:<i>` for each instance whose formula brew has deprecated or
/// disabled, from one `brew info --json=v2` for the formulae it would answer for (`askable`).
/// A brew that cannot answer is a warning, never an error.
async fn deprecated(brew: &Brew, services: &ServicesConfig, trust: &Trust) -> Vec<Check> {
    let mut formulae: Vec<&str> = vec![];
    for entry in services.services.values() {
        if !formulae.contains(&entry.formula.as_str()) && askable(brew, trust, &entry.formula) {
            formulae.push(&entry.formula);
        }
    }
    // With no brew the `brew` check already fails; a formula brew would refuse is left out.
    if formulae.is_empty() || !brew.brew_bin().exists() {
        return vec![];
    }
    let mut args = vec!["info", "--json=v2"];
    args.extend(&formulae);
    let cannot = |why: String| {
        vec![warn(
            "service-deprecated",
            format!("cannot ask brew whether the service formulae are deprecated: {why}"),
            format!("brew info {}", formulae.join(" ")),
        )]
    };
    let answer = match tokio::time::timeout(BREW_TIMEOUT, sources::brew_stdout(brew, &args)).await {
        Err(_) => return cannot(format!("brew did not answer within {} s", BREW_TIMEOUT.as_secs())),
        Ok(Err(e)) => return cannot(e.message),
        Ok(Ok(answer)) => answer,
    };
    let (true, out, _) = answer else {
        let tail = answer.2.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it printed nothing");
        return cannot(tail.trim().to_string());
    };
    let Ok(described) = brew_info::parse(&out) else { return cannot("its answer did not parse".into()) };
    services
        .services
        .iter()
        .filter_map(|(instance, entry)| {
            let f = described.iter().find(|f| {
                f.full_name == entry.formula || f.name == entry.formula || f.aliases.contains(&entry.formula)
            })?;
            let what = if f.disabled {
                match &f.disable_date {
                    Some(date) => format!("which brew has disabled (since {date})"),
                    None => "which brew has disabled".to_string(),
                }
            } else if f.deprecated || f.disable_date.is_some() {
                match &f.disable_date {
                    Some(date) => format!("which brew has deprecated and disables on {date}"),
                    None => "which brew has deprecated, with no date set to disable it".to_string(),
                }
            } else {
                return None;
            };
            let hint = match entry.engine.default_line() {
                Some(line) => format!("yard service add {} --version {line}", entry.engine.key()),
                None => format!("brew info {}", entry.formula),
            };
            Some(warn(
                &format!("service-deprecated:{instance}"),
                format!("{instance} runs {}, {what}", entry.formula),
                hint,
            ))
        })
        .collect()
}

// ---- Brew sources (spec M4b §8) ----

/// What brew said about trust, asked once per doctor run and shared by the checks.
enum Trust {
    /// There is no brew to ask.
    NoBrew,
    Read(TrustRead),
    /// Why brew could not be asked.
    Unreadable(String),
}

async fn ask_trust(brew: &Brew) -> Trust {
    if !brew.brew_bin().exists() {
        return Trust::NoBrew;
    }
    match tokio::time::timeout(BREW_TIMEOUT, sources::read_trust_state(brew)).await {
        Ok(Ok(read)) => Trust::Read(read),
        Ok(Err(e)) => Trust::Unreadable(e.message),
        Err(_) => Trust::Unreadable(format!("brew did not answer within {} s", BREW_TIMEOUT.as_secs())),
    }
}

/// Whether `brew info` may name `formula`. Brew 6 refuses the whole call when any formula is
/// from a tap it does not trust, and `info <tap>/<formula>` may tap the tap, which a
/// read-only doctor must not do. So a formula is named only when it is homebrew/core's, or its
/// tap is already tapped and brew would load it (it is trusted, or brew predates trust).
/// When trust could not be read only homebrew/core's are named.
fn askable(brew: &Brew, trust: &Trust, formula: &str) -> bool {
    if tap_of(formula).is_none() {
        return true;
    }
    let loads = match trust {
        Trust::Read(TrustRead::Store(store)) => store.trusts(formula),
        Trust::Read(TrustRead::Predates { .. }) => true,
        Trust::NoBrew | Trust::Unreadable(_) => false,
    };
    let tapped = tap_dir(formula).is_some_and(|dir| brew.prefix.join("Library/Taps").join(dir).is_dir());
    loads && tapped
}

/// `sources:brew` (brew older than 6.0, with no tap trust), `sources:trust` (an installed
/// formula from a Yard tap that brew does not trust) and `sources:env` (settings of the
/// CLI's shell that brew reads and yardd's environment lacks). Brew is only ever asked
/// questions: doctor never installs, trusts or runs `brew services`.
async fn sources_checks(
    brew: &Brew,
    trust: &Trust,
    yard: Option<&YardConfig>,
    services: Option<&ServicesConfig>,
    client_env: &BTreeMap<String, String>,
) -> Vec<Check> {
    let mut checks = vec![];
    match trust {
        Trust::NoBrew => {}
        Trust::Read(TrustRead::Predates { major }) => checks.push(warn(
            "sources:brew",
            format!("Homebrew {major} predates tap trust (6.0), so Yard cannot gate installs from third-party taps"),
            "brew update",
        )),
        Trust::Read(TrustRead::Store(store)) => {
            // Without a readable yard.toml the extensions brew installed are not known: the
            // config check says so.
            if let Some(yard) = yard {
                for formula in sources::installed(brew, yard, services).await {
                    if !store.trusts(&formula) {
                        checks.push(warn(
                            "sources:trust",
                            format!(
                                "{formula} is installed but brew does not trust it, so brew will refuse to install, upgrade or remove it"
                            ),
                            format!("yard source trust {formula}"),
                        ));
                    }
                }
            }
        }
        Trust::Unreadable(why) => checks.push(cannot_ask_trust(why)),
    }
    let own = |key: &str| std::env::var(key).ok();
    let differing = env_differences(client_env, &own);
    if !differing.is_empty() {
        checks.push(warn(
            "sources:env",
            format!(
                "your shell sets {}, which yardd's environment lacks or sets differently, so the brew Yard runs does not see it",
                differing.join(", ")
            ),
            "put it in ~/.homebrew/brew.env",
        ));
    }
    checks
}

fn cannot_ask_trust(why: &str) -> Check {
    warn("sources:trust", format!("cannot ask brew what it trusts: {why}"), "brew trust --json=v1 shows why")
}

/// The brew settings in `client_env` (`XDG_CONFIG_HOME`, which moves brew's trust file, and
/// the `HOMEBREW_FORBIDDEN_*` policies) that `own` does not have, or has differently.
/// Names only: a value is never printed.
fn env_differences<'a>(
    client_env: &'a BTreeMap<String, String>,
    own: &dyn Fn(&str) -> Option<String>,
) -> Vec<&'a str> {
    client_env
        .iter()
        .filter(|(key, _)| *key == "XDG_CONFIG_HOME" || key.starts_with("HOMEBREW_FORBIDDEN_"))
        .filter(|(key, value)| own(key).as_deref() != Some(value.as_str()))
        .map(|(key, _)| key.as_str())
        .collect()
}

/// 1 when any check failed; warnings alone still exit 0.
pub fn exit_code(report: &DoctorReport) -> i32 {
    i32::from(report.checks.iter().any(|c| c.status == CheckStatus::Fail))
}

fn resolver(view: &View, dir: &Path, daemon_running: bool) -> Check {
    let Some(tld) = &view.tld else {
        return warn("resolver", "not checked while the configuration does not parse", view.config_hint());
    };
    let dns = view.registry.as_ref().and_then(|r| r.entries.get(DNSMASQ)).map_or(DNSMASQ_PORT, |e| e.port);
    let file = dir.join(tld);
    // yardd installs the resolver on start, and again after a restart.
    let reinstall = if daemon_running { "yard stop && yard start" } else { "yard start" };
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return fail(
                "resolver",
                format!("{} is missing, so .{tld} names do not resolve", file.display()),
                reinstall,
            );
        }
        Err(e) => {
            return fail(
                "resolver",
                format!("cannot read {}: {e}", file.display()),
                format!("ls -l {}", file.display()),
            );
        }
    };
    if text.lines().next() != Some(RESOLVER_MARKER) {
        return fail(
            "resolver",
            format!("{} belongs to another tool, so .{tld} names do not reach Yard", file.display()),
            format!(
                "set `tld` in yard.toml to another name, or remove {} if nothing uses it",
                file.display()
            ),
        );
    }
    let port = text.lines().find_map(|l| l.strip_prefix("port ")).and_then(|p| p.trim().parse::<u16>().ok());
    if port != Some(dns) {
        let found = port.map_or("no port".to_string(), |p| format!("port {p}"));
        return warn(
            "resolver",
            format!("{} sends .{tld} to {found}, but dnsmasq listens on {dns}", file.display()),
            reinstall,
        );
    }
    ok("resolver", format!("{} sends .{tld} to dnsmasq on 127.0.0.1:{dns}", file.display()))
}

/// A Fail per held pinned or preferred port, a Warn per external port nothing listens on.
/// An `auto` port that is taken moves on the next reconcile, so it is not reported.
fn ports(infos: &[PortInfo]) -> Vec<Check> {
    let mut checks = vec![];
    for info in infos {
        let unit = &info.unit;
        let port = info.port;
        match (info.status, info.mode) {
            (PortStatus::InUse, PortMode::Pinned | PortMode::Preferred) => {
                let mode = yard_core::ports::mode_str(info.mode);
                let holder = holder::label(info.holder.as_ref());
                let stop = match &info.holder {
                    Some(h) => stop_command(h, port),
                    None => format!("sudo lsof -nP -iTCP:{port} -sTCP:LISTEN names it"),
                };
                checks.push(fail(
                    "ports",
                    format!("port {port} ({unit}, {mode}) is held by {holder}"),
                    format!("{stop}, or move {unit} with yard ports set {unit} <port>"),
                ));
            }
            (PortStatus::ExternalNotListening, _) => {
                let name = unit.strip_prefix("proxy:").unwrap_or(unit);
                checks.push(warn(
                    "ports",
                    format!("nothing listens on port {port}, which {unit} proxies to"),
                    format!("start the app on port {port}, or yard unproxy {name}"),
                ));
            }
            _ => {}
        }
    }
    if checks.is_empty() {
        checks.push(ok("ports", format!("{} registered ports are free or Yard's", infos.len())));
    }
    checks
}

fn stop_command(h: &Holder, port: u16) -> String {
    match h.kind {
        HolderKind::Herd => "herd stop".into(),
        HolderKind::BrewServices => "brew services stop <formula> (brew services list names it)".into(),
        HolderKind::Docker => format!("docker ps --filter publish={port} names the container to stop"),
        HolderKind::Dbngin => "quit DBngin, or stop the server it runs on that port".into(),
        HolderKind::PostgresApp => "quit Postgres.app, or stop its server".into(),
        HolderKind::Yard => format!("stop the Yard that runs pid {}", h.pid),
        HolderKind::Unknown => format!("kill {}", h.pid),
    }
}

/// Generated files whose content no longer matches the hash Yard stored when it wrote them.
fn generated(paths: &Paths, hashes: &Hashes) -> Check {
    let edited: Vec<&PathBuf> = hashes
        .0
        .iter()
        .filter(|(path, stored)| {
            std::fs::read_to_string(path).is_ok_and(|text| sha256_hex(&text) != **stored)
        })
        .map(|(path, _)| path)
        .collect();
    if edited.is_empty() {
        return ok("generated", format!("{} generated files are as Yard wrote them", hashes.0.len()));
    }
    let list: Vec<String> = edited
        .iter()
        .map(|path| match overrides_for(paths, path) {
            Some(o) => format!("{} (edit {} instead)", path.display(), o.display()),
            None => path.display().to_string(),
        })
        .collect();
    let files: Vec<String> = edited.iter().map(|p| format!("{:?}", p.display().to_string())).collect();
    warn(
        "generated",
        format!("edited by hand, so Yard no longer updates them: {}", list.join("; ")),
        format!("move your changes into the file named after each, then rm {}", files.join(" ")),
    )
}

/// What a failed unit's check says: why it stopped and its last lines.
fn failure(u: &UnitInfo, last_lines: &[String]) -> String {
    let tail = &last_lines[last_lines.len().saturating_sub(UNIT_LINES)..];
    let error = u.last_error.as_deref().unwrap_or("it stopped restarting");
    format!("{} failed: {error}\n{}", u.name, tail.join("\n"))
}

fn units(units: &[UnitInfo], daemon_running: bool) -> Vec<Check> {
    if daemon_running && units.is_empty() {
        return vec![fail(
            "units",
            "yardd runs no units, so no site is served",
            "fix what the config check names; yardd retries on the next change",
        )];
    }
    let mut checks: Vec<Check> = units
        .iter()
        .filter_map(|u| {
            let UnitState::Failed { last_lines } = &u.state else { return None };
            Some(fail("units", failure(u, last_lines), format!("yard restart {}", u.name)))
        })
        .collect();
    if checks.is_empty() {
        checks.push(ok("units", "no unit has failed"));
    }
    checks
}

fn ok(id: &str, message: impl Into<String>) -> Check {
    Check { id: id.into(), status: CheckStatus::Ok, message: message.into(), hint: None }
}

fn warn(id: &str, message: impl Into<String>, hint: impl Into<String>) -> Check {
    Check { id: id.into(), status: CheckStatus::Warn, message: message.into(), hint: Some(hint.into()) }
}

fn fail(id: &str, message: impl Into<String>, hint: impl Into<String>) -> Check {
    Check { id: id.into(), status: CheckStatus::Fail, message: message.into(), hint: Some(hint.into()) }
}

fn from_error(id: &str, e: &YardError) -> Check {
    let hint = e.hint.clone().unwrap_or_else(|| "yard doctor --json shows the details".into());
    fail(id, e.message.clone(), hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn only_the_brew_settings_that_differ_are_named() {
        let own = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some("/same".to_string()),
            "HOMEBREW_FORBIDDEN_FORMULAE" => Some("yardd's own".to_string()),
            _ => None,
        };
        let client = env(&[
            ("XDG_CONFIG_HOME", "/same"),
            ("HOMEBREW_FORBIDDEN_FORMULAE", "the shell's"),
            ("HOMEBREW_FORBIDDEN_CASKS", "x"),
            ("HOMEBREW_NO_AUTO_UPDATE", "1"),
            ("PATH", "/shell"),
        ]);
        // The same value is nothing to say; only the two policies differ, and the rest is
        // not a setting doctor compares at all.
        assert_eq!(
            env_differences(&client, &own),
            ["HOMEBREW_FORBIDDEN_CASKS", "HOMEBREW_FORBIDDEN_FORMULAE"]
        );
        assert!(env_differences(&env(&[("XDG_CONFIG_HOME", "/same")]), &own).is_empty());
        assert!(env_differences(&BTreeMap::new(), &own).is_empty());
    }

    /// A brew prefix whose PHP `v` conf.d holds `files`, as `(name, text)`.
    fn brew_with(v: PhpVersion, files: &[(&str, &str)]) -> (tempfile::TempDir, Brew) {
        let dir = tempfile::tempdir().unwrap();
        let brew = Brew::new(dir.path());
        let conf_d = brew.php_conf_d(v);
        std::fs::create_dir_all(&conf_d).unwrap();
        for (name, text) in files {
            std::fs::write(conf_d.join(name), text).unwrap();
        }
        (dir, brew)
    }

    #[test]
    fn an_ini_left_by_brew_uninstall_is_a_warning_to_remove_it() {
        let v = PhpVersion { major: 8, minor: 4 };
        let (_dir, brew) = brew_with(
            v,
            &[
                ("20-xdebug.ini", "[xdebug]\nzend_extension=\"/gone/xdebug@8.4/xdebug.so\"\n"),
                ("ext-opcache.ini", "zend_extension=\"opcache.so\"\n"),
            ],
        );
        let checks = stale_brew_inis(&brew, &[v]);
        let file = brew.php_conf_d(v).join("20-xdebug.ini");
        assert_eq!(checks.len(), 1, "a bare name is PHP's to find: {checks:?}");
        assert_eq!(checks[0].id, "brew-ini:8.4/20-xdebug.ini");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("/gone/xdebug@8.4/xdebug.so"), "{}", checks[0].message);
        assert_eq!(checks[0].hint.as_deref(), Some(format!("rm {}", file.display()).as_str()));
    }

    #[test]
    fn a_stale_ini_beside_brews_current_one_is_replaced_by_it() {
        let v = PhpVersion { major: 7, minor: 4 };
        let (dir, brew) = brew_with(v, &[]);
        let so = dir.path().join("opt/phpredis@7.4/redis.so");
        std::fs::create_dir_all(so.parent().unwrap()).unwrap();
        std::fs::write(&so, "").unwrap();
        let conf_d = brew.php_conf_d(v);
        std::fs::write(conf_d.join("20-redis.ini"), "extension=\"/gone/redis@7.4/redis.so\"\n").unwrap();
        std::fs::write(conf_d.join("20-redis.ini.default"), format!("extension=\"{}\"\n", so.display()))
            .unwrap();
        let checks = stale_brew_inis(&brew, &[v]);
        assert_eq!(checks.len(), 1, "{checks:?}");
        assert!(checks[0].message.contains("redis does not load"), "{}", checks[0].message);
        let file = conf_d.join("20-redis.ini");
        assert_eq!(
            checks[0].hint.as_deref(),
            Some(format!("mv {}.default {}", file.display(), file.display()).as_str())
        );
    }

    #[test]
    fn nothing_is_said_when_every_ini_loads() {
        let v = PhpVersion { major: 8, minor: 4 };
        let (_dir, brew) = brew_with(v, &[("ext-intl.ini", "extension=\"intl.so\"\n")]);
        assert!(stale_brew_inis(&brew, &[v]).is_empty());
        assert!(
            stale_brew_inis(&brew, &[PhpVersion { major: 7, minor: 0 }]).is_empty(),
            "no conf.d, nothing to say"
        );
    }
}
