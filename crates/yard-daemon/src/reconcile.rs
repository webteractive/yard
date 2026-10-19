//! The reconcile loop (spec §6.1): read the TOML files, render every config, write what
//! changed, check it with `nginx -t`, then bring units and the resolver in line. Each
//! step either completes or leaves the previous state in place.

use crate::describe;
use crate::events::EventBus;
use crate::helper_client::HelperClient;
use crate::probe::{self, Proto};
use crate::state::{Snapshot, State};
use crate::supervisor::{Health, Supervisor, UnitKind, UnitSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use yard_core::brew::Brew;
use yard_core::config::edit::TomlFile;
use yard_core::config::{load_sites, load_yard};
use yard_core::generated::{self, Action};
use yard_core::php::system_timezone;
use yard_core::ports::{DNSMASQ, DNSMASQ_PORT, NGINX_HTTP, NGINX_HTTPS, Registry};
use yard_core::render::RenderCtx;
use yard_core::render::dnsmasq::dnsmasq_conf;
use yard_core::render::nginx::{default_index, nginx_main, nginx_site};
use yard_core::render::php::{fpm_conf, php_fpm_env, php_ini};
use yard_core::sites::{self, DirEntry, EntryKind};
use yard_core::{Paths, PhpVersion};
use yard_proto::{ErrorCode, Event, PortMode, SiteInfo, SiteKind, UnitState, YardError};

const USER_OWNED_NGINX: &str =
    "# USER-OWNED — Yard never rewrites this file. nginx directives for this site go here.\n";
const USER_OWNED_INI: &str = "; USER-OWNED — Yard never rewrites this file\n";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReconcileReport {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub hand_edited: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Who nginx workers run as, and the zone php.ini uses.
struct Identity {
    user: String,
    group: String,
    timezone: String,
}

pub struct Reconciler {
    state: Mutex<State>,
    supervisor: Arc<Supervisor>,
    helper: HelperClient,
    bus: EventBus,
    identity: Identity,
}

impl Reconciler {
    pub fn new(
        paths: Paths,
        brew: Brew,
        supervisor: Arc<Supervisor>,
        helper: HelperClient,
        bus: EventBus,
    ) -> Result<Self, YardError> {
        let localtime = std::fs::read_link("/etc/localtime").ok();
        let identity = Identity {
            user: std::env::var("USER").unwrap_or_else(|_| "nobody".into()),
            group: "staff".into(),
            timezone: system_timezone(localtime.as_deref()),
        };
        Ok(Self { state: Mutex::new(State::new(paths, brew)?), supervisor, helper, bus, identity })
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    pub async fn snapshot(&self) -> Option<Snapshot> {
        self.state.lock().await.snapshot.clone()
    }

    pub async fn config_error(&self) -> Option<YardError> {
        self.state.lock().await.config_error.clone()
    }

    /// Serialized: a second call waits for the first to finish.
    pub async fn reconcile(&self) -> Result<ReconcileReport, YardError> {
        let mut state = self.state.lock().await;
        self.run(&mut state).await
    }

    async fn run(&self, state: &mut State) -> Result<ReconcileReport, YardError> {
        let mut report = ReconcileReport::default();

        // 1. Load config. A parse error keeps the last good state serving.
        let loaded = load_yard(&state.paths)
            .and_then(|y| Ok((y, load_sites(&state.paths)?, Registry::load(&state.paths)?)));
        let (yard, sites_cfg, mut registry) = match loaded {
            Ok(loaded) => loaded,
            Err(e) => {
                state.config_error = Some(e.clone());
                return Err(e);
            }
        };

        // 2. Settle ports. A port one of our running units holds is free for that unit.
        let tld = yard.tld.clone().unwrap_or_else(|| registry.profile.tld().to_string());
        let before = registry.clone();
        let own = self.own_ports(state.snapshot.as_ref());
        let is_free = |p: u16| own.contains(&p) || probe::is_free(p, Proto::Tcp);
        let profile = registry.profile;
        let http = registry.ensure(NGINX_HTTP, PortMode::Pinned, Some(profile.http_port()), &is_free)?;
        registry.ensure(NGINX_HTTPS, PortMode::Pinned, Some(profile.https_port()), &is_free)?;
        let dns = registry.ensure(DNSMASQ, PortMode::Pinned, Some(DNSMASQ_PORT), &is_free)?;
        let mut proxies = BTreeSet::new();
        for (name, entry) in &sites_cfg.sites {
            let Some(target) = &entry.proxy else { continue };
            let Ok((_, port)) = sites::parse_proxy_target(target) else { continue };
            let unit = format!("proxy:{name}");
            match registry.ensure(&unit, PortMode::External, Some(port), &is_free) {
                Ok(_) => {
                    proxies.insert(unit);
                }
                Err(e) => report.warnings.push(format!("{unit}: {}", describe(&e))),
            }
        }
        registry.entries.retain(|unit, _| !unit.starts_with("proxy:") || proxies.contains(unit));

        // 3. Resolve sites.
        let mut parked = vec![];
        for dir in &yard.parked {
            match list_parked(dir) {
                Ok(entries) => parked.push((dir.clone(), entries)),
                Err(e) => report.warnings.push(format!("parked path {} is not readable: {e}", dir.display())),
            }
        }
        let resolution = sites::resolve(&yard, &sites_cfg, &parked, &tld, http, &|p: &Path| p.is_dir());
        report.warnings.extend(resolution.warnings.iter().cloned());

        // 4. Render, and create the user-owned files the rendered ones include.
        let paths = state.paths.clone();
        let ctx = RenderCtx {
            paths: &paths,
            brew: &state.brew,
            user: &self.identity.user,
            group: &self.identity.group,
            tld: &tld,
            http_port: http,
            timezone: &self.identity.timezone,
        };
        let versions = php_versions(&resolution.sites);
        let mut rendered = BTreeMap::new();
        rendered.insert(paths.nginx_conf(), nginx_main(&ctx));
        rendered.insert(paths.nginx_default_index(), default_index(&ctx, &resolution.sites));
        rendered.insert(paths.dnsmasq_conf(), dnsmasq_conf(&ctx, dns));
        for site in &resolution.sites {
            rendered.insert(paths.nginx_site(&site.name), nginx_site(&ctx, site));
        }
        for v in &versions {
            rendered.insert(paths.php_ini(*v), php_ini(&ctx, *v));
            rendered.insert(paths.fpm_conf(*v), fpm_conf(&ctx, *v));
        }
        prepare_dirs(&paths, &resolution.sites, &versions).map_err(io_error)?;

        // 5. Write.
        let mut hashes = state.hashes.clone();
        let mut managed = vec![paths.nginx_sites_dir()];
        managed.extend(subdirs(&paths.root().join("php")));
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        let plan = generated::plan(&rendered, &hashes, &managed, &read, &files_in);
        let applied = generated::apply(&plan, &rendered, &mut hashes).map_err(io_error)?;
        for (path, action) in &plan.actions {
            match action {
                Action::Write | Action::BackupThenWrite => report.written.push(path.clone()),
                Action::Remove | Action::BackupThenRemove => report.removed.push(path.clone()),
                Action::Unchanged | Action::KeepHandEdit => {}
            }
            if matches!(action, Action::BackupThenWrite | Action::BackupThenRemove) {
                let backup = generated::backup_path(path);
                report.warnings.push(format!(
                    "{} was edited by hand; saved it as {}",
                    path.display(),
                    backup.display()
                ));
            }
        }
        for path in &plan.hand_edited {
            report.warnings.push(format!("kept your edit of {}; Yard will not update it", path.display()));
        }
        report.hand_edited = plan.hand_edited.clone();

        // 6. Test nginx. Only a failed test rolls back; no helper means nothing can run it.
        let changed: BTreeSet<PathBuf> = applied.changed().map(Path::to_path_buf).collect();
        let nginx_changed = changed.iter().any(|p| p.starts_with(paths.nginx_dir()));
        if nginx_changed || state.snapshot.is_none() {
            match self.helper.test_nginx().await {
                Ok(result) if !result.ok => {
                    generated::rollback(&applied, &mut hashes).map_err(io_error)?;
                    return Err(nginx_rejected(&result.output));
                }
                Ok(_) => {}
                Err(e) => report.warnings.push(format!("nginx -t did not run: {}", describe(&e))),
            }
        }
        if let Err(e) = hashes.save(&paths.hashes()) {
            report.warnings.push(format!("cannot save {}: {e}", paths.hashes().display()));
        }
        state.hashes = hashes;

        // 7. Apply units; restart the ones whose files changed and reload nginx.
        let existing: BTreeSet<String> = self.supervisor.list().into_iter().map(|u| u.name).collect();
        let mut desired = vec![
            UnitSpec {
                name: "dnsmasq".into(),
                kind: UnitKind::Process {
                    program: state.brew.dnsmasq_bin(),
                    args: vec![
                        "--keep-in-foreground".into(),
                        OsString::from(format!("--conf-file={}", paths.dnsmasq_conf().display())),
                    ],
                    env: vec![],
                },
                health: Health::Tcp(dns),
                port: Some((dns, Proto::TcpUdp)),
            },
            UnitSpec {
                name: "nginx".into(),
                kind: UnitKind::Nginx,
                health: Health::Tcp(http),
                port: Some((http, Proto::Tcp)),
            },
        ];
        let mut restart = vec![];
        if changed.contains(&paths.dnsmasq_conf()) {
            restart.push("dnsmasq".to_string());
        }
        for v in &versions {
            let exists = |p: &Path| p.exists();
            let read_link = |p: &Path| std::fs::read_link(p).ok();
            let opt = match state.brew.php_opt(*v, &exists, &read_link) {
                Ok(opt) => opt,
                Err(e) => {
                    report.warnings.push(describe(&e));
                    continue;
                }
            };
            let name = fpm_unit(*v);
            if changed.contains(&paths.fpm_conf(*v)) || changed.contains(&paths.php_ini(*v)) {
                restart.push(name.clone());
            }
            desired.push(UnitSpec {
                name,
                kind: UnitKind::Process {
                    program: Brew::php_fpm(&opt),
                    args: vec![
                        "--nodaemonize".into(),
                        "-y".into(),
                        paths.fpm_conf(*v).into(),
                        "-c".into(),
                        paths.php_ini(*v).into(),
                    ],
                    env: php_fpm_env(&ctx, *v),
                },
                health: Health::Unix(paths.fpm_socket(*v)),
                port: None,
            });
        }
        self.supervisor.apply(desired).await;
        for name in restart.iter().filter(|n| existing.contains(*n)) {
            if let Err(e) = self.supervisor.restart(name).await {
                report.warnings.push(format!("restarting {name}: {}", describe(&e)));
            }
        }
        // A new nginx unit may find an nginx from an earlier yardd still running on old configs.
        if nginx_changed {
            match self.supervisor.reload("nginx").await {
                Ok(()) => {}
                Err(e) if matches!(e.code, ErrorCode::HelperRefused | ErrorCode::HelperNotInstalled) => {}
                Err(e) => report.warnings.push(format!("reloading nginx: {}", describe(&e))),
            }
        }

        // 8. Resolver: install the new TLD's file, then drop the old one.
        let want = (tld.clone(), dns);
        if state.applied_resolver.as_ref() != Some(&want) {
            match self.helper.install_resolver(&tld, dns).await {
                Ok(()) => {
                    if let Some((old, _)) = state.applied_resolver.take()
                        && old != tld
                        && let Err(e) = self.helper.remove_resolver(&old).await
                    {
                        report.warnings.push(format!("removing the .{old} resolver: {}", describe(&e)));
                    }
                    state.applied_resolver = Some(want);
                }
                Err(e) => report.warnings.push(format!("installing the .{tld} resolver: {}", describe(&e))),
            }
        }

        // 9. Commit.
        if registry != before {
            let mut file = TomlFile::load(&paths.ports_toml())?;
            registry.save(&mut file);
            file.save()?;
        }
        let old: BTreeSet<String> = state
            .snapshot
            .as_ref()
            .map(|s| s.resolution.sites.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        for site in resolution.sites.iter().filter(|s| !old.contains(&s.name)) {
            self.bus.emit(Event::SiteAdded { site: site.clone() });
        }
        let new: BTreeSet<&str> = resolution.sites.iter().map(|s| s.name.as_str()).collect();
        for name in old.iter().filter(|n| !new.contains(n.as_str())) {
            self.bus.emit(Event::SiteRemoved { name: name.clone() });
        }
        state.snapshot = Some(Snapshot { yard, sites_cfg, registry, tld, resolution });
        state.config_error = None;
        Ok(report)
    }

    /// Registered ports whose unit is running now: nginx-http and nginx-https belong to nginx.
    fn own_ports(&self, snapshot: Option<&Snapshot>) -> BTreeSet<u16> {
        let running: BTreeSet<String> = self
            .supervisor
            .list()
            .into_iter()
            .filter(|u| u.state == UnitState::Running)
            .map(|u| u.name)
            .collect();
        let Some(snapshot) = snapshot else { return BTreeSet::new() };
        snapshot
            .registry
            .entries
            .iter()
            .filter(|(unit, _)| {
                let owner = if *unit == NGINX_HTTP || *unit == NGINX_HTTPS { "nginx" } else { unit.as_str() };
                running.contains(owner)
            })
            .map(|(_, e)| e.port)
            .collect()
    }
}

pub fn fpm_unit(v: PhpVersion) -> String {
    format!("php-fpm-{v}")
}

fn php_versions(sites: &[SiteInfo]) -> BTreeSet<PhpVersion> {
    sites
        .iter()
        .filter_map(|s| match &s.kind {
            SiteKind::Php { php, .. } => php.parse().ok(),
            SiteKind::Proxy { .. } => None,
        })
        .collect()
}

/// A parked path's children, following symlinks to tell directories from files.
fn list_parked(dir: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => EntryKind::Dir,
            Ok(_) => EntryKind::File,
            Err(_) => EntryKind::BrokenLink,
        };
        out.push(DirEntry { name: entry.file_name().to_string_lossy().into_owned(), path, kind });
    }
    Ok(out)
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    files
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
}

/// Directories nginx and the units write into, and the user-owned files the generated
/// configs include, which must exist before `nginx -t`.
fn prepare_dirs(paths: &Paths, sites: &[SiteInfo], versions: &BTreeSet<PhpVersion>) -> std::io::Result<()> {
    for dir in [
        paths.logs_dir(),
        paths.run_dir().join("nginx"),
        paths.nginx_sites_dir(),
        paths.nginx_dir().join("overrides"),
    ] {
        std::fs::create_dir_all(dir)?;
    }
    for site in sites {
        generated::ensure_user_file(&paths.nginx_override(&site.name), USER_OWNED_NGINX)?;
    }
    for v in versions {
        generated::ensure_user_file(&paths.php_overrides(*v), USER_OWNED_INI)?;
    }
    Ok(())
}

fn nginx_rejected(output: &str) -> YardError {
    let first = output
        .lines()
        .find(|l| l.contains("[emerg]"))
        .or_else(|| output.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("nginx -t failed")
        .trim();
    YardError::new(
        ErrorCode::NginxConfigInvalid,
        format!("nginx rejected the new config ({first}); the previous configs are back in place"),
    )
    .with_hint("fix the file named above; Yard applies it on the next change")
    .with_data(serde_json::json!({ "output": output }))
}

fn io_error(e: std::io::Error) -> YardError {
    YardError::internal(format!("writing generated files: {e}"))
}
