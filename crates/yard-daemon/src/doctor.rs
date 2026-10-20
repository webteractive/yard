//! `yard doctor`: read-only checks, each non-Ok one with the command that fixes it.
//!
//! yardd runs them against its state. The CLI runs them in-process when yardd is down,
//! and then every check that needs config re-reads the TOML files itself.

use crate::daemon::VERSION;
use crate::helper_client::HelperClient;
use crate::reconcile::list_parked;
use crate::state::State;
use crate::{holder, portscan};
use std::path::{Path, PathBuf};
use yard_core::brew::Brew;
use yard_core::config::{load_sites, load_yard};
use yard_core::generated::{Hashes, sha256_hex};
use yard_core::paths::RESOLVER_MARKER;
use yard_core::ports::{DNSMASQ, DNSMASQ_PORT, NGINX_HTTP, Registry};
use yard_core::render::overrides_for;
use yard_core::{Paths, PhpVersion, sites};
use yard_proto::{
    Check, CheckStatus, DoctorReport, Holder, HolderKind, PortInfo, PortMode, PortStatus, UnitInfo,
    UnitState, YardError,
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
}

/// What the config-dependent checks read: yardd's last good snapshot, or the files.
/// A `None` field could not be worked out because a TOML file does not parse.
struct View {
    config_error: Option<YardError>,
    default_php: PhpVersion,
    tld: Option<String>,
    registry: Option<Registry>,
    /// The resolution warnings, and the site count.
    sites: Option<(Vec<String>, usize)>,
    hashes: Result<Hashes, YardError>,
}

impl View {
    fn from_state(state: &State) -> Option<View> {
        let snap = state.snapshot.as_ref()?;
        Some(View {
            config_error: None,
            default_php: snap.yard.default_php,
            tld: Some(snap.tld.clone()),
            registry: Some(snap.registry.clone()),
            sites: Some((snap.resolution.warnings.clone(), snap.resolution.sites.len())),
            hashes: Ok(state.hashes.clone()),
        })
    }

    fn from_files(paths: &Paths) -> View {
        let yard = load_yard(paths);
        let sites_cfg = load_sites(paths);
        let registry = Registry::load(paths);
        let config_error = [yard.as_ref().err(), sites_cfg.as_ref().err(), registry.as_ref().err()]
            .into_iter()
            .flatten()
            .next();
        let tld = match (&yard, &registry) {
            (Ok(y), Ok(_)) => Some(y.effective_tld().to_string()),
            _ => None,
        };
        let sites = match (&yard, &sites_cfg, &registry, &tld) {
            (Ok(y), Ok(s), Ok(r), Some(tld)) => {
                let mut warnings = vec![];
                let mut parked = vec![];
                for dir in &y.parked {
                    match list_parked(dir) {
                        Ok(entries) => parked.push((dir.clone(), entries)),
                        Err(e) => {
                            warnings.push(format!("parked path {} is not readable: {e}", dir.display()))
                        }
                    }
                }
                let http = r.entries.get(NGINX_HTTP).map_or(r.profile.http_port(), |e| e.port);
                let resolution = sites::resolve(y, s, &parked, tld, http, &|p: &Path| p.is_dir());
                warnings.extend(resolution.warnings);
                Some((warnings, resolution.sites.len()))
            }
            _ => None,
        };
        View {
            config_error: config_error.cloned(),
            default_php: yard.as_ref().map_or(PhpVersion::DEFAULT, |y| y.default_php),
            tld,
            registry: registry.ok(),
            sites,
            hashes: Hashes::load(&paths.hashes()),
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
    let mut view = input.state.and_then(View::from_state).unwrap_or_else(|| View::from_files(paths));
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
            "rename, unlink or unpark what the warnings name; yard sites shows what is served",
        ),
        Some((_, count)) => ok("sites", format!("{count} sites resolve")),
        None => warn("sites", "not checked while the configuration does not parse", view.config_hint()),
    });

    checks.extend(units(&input.units, input.daemon_running));
    DoctorReport { checks }
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
            let tail = &last_lines[last_lines.len().saturating_sub(UNIT_LINES)..];
            let error = u.last_error.as_deref().unwrap_or("it stopped restarting");
            Some(fail(
                "units",
                format!("{} failed: {error}\n{}", u.name, tail.join("\n")),
                format!("yard restart {}", u.name),
            ))
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
