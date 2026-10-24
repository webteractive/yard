//! The reconcile loop (spec §6.1): read the TOML files, render every config, write what
//! changed, check it with `nginx -t`, then bring units and the resolver in line. Each
//! step either completes or leaves the previous state in place.

use crate::describe;
use crate::events::EventBus;
use crate::helper_client::HelperClient;
use crate::nginx::NginxCommand;
use crate::probe::{self, Proto};
use crate::services;
use crate::state::{Snapshot, State};
use crate::supervisor::{Health, Reload, Supervisor, UnitSpec};
use crate::tls;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use yard_core::brew::Brew;
use yard_core::config::edit::TomlFile;
use yard_core::config::yard::DEFAULT_TLD;
use yard_core::config::{ServicesConfig, load_services, load_sites, load_yard};
use yard_core::generated::{self, Action};
use yard_core::paths::RESOLVER_MARKER;
use yard_core::php::system_timezone;
use yard_core::ports::{DNSMASQ, DNSMASQ_PORT, NGINX_HTTP, NGINX_HTTPS, Registry};
use yard_core::render::RenderCtx;
use yard_core::render::dnsmasq::dnsmasq_conf;
use yard_core::render::nginx::{default_index, nginx_main, nginx_site, service_site, setup_page};
use yard_core::render::php::{brew_inis, ext_ini, fpm_conf, is_stale, php_fpm_env, php_ini};
use yard_core::services::Engine;
use yard_core::services::render::{config_file, user_overrides};
use yard_core::shim::SHIMS;
use yard_core::sites;
use yard_core::tls::{LEAF_DAYS, needs_issue};
use yard_core::{Paths, PhpVersion};
use yard_proto::{
    CaInfo, ErrorCode, Event, PortMode, PortRef, Profile, SiteInfo, SiteKind, UnitState, YardError,
};

const USER_OWNED_NGINX: &str =
    "# USER-OWNED — Yard never rewrites this file. nginx directives for this site go here.\n";
pub(crate) const USER_OWNED_INI: &str =
    "; USER-OWNED — Yard changes this file only for yard php config set and unset\n";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReconcileReport {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub hand_edited: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub struct Reconciler {
    state: Mutex<State>,
    supervisor: Arc<Supervisor>,
    helper: HelperClient,
    bus: EventBus,
    nginx: NginxCommand,
    /// The zone php.ini uses.
    timezone: String,
    /// Where resolver files live, read to refuse a TLD another tool owns.
    resolver_dir: PathBuf,
    /// The `yard-shim` binary `bin/<shim>` links to; `None` leaves `bin/` alone.
    shim: Option<PathBuf>,
    /// The port each service is offered first (`DaemonOptions.usual_ports`); `None` is the
    /// engine table's.
    usual_ports: Option<BTreeMap<String, u16>>,
    /// Where DBngin.app and Postgres.app would be, for the ports they keep.
    app_dirs: Vec<PathBuf>,
}

const HERD_APP: &str = "/Applications/Herd.app";

/// `<resolver_dir>/<tld>` when it exists without Yard's marker, i.e. belongs to someone else.
pub(crate) fn foreign_resolver(resolver_dir: &Path, tld: &str) -> Option<PathBuf> {
    let file = resolver_dir.join(tld);
    std::fs::read_to_string(&file).is_ok_and(|t| t.lines().next() != Some(RESOLVER_MARKER)).then_some(file)
}

impl Reconciler {
    pub fn new(
        paths: Paths,
        brew: Brew,
        supervisor: Arc<Supervisor>,
        helper: HelperClient,
        bus: EventBus,
        resolver_dir: PathBuf,
    ) -> Result<Self, YardError> {
        let localtime = std::fs::read_link("/etc/localtime").ok();
        Ok(Self {
            nginx: NginxCommand::new(&paths, &brew),
            state: Mutex::new(State::new(paths, brew)?),
            supervisor,
            helper,
            bus,
            timezone: system_timezone(localtime.as_deref()),
            resolver_dir,
            shim: None,
            usual_ports: None,
            app_dirs: Vec::new(),
        })
    }

    /// Where service ports start (`None`: each engine's usual port) and where DBngin.app
    /// and Postgres.app would be installed.
    pub fn with_services(
        mut self,
        usual_ports: Option<BTreeMap<String, u16>>,
        app_dirs: Vec<PathBuf>,
    ) -> Self {
        self.usual_ports = usual_ports;
        self.app_dirs = app_dirs;
        self
    }

    pub fn with_shim(mut self, shim: Option<PathBuf>) -> Self {
        self.shim = shim;
        self
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    pub async fn snapshot(&self) -> Option<Snapshot> {
        self.state.lock().await.snapshot.clone()
    }

    pub async fn last_error(&self) -> Option<YardError> {
        self.state.lock().await.last_error.clone()
    }

    /// A copy, so `doctor` does not hold up a reconcile while it waits on the helper.
    pub async fn state(&self) -> State {
        self.state.lock().await.clone()
    }

    /// After a hand edit of `nginx/overrides/`: test nginx, then reload it or report why
    /// not. Nothing is rendered, so nothing is rolled back; nginx keeps the old config.
    pub async fn apply_overrides(&self) -> Result<(), YardError> {
        let mut state = self.state.lock().await;
        self.test_nginx().await?.inspect_err(|e| state.last_error = Some(e.clone()))?;
        if state.last_error.as_ref().is_some_and(|e| e.code == ErrorCode::NginxConfigInvalid) {
            state.last_error = None;
        }
        self.supervisor.reload("nginx").await
    }

    /// A served PHP site whose detected docroot changed since the last reconcile, e.g.
    /// `public/` appearing after `laravel new` created the project. Nothing watches project
    /// directories, so the periodic scan finds it.
    pub async fn docroots_stale(&self) -> bool {
        let state = self.state.lock().await;
        let Some(snap) = &state.snapshot else { return false };
        snap.resolution.sites.iter().any(|site| {
            let SiteKind::Php { path, docroot, .. } = &site.kind else { return false };
            let pinned = match snap.projects.get(&site.name) {
                Some(Ok(file)) => file.docroot.is_some(),
                Some(Err(_)) => false,
                None => snap.sites_cfg.sites.get(&site.name).is_some_and(|e| e.docroot.is_some()),
            };
            !pinned && sites::detect_docroot(Path::new(path), &|p: &Path| p.is_dir()) != Path::new(docroot)
        })
    }

    /// A secured site whose certificate is missing, from another CA, for other names or
    /// close to expiry. Nothing watches `certs/` or the clock, so the periodic scan finds it.
    pub async fn certs_due(&self) -> bool {
        let state = self.state.lock().await;
        let Some(snap) = &state.snapshot else { return false };
        let mut secure = snap.resolution.sites.iter().filter(|s| s.secure).peekable();
        let console = secure_console(&snap.services, &snap.resolution.sites, &snap.yard, &snap.tld);
        if secure.peek().is_none() && console.is_none() {
            return false;
        }
        let now = OffsetDateTime::now_utc();
        if tls::ca_due(&state.paths, now) {
            return true;
        }
        let ca = std::fs::read(state.paths.ca_cert()).ok();
        let now = now.unix_timestamp();
        secure.any(|s| {
            needs_issue(tls::facts_against(&state.paths, &s.name, ca.as_deref()).as_ref(), &s.host, now)
        }) || console.is_some_and(|(name, host)| {
            needs_issue(tls::service_facts_against(&state.paths, name, ca.as_deref()).as_ref(), &host, now)
        })
    }

    /// Yard's CA, made now if it is missing, and whether `security` says the login
    /// keychain trusts it. The CA is made under the reconcile lock, so a reconcile never
    /// signs with one half written.
    pub async fn ca_info(&self, security: &Path) -> Result<CaInfo, YardError> {
        let paths = {
            let state = self.state.lock().await;
            tls::ensure_ca(&state.paths, &services::macos_user(), OffsetDateTime::now_utc())?;
            state.paths.clone()
        };
        let cert = paths.ca_cert();
        let trusted = tls::trusted(security, &cert).await.unwrap_or(false);
        Ok(CaInfo {
            cert: cert.display().to_string(),
            sha256: tls::ca_sha256(&paths)?,
            trusted,
            expires: tls::ca_expires(&paths)?,
        })
    }

    /// `Ok(Err(..))` when nginx rejects the config; `Err` when the test could not run.
    pub async fn test_nginx(&self) -> Result<Result<(), YardError>, YardError> {
        Ok(self.nginx.test().await?.map_err(|output| nginx_rejected(&output, false)))
    }

    /// Serialized: a second call waits for the first to finish.
    pub async fn reconcile(&self) -> Result<ReconcileReport, YardError> {
        let mut state = self.state.lock().await;
        let result = self.run(&mut state).await;
        state.last_error = result.as_ref().err().cloned();
        result
    }

    async fn run(&self, state: &mut State) -> Result<ReconcileReport, YardError> {
        let mut report = ReconcileReport::default();

        // 1. Load config. A parse error keeps the last good state serving.
        let loaded = load_yard(&state.paths).and_then(|y| {
            Ok((y, load_sites(&state.paths)?, load_services(&state.paths)?, Registry::load(&state.paths)?))
        });
        let ((yard, yard_warnings), sites_cfg, services_cfg, mut registry) = loaded?;
        report.warnings.extend(yard_warnings);
        let tld = yard.effective_tld().to_string();
        // Moving to a TLD another tool's resolver claims would serve nothing (Herd's `.test`).
        let old_tld = state.snapshot.as_ref().map(|s| s.tld.as_str());
        if old_tld != Some(tld.as_str())
            && let Some(file) = foreign_resolver(&self.resolver_dir, &tld)
        {
            let owner = if tld == "test" && Path::new(HERD_APP).exists() { "Herd" } else { "another tool" };
            let back = old_tld.unwrap_or(DEFAULT_TLD);
            return Err(YardError::new(
                ErrorCode::ResolverForeign,
                format!("{} belongs to {owner}, so .{tld} would not reach Yard", file.display()),
            )
            .with_hint(format!("set `tld` in yard.toml back to {back:?}, or remove it")));
        }

        // 2. Settle ports. A port one of our running units holds is free for that unit.
        let before = registry.clone();
        let own = self.own_ports(state.snapshot.as_ref());
        let is_free = |p: u16| own.contains(&p) || probe::is_free(p, Proto::Tcp);
        let profile = registry.profile;
        let http = registry.ensure(NGINX_HTTP, PortMode::Pinned, Some(profile.http_port()), &is_free)?;
        let https = registry.ensure(NGINX_HTTPS, PortMode::Pinned, Some(profile.https_port()), &is_free)?;
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
        // A service's ports were pinned when it was added. Reconcile only reads them, and
        // pins a new one the same way when `ports.toml` lost its entry.
        let service_ports = self.settle_service_ports(&services_cfg, &mut registry, &is_free, &mut report);

        // 3. Resolve sites.
        let projects = crate::project::read_projects(&sites_cfg);
        let mut resolution =
            sites::resolve(&yard, &sites_cfg, &projects, &tld, http, https, &|p: &Path| p.is_dir());
        crate::project::fill_needs(
            &mut resolution.sites,
            &projects,
            &yard,
            Some(&services_cfg),
            &state.brew,
            &state.paths,
        );
        report.warnings.extend(resolution.warnings.iter().cloned());

        // 4. Issue the certificates secured sites need, which `nginx -t` loads.
        let paths = state.paths.clone();
        let console = secure_console(&services_cfg, &resolution.sites, &yard, &tld);
        if issue_certs(&paths, &resolution.sites, console.as_ref(), &mut report)? {
            state.nginx_reload_pending = true;
        }

        // 5. Render, and create the user-owned files the rendered ones include.
        let ctx = RenderCtx {
            paths: &paths,
            brew: &state.brew,
            tld: &tld,
            http_port: http,
            https_port: https,
            timezone: &self.timezone,
        };
        // Pools run for the versions sites use; every installed version gets its inis, so
        // the CLI shims load the same extensions and overrides.
        let versions = php_versions(&resolution.sites);
        let exists = |p: &Path| p.exists();
        let read_link = |p: &Path| std::fs::read_link(p).ok();
        let mut inis: BTreeSet<PhpVersion> =
            state.brew.installed_php(&exists, &read_link).into_iter().collect();
        inis.extend(versions.iter().copied());
        let mut rendered = BTreeMap::new();
        rendered.insert(paths.nginx_conf(), nginx_main(&ctx));
        rendered.insert(paths.nginx_default_index(), default_index(&ctx, &resolution.sites));
        rendered.insert(paths.dnsmasq_conf(), dnsmasq_conf(&ctx, dns));
        for site in &resolution.sites {
            rendered.insert(paths.nginx_site(&site.name), nginx_site(&ctx, site));
            if site.needs_setup() {
                rendered.insert(paths.nginx_setup_page(&site.name), setup_page(&ctx, site));
            }
        }
        let no_exts = BTreeMap::new();
        for v in &inis {
            rendered.insert(paths.php_ini(*v), php_ini(&ctx, *v));
            rendered.insert(paths.fpm_conf(*v), fpm_conf(&ctx, *v));
            let exts = yard.extensions.get(v).unwrap_or(&no_exts);
            let conf_d = paths.php_conf_d(*v);
            let brew_files = brew_conf_d(&state.brew.php_conf_d(*v));
            for (name, text) in brew_inis(&ctx, &brew_files, exts) {
                rendered.insert(conf_d.join(name), text);
            }
            for (name, entry) in exts {
                if let Some(text) = ext_ini(&ctx, name, entry, &brew_files) {
                    rendered.insert(conf_d.join(format!("50-{name}.ini")), text);
                }
            }
        }
        // Each instance's own config, and `minio.<tld>` unless a site has the name.
        let mut service_confs = BTreeMap::new();
        for (instance, entry) in &services_cfg.services {
            let Some(ports) = service_ports.get(instance) else { continue };
            if let Some((file, text)) = config_file(entry.engine, instance, &paths, ports) {
                service_confs.insert(instance.clone(), file.clone());
                rendered.insert(file, text);
            }
        }
        if serves_console(&services_cfg, &resolution.sites)
            && let Some(ports) = service_ports.get("minio")
            && let Some(console) = ports.iter().find(|p| p.name == "minio-console")
        {
            let secure = yard.secure_by_default;
            rendered.insert(paths.nginx_service("minio"), service_site(&ctx, "minio", console.port, secure));
        }
        prepare_dirs(&paths, &resolution.sites, &inis, &services_cfg).map_err(io_error)?;

        // 6. Write.
        let mut hashes = state.hashes.clone();
        let mut managed = vec![paths.nginx_sites_dir(), paths.nginx_setup_dir(), paths.nginx_services_dir()];
        for dir in subdirs(&paths.root().join("php")) {
            managed.push(dir.join("conf.d"));
            managed.push(dir);
        }
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
        if let Some(shim) = &self.shim {
            report.warnings.extend(link_shims(&paths.bin_dir(), shim));
        }

        // 7. Test nginx. Only a failed test rolls back; an nginx that cannot run is a warning.
        // A new certificate is loaded only by a reload.
        let changed: BTreeSet<PathBuf> = applied.changed().map(Path::to_path_buf).collect();
        let nginx_changed =
            state.nginx_reload_pending || changed.iter().any(|p| p.starts_with(paths.nginx_dir()));
        if nginx_changed || state.snapshot.is_none() {
            match self.nginx.test().await {
                Ok(Err(output)) => {
                    generated::rollback(&applied, &mut hashes).map_err(io_error)?;
                    return Err(nginx_rejected(&output, true));
                }
                Ok(Ok(())) => {}
                Err(e) => report.warnings.push(format!("nginx -t did not run: {}", describe(&e))),
            }
        }
        if let Err(e) = hashes.save(&paths.hashes()) {
            report.warnings.push(format!("cannot save {}: {e}", paths.hashes().display()));
        }
        state.hashes = hashes;
        // Only now: until the new confs passed, the old ones still named these files.
        report.removed.extend(prune_certs(&paths, &resolution.sites, console.is_some()));

        // 8. Apply units; restart the ones whose files changed and reload nginx. In the
        // privileged profile nginx inherits the helper's sockets, whose bind is the probe.
        let existing: BTreeSet<String> = self.supervisor.list().into_iter().map(|u| u.name).collect();
        let privileged = profile == Profile::Privileged;
        let inherit = if privileged { vec![http, https] } else { vec![] };
        let mut desired = vec![
            UnitSpec {
                name: "dnsmasq".into(),
                program: state.brew.dnsmasq_bin(),
                args: vec![
                    "--keep-in-foreground".into(),
                    OsString::from(format!("--conf-file={}", paths.dnsmasq_conf().display())),
                ],
                env: vec![],
                health: Health::Tcp(dns),
                ports: vec![(dns, Proto::TcpUdp)],
                reload: Reload::Restart,
                stop: libc::SIGTERM,
                inherit: vec![],
                stop_timeout: None,
                pid_file: None,
            },
            UnitSpec {
                name: "nginx".into(),
                program: self.nginx.program().to_path_buf(),
                args: self.nginx.args(),
                env: vec![],
                health: Health::Tcp(http),
                ports: if privileged { vec![] } else { vec![(http, Proto::Tcp)] },
                reload: Reload::Signal(libc::SIGHUP),
                stop: libc::SIGQUIT,
                inherit: inherit.clone(),
                stop_timeout: None,
                pid_file: None,
            },
        ];
        let mut restart = vec![];
        if changed.contains(&paths.dnsmasq_conf()) {
            restart.push("dnsmasq".to_string());
        }
        for v in &versions {
            let opt = match state.brew.php_opt(*v, &exists, &read_link) {
                Ok(opt) => opt,
                Err(e) => {
                    report.warnings.push(describe(&e));
                    continue;
                }
            };
            let name = fpm_unit(*v);
            let conf_d = paths.php_conf_d(*v);
            if changed.contains(&paths.fpm_conf(*v))
                || changed.contains(&paths.php_ini(*v))
                || changed.iter().any(|p| p.starts_with(&conf_d))
            {
                restart.push(name.clone());
            }
            desired.push(UnitSpec {
                name,
                program: Brew::php_fpm(&opt),
                args: vec![
                    "--nodaemonize".into(),
                    "-y".into(),
                    paths.fpm_conf(*v).into(),
                    "-c".into(),
                    paths.php_ini(*v).into(),
                ],
                env: php_fpm_env(&ctx, *v),
                health: Health::Unix(paths.fpm_socket(*v)),
                ports: vec![],
                reload: Reload::Signal(libc::SIGUSR2),
                stop: libc::SIGTERM,
                inherit: vec![],
                stop_timeout: None,
                pid_file: None,
            });
        }
        for (instance, entry) in services_cfg.services.iter().filter(|(_, e)| !e.stopped) {
            let Some(ports) = service_ports.get(instance) else { continue };
            let cmd =
                services::unit_command(&state.brew, &paths, entry.engine, instance, &entry.formula, ports);
            if let Some(warning) = missing_keg(&state.brew, instance, &entry.formula, &cmd.program) {
                report.warnings.push(warning);
                continue;
            }
            if service_confs.get(instance).is_some_and(|conf| changed.contains(conf)) {
                restart.push(instance.clone());
            }
            desired.push(UnitSpec {
                name: instance.clone(),
                program: cmd.program,
                args: cmd.args,
                env: cmd.env,
                health: Health::Tcp(ports.first().map_or(0, |p| p.port)),
                ports: ports.iter().map(|p| (p.port, Proto::Tcp)).collect(),
                reload: Reload::Restart,
                stop: cmd.stop_signal,
                inherit: vec![],
                stop_timeout: Some(services::STOP_TIMEOUT),
                pid_file: Some(paths.service_pid(instance)),
            });
        }
        if !existing.contains("nginx")
            && let Some(pid) = self.nginx.stop_orphan().await
        {
            report.warnings.push(format!("stopped nginx (pid {pid}), which an earlier yardd left running"));
        }
        // A unit apply started reads the new files itself; restarting or reloading it on
        // top would start it twice.
        let started = self.supervisor.apply(desired).await;
        self.supervisor.sockets().release_except(&inherit);
        for name in restart.iter().filter(|n| existing.contains(*n) && !started.contains(*n)) {
            if let Err(e) = self.supervisor.restart(name).await {
                report.warnings.push(format!("restarting {name}: {}", describe(&e)));
            }
        }
        if nginx_changed && existing.contains("nginx") && !started.contains("nginx") {
            match self.supervisor.reload("nginx").await {
                Ok(()) => state.nginx_reload_pending = false,
                Err(e) => report.warnings.push(format!("reloading nginx: {}", describe(&e))),
            }
        } else if started.contains("nginx") {
            state.nginx_reload_pending = false;
        }

        // 9. Resolver: install the new TLD's file, then drop the old one.
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

        // 10. Commit.
        if registry != before {
            TomlFile::edit(&paths.ports_toml(), |file| {
                registry.save(file);
                Ok(())
            })?;
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
        state.snapshot =
            Some(Snapshot { yard, sites_cfg, services: services_cfg, projects, registry, tld, resolution });
        Ok(report)
    }

    /// The registry entries of every instance in `services.toml`, as the `PortRef`s its unit
    /// command takes, main port first. An existing entry is read as it is (a user's edit of
    /// `ports.toml` wins); a missing one is pinned at the first free port from the engine's
    /// usual one, around the ports DBngin and Postgres.app keep. An instance whose port
    /// cannot be had is a warning and has no unit.
    fn settle_service_ports(
        &self,
        services: &ServicesConfig,
        registry: &mut Registry,
        is_free: &dyn Fn(u16) -> bool,
        report: &mut ReconcileReport,
    ) -> BTreeMap<String, Vec<PortRef>> {
        let missing = services.services.iter().any(|(i, e)| {
            services::port_entries(e.engine, i).iter().any(|(n, ..)| !registry.entries.contains_key(n))
        });
        let reserved = if missing {
            services::reserved(&self.app_dirs, self.usual_ports.as_ref())
        } else {
            Default::default()
        };
        let mut settled = BTreeMap::new();
        'instances: for (instance, entry) in &services.services {
            let mut ports = vec![];
            for (name, key, usual) in services::port_entries(entry.engine, instance) {
                let port = match registry.entries.get(&name) {
                    Some(e) => e.port,
                    None => {
                        let usual =
                            self.usual_ports.as_ref().and_then(|p| p.get(&key)).copied().unwrap_or(usual);
                        match registry.allocate_from(&name, usual, &reserved, is_free) {
                            Ok(port) => port,
                            Err(e) => {
                                report.warnings.push(format!("{instance}: {}", describe(&e)));
                                continue 'instances;
                            }
                        }
                    }
                };
                ports.push(PortRef { name, port });
            }
            settled.insert(instance.clone(), ports);
        }
        settled
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
            .filter(|(unit, _)| running.contains(port_owner(unit)))
            .map(|(_, e)| e.port)
            .collect()
    }
}

/// The supervised unit behind a registry entry: nginx owns both nginx ports, and MinIO's
/// console port belongs to the `minio` unit that also holds its API port.
pub fn port_owner(entry: &str) -> &str {
    match entry {
        NGINX_HTTP | NGINX_HTTPS => "nginx",
        "minio-console" => "minio",
        _ => entry,
    }
}

pub fn fpm_unit(v: PhpVersion) -> String {
    format!("php-fpm-{v}")
}

/// The versions served sites run; a site on its setup page runs none.
fn php_versions(sites: &[SiteInfo]) -> BTreeSet<PhpVersion> {
    sites
        .iter()
        .filter(|s| !s.needs_setup())
        .filter_map(|s| match &s.kind {
            SiteKind::Php { php, .. } => php.parse().ok(),
            SiteKind::Proxy { .. } => None,
        })
        .collect()
}

/// Makes the CA when a site is secure and issues each secure site whose certificate
/// `needs_issue`, and likewise `console`, the `(name, host)` of a secured service host
/// (`minio.<tld>`). `Ok(true)` when it issued one.
fn issue_certs(
    paths: &Paths,
    sites: &[SiteInfo],
    console: Option<&(&str, String)>,
    report: &mut ReconcileReport,
) -> Result<bool, YardError> {
    let secure: Vec<&SiteInfo> = sites.iter().filter(|s| s.secure).collect();
    if secure.is_empty() && console.is_none() {
        return Ok(false);
    }
    let now = OffsetDateTime::now_utc();
    tls::ensure_ca(paths, &services::macos_user(), now)?;
    let ca = std::fs::read(paths.ca_cert()).ok();
    let mut issued = false;
    for site in secure {
        let facts = tls::facts_against(paths, &site.name, ca.as_deref());
        if needs_issue(facts.as_ref(), &site.host, now.unix_timestamp()) {
            tls::issue(paths, &site.name, &site.host, now, LEAF_DAYS)?;
            report.written.push(paths.cert(&site.name));
            issued = true;
        }
    }
    if let Some((name, host)) = console {
        let facts = tls::service_facts_against(paths, name, ca.as_deref());
        if needs_issue(facts.as_ref(), host, now.unix_timestamp()) {
            tls::issue_service(paths, name, host, now, LEAF_DAYS)?;
            report.written.push(paths.services_certs_dir().join(format!("{name}.pem")));
            issued = true;
        }
    }
    Ok(issued)
}

/// Whether `minio.<tld>` is served: there is a `minio` instance and no site took the name.
pub(crate) fn serves_console(services: &ServicesConfig, sites: &[SiteInfo]) -> bool {
    services.services.contains_key("minio") && !sites.iter().any(|s| s.name == "minio")
}

/// `("minio", "minio.<tld>")` when that host is served over HTTPS, which `secure_by_default`
/// decides: a service has no `secure` setting of its own.
fn secure_console(
    services: &ServicesConfig,
    sites: &[SiteInfo],
    yard: &yard_core::config::YardConfig,
    tld: &str,
) -> Option<(&'static str, String)> {
    (yard.secure_by_default && serves_console(services, sites)).then(|| ("minio", format!("minio.{tld}")))
}

/// The warning for an instance whose binary is not there, or `None`. A keg directory with no
/// binary in it is a broken install, which `brew install` skips, so that one says reinstall.
fn missing_keg(brew: &Brew, instance: &str, formula: &str, program: &Path) -> Option<String> {
    if program.exists() {
        return None;
    }
    let keg = services::keg(brew, formula);
    Some(if keg.exists() {
        format!("{instance}: {} is missing (brew reinstall {formula})", program.display())
    } else {
        format!("{instance}: {} is missing (brew install {formula})", keg.display())
    })
}

/// Removes every `certs/<name>.{pem,key}` whose site is not secure, and any `.yard-tmp`
/// file an interrupted write left there; the same under `certs/services/` unless
/// `console` (a secured `minio.<tld>`) keeps its certificate.
fn prune_certs(paths: &Paths, sites: &[SiteInfo], console: bool) -> Vec<PathBuf> {
    let secure: BTreeSet<&str> = sites.iter().filter(|s| s.secure).map(|s| s.name.as_str()).collect();
    let stale = |f: &Path, keep: &dyn Fn(&str) -> bool| match f.extension().and_then(|e| e.to_str()) {
        Some("yard-tmp") => true,
        Some("pem" | "key") => f.file_stem().and_then(|s| s.to_str()).is_none_or(|s| !keep(s)),
        _ => false,
    };
    let sites_certs = files_in(&paths.certs_dir()).into_iter().filter(|f| stale(f, &|s| secure.contains(s)));
    let service_certs =
        files_in(&paths.services_certs_dir()).into_iter().filter(|f| stale(f, &|s| console && s == "minio"));
    sites_certs.chain(service_certs).filter(|f| std::fs::remove_file(f).is_ok()).collect()
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    files
}

/// Keeps each `bin/<shim>` a symlink to `shim`. A link to another `yard-shim` (an older
/// build) is Yard's and is moved; anything else there is the user's and stays.
fn link_shims(bin: &Path, shim: &Path) -> Vec<String> {
    if !shim.is_file() {
        return vec![format!(
            "{} is missing, so the php shims in {} are not linked",
            shim.display(),
            bin.display()
        )];
    }
    if let Err(e) = std::fs::create_dir_all(bin) {
        return vec![format!("cannot create {}: {e}", bin.display())];
    }
    let mut warnings = vec![];
    for name in SHIMS {
        let link = bin.join(name);
        let ours = match std::fs::read_link(&link) {
            Ok(target) if target == shim => continue,
            Ok(target) => target.file_name().is_some_and(|n| n == "yard-shim"),
            Err(_) => link.symlink_metadata().is_err(),
        };
        if !ours {
            warnings.push(format!("{} is not Yard's shim; left it alone", link.display()));
            continue;
        }
        let _ = std::fs::remove_file(&link);
        if let Err(e) = std::os::unix::fs::symlink(shim, &link) {
            warnings.push(format!("cannot link {}: {e}", link.display()));
        }
    }
    warnings
}

/// Brew's conf.d for one version, without the inis brew left behind for an uninstalled
/// extension (whose `.so` is gone).
pub(crate) fn brew_conf_d(dir: &Path) -> Vec<(String, String)> {
    let mut files = ini_files(dir);
    files.retain(|(_, text)| !is_stale(text, &|p: &Path| p.exists()));
    files
}

/// The `.ini` files in brew's conf.d for one version, as `(name, text)`; a missing
/// directory has none.
pub(crate) fn ini_files(dir: &Path) -> Vec<(String, String)> {
    files_in(dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "ini"))
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            Some((name, std::fs::read_to_string(&p).ok()?))
        })
        .collect()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
}

/// Directories nginx and the units write into, and the user-owned files the generated
/// configs include, which must exist before `nginx -t` or a unit starts.
fn prepare_dirs(
    paths: &Paths,
    sites: &[SiteInfo],
    versions: &BTreeSet<PhpVersion>,
    services: &ServicesConfig,
) -> std::io::Result<()> {
    for dir in [
        paths.logs_dir(),
        paths.run_dir().join("nginx"),
        paths.nginx_sites_dir(),
        paths.nginx_setup_dir(),
        paths.nginx_services_dir(),
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
    for (instance, entry) in &services.services {
        // The file my.cnf, redis.conf or minio.<tld>'s block includes; PostgreSQL has none.
        if let Some(initial) = user_overrides(entry.engine) {
            generated::ensure_user_file(&paths.service_overrides(instance), initial)?;
        }
        // redis-server exits at config load when its `dir` is missing. MySQL's and PostgreSQL's
        // init needs the data directory empty or absent, and MinIO makes its own.
        if entry.engine == Engine::Redis {
            std::fs::create_dir_all(paths.service_data(instance))?;
        }
    }
    Ok(())
}

/// `rolled_back`: Yard rendered a change and put its previous files back. Otherwise
/// nothing was rendered (an override edit, a restart) and nginx keeps what it had.
fn nginx_rejected(output: &str, rolled_back: bool) -> YardError {
    let first = output
        .lines()
        .find(|l| l.contains("[emerg]"))
        .or_else(|| output.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("nginx -t failed")
        .trim();
    let (message, hint) = if rolled_back {
        (
            format!("nginx rejected the new config ({first}); Yard kept serving the previous config"),
            "fix the file named above; your change is saved and applies once nginx accepts it",
        )
    } else {
        (
            format!("nginx rejected the config ({first}); nginx keeps serving the config it had"),
            "fix the file named above; Yard applies it when the file changes",
        )
    };
    YardError::new(ErrorCode::NginxConfigInvalid, message)
        .with_hint(hint)
        .with_data(serde_json::json!({ "output": output }))
}

fn io_error(e: std::io::Error) -> YardError {
    YardError::internal(format!("writing generated files: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_keg_names_the_command_that_fixes_it() {
        let dir = tempfile::tempdir().unwrap();
        let brew = Brew::new(dir.path());
        let program = dir.path().join("opt/mysql@8.4/bin/mysqld");
        let keg = dir.path().join("opt/mysql@8.4");
        assert_eq!(
            missing_keg(&brew, "mysql-8.4", "mysql@8.4", &program),
            Some(format!("mysql-8.4: {} is missing (brew install mysql@8.4)", keg.display()))
        );
        // A keg with no binary in it is a broken install, which `brew install` would skip.
        std::fs::create_dir_all(&keg).unwrap();
        assert_eq!(
            missing_keg(&brew, "mysql-8.4", "mysql@8.4", &program),
            Some(format!("mysql-8.4: {} is missing (brew reinstall mysql@8.4)", program.display()))
        );
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "").unwrap();
        assert_eq!(missing_keg(&brew, "mysql-8.4", "mysql@8.4", &program), None);
    }
}
