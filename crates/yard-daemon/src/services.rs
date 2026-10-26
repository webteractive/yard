//! Service instances at run time: where each one's keg and unit command are, the orphans a
//! killed yardd left behind, and the `ServiceInfo` every service method answers with
//! (M4b spec §4 to §6, §15).

use crate::holder;
use crate::jobs::JobCtx;
use crate::orphans;
use crate::probe::{self, Proto};
use crate::reconcile::{Reconciler, serves_console};
use crate::sources::{brew_stdout, require_trusted};
use crate::state::Snapshot;
use crate::supervisor::Supervisor;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use toml_edit::value;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::{ServicesConfig, TomlFile, load_services};
use yard_core::ports::{NGINX_HTTP, NGINX_HTTPS, Registry};
use yard_core::project::ServiceWant;
use yard_core::services::brew_info::{self, Formula};
use yard_core::services::render::{UnitCommand, command, init_command};
use yard_core::services::reserved::reserved_ports;
use yard_core::services::{
    DATABASE_START_TIMEOUT, Engine, READY_WAIT, candidate_names, instance_name, line_formula, resolve_line,
};
use yard_core::sources::tap_of;
use yard_proto::{ErrorCode, PortRef, ServiceInfo, ServiceState, SettingSource, UnitState, YardError};

/// How long a service gets to stop after its stop signal before SIGKILL: a database
/// finishes its checkpoint (spec §15).
pub const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// The unit's `start_timeout` for an instance of `engine`; `None` is the supervisor's grace.
pub fn start_timeout(engine: Engine) -> Option<Duration> {
    match engine {
        Engine::Mysql | Engine::Postgres => Some(DATABASE_START_TIMEOUT),
        Engine::Redis | Engine::Minio => None,
    }
}

/// The names of an instance's registry entries, in the order the unit lists its ports and
/// with the engine's usual port for each: `mysql-8.4` for MySQL, `minio` and `minio-console`
/// for MinIO. The key is the one `DaemonOptions.usual_ports` uses.
pub fn port_entries(engine: Engine, instance: &str) -> Vec<(String, String, u16)> {
    engine
        .usual_ports()
        .iter()
        .map(|(suffix, usual)| {
            if suffix.is_empty() {
                (instance.to_string(), engine.key().to_string(), *usual)
            } else {
                (format!("{instance}-{suffix}"), format!("{}-{suffix}", engine.key()), *usual)
            }
        })
        .collect()
}

/// brew's keg for a recorded formula: `opt/<name>`, whose name is the last segment of
/// `minio/stable/minio`.
pub fn keg(brew: &Brew, formula: &str) -> PathBuf {
    brew.prefix.join("opt").join(formula.rsplit('/').next().unwrap_or(formula))
}

/// `FORMULA_MISSING` when the binary that runs an instance on `formula` is not there, else
/// `None`. A keg directory with no binary in it is a broken install, which `brew install` would
/// skip, so that one says reinstall (homebrew rule).
pub fn missing_program(brew: &Brew, formula: &str, program: &std::path::Path) -> Option<YardError> {
    if program.exists() {
        return None;
    }
    let keg = keg(brew, formula);
    let (missing, fix) = if keg.exists() { (program.to_path_buf(), "reinstall") } else { (keg, "install") };
    Some(
        YardError::new(ErrorCode::FormulaMissing, format!("{} is missing", missing.display()))
            .with_hint(format!("brew {fix} {formula}")),
    )
}

/// What running `instance` takes, with its ports read from the registry: the unit command.
pub fn unit_command(
    brew: &Brew,
    paths: &Paths,
    engine: Engine,
    instance: &str,
    formula: &str,
    ports: &[PortRef],
) -> UnitCommand {
    command(engine, instance, &keg(brew, formula), paths, ports)
}

/// Stops the service processes an earlier yardd left running (spec §15), for every
/// instance `services.toml` lists, and returns `(instance, pid)` for each it stopped. A
/// `services.toml` that does not parse is the reconcile's to report, not this.
pub async fn stop_orphans(paths: &Paths, brew: &Brew) -> Vec<(String, u32)> {
    let Ok(services) = load_services(paths) else { return vec![] };
    let mut stopped = vec![];
    for (instance, entry) in &services.services {
        let cmd = unit_command(brew, paths, entry.engine, instance, &entry.formula, &[]);
        let pid_file = paths.service_pid(instance);
        if let Some(pid) = orphans::stop(&pid_file, &cmd.program, STOP_TIMEOUT, cmd.stop_signal).await {
            stopped.push((instance.clone(), pid));
        }
    }
    stopped
}

/// The macOS user: PostgreSQL's superuser, and who the CA's name (`Yard CA (<user>)`) says
/// it belongs to.
pub fn macos_user() -> String {
    std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "user".into())
}

/// The instance a `.yard` value of `true` means for `engine`: the default line's if it
/// exists, else the first by name (spec §9).
pub fn default_instance(services: &ServicesConfig, engine: Engine) -> Option<&str> {
    let default = instance_name(engine, None);
    if services.services.contains_key(&default) {
        return services.services.get_key_value(&default).map(|(name, _)| name.as_str());
    }
    services.services.iter().find(|(_, e)| e.engine == engine).map(|(name, _)| name.as_str())
}

/// The sites whose `.yard` asks for `instance`: by its line, or by `true` when it is the
/// instance `true` means.
pub fn used_by(snapshot: &Snapshot, engine: Engine, instance: &str) -> Vec<String> {
    let services = &snapshot.services;
    snapshot
        .projects
        .iter()
        .filter_map(|(site, project)| Some((site, project.ok()?.services.get(&engine)?)))
        .filter(|(_, want)| match want {
            ServiceWant::Line(line) => instance_name(engine, Some(line)) == instance,
            ServiceWant::Default => default_instance(services, engine) == Some(instance),
        })
        .map(|(site, _)| site.clone())
        .collect()
}

/// `http(s)://minio.<tld>` with nginx's port for that scheme, left out when it is the
/// scheme's own (80, 443).
pub fn console_url(tld: &str, secure: bool, http: u16, https: u16) -> String {
    let (scheme, port, default) = if secure { ("https", https, 443) } else { ("http", http, 80) };
    if port == default { format!("{scheme}://minio.{tld}") } else { format!("{scheme}://minio.{tld}:{port}") }
}

/// One instance of `services.toml` as the service methods report it, or `None` when
/// `name` is not in it. The state comes from the supervised unit (`Stopped` with
/// `stopped_by_user` when `services.toml` says `stopped`; a backoff counts as starting).
pub fn service_info(snapshot: &Snapshot, supervisor: &Supervisor, name: &str) -> Option<ServiceInfo> {
    let entry = snapshot.services.services.get(name)?;
    let engine = entry.engine;
    let ports: Vec<PortRef> = port_entries(engine, name)
        .into_iter()
        .filter_map(|(entry_name, ..)| {
            let port = snapshot.registry.entries.get(&entry_name)?.port;
            Some(PortRef { name: entry_name, port })
        })
        .collect();
    let state = if entry.stopped {
        ServiceState::Stopped
    } else {
        let unit = supervisor.list().into_iter().find(|u| u.name == name);
        match unit.map(|u| (u.state, u.last_error)) {
            Some((UnitState::Running, _)) => ServiceState::Running,
            Some((UnitState::Starting | UnitState::Backoff { .. }, _)) => ServiceState::Starting,
            Some((UnitState::Failed { last_lines }, last_error)) => {
                ServiceState::Failed { last_lines, reason: failure_reason(name, last_error.as_deref()) }
            }
            // Reconcile started no unit for it (a missing keg, a port it could not have).
            None if snapshot.left_out.contains_key(name) => {
                ServiceState::Failed { last_lines: vec![], reason: snapshot.left_out.get(name).cloned() }
            }
            Some((UnitState::Stopped, _)) | None => ServiceState::Stopped,
        }
    };
    let port_of = |unit: &str, usual: u16| snapshot.registry.entries.get(unit).map_or(usual, |e| e.port);
    let console = (engine == Engine::Minio && serves_console(&snapshot.services, &snapshot.resolution.sites))
        .then(|| {
            let profile = snapshot.registry.profile;
            let http = port_of(NGINX_HTTP, profile.http_port());
            let https = port_of(NGINX_HTTPS, profile.https_port());
            console_url(&snapshot.tld, snapshot.yard.secure_by_default, http, https)
        });
    Some(ServiceInfo {
        name: name.to_string(),
        engine: engine.key().to_string(),
        version: entry.version.clone(),
        formula: entry.formula.clone(),
        ports,
        state,
        stopped_by_user: entry.stopped,
        account: engine.account(&macos_user()),
        console,
        used_by: used_by(snapshot, engine, name),
        sleeping_since: None,
        sleep_after_secs: None,
        sleep_after_source: SettingSource::Default,
    })
}

/// Why the unit of `instance` failed, from the supervisor's `last_error`: a port someone else
/// holds is `PORT_IN_USE` naming the holder (`port 3307 is held by mysqld (pid 77034, DBngin)`),
/// hint `yard service port <i> <port>`, since retrying cannot free it; anything else is
/// `JOB_FAILED` with the supervisor's words and a hint at the log.
pub fn failure_reason(instance: &str, last_error: Option<&str>) -> Option<YardError> {
    let last_error = last_error?;
    Some(match crate::supervisor::conflict_of(last_error) {
        Some((port, message)) => YardError::new(ErrorCode::PortInUse, message)
            .with_hint(format!("yard service port {instance} <port>"))
            .with_data(serde_json::json!({ "port": port })),
        None => YardError::new(ErrorCode::JobFailed, last_error)
            .with_hint(format!("yard service logs {instance}")),
    })
}

// ---- Ports ----

/// The port `usual_ports` (`DaemonOptions`) gives `key`, else the engine table's `default`.
pub fn usual_port(usual_ports: Option<&BTreeMap<String, u16>>, key: &str, default: u16) -> u16 {
    usual_ports.and_then(|ports| ports.get(key)).copied().unwrap_or(default)
}

/// The ports DBngin.app and Postgres.app keep (`reserved_ports`). When `usual_ports`
/// replaces the engines' usual ports (tests), each reserved port is the one of the engine
/// whose usual port it is, so the reservation then lands on a port the test owns and never
/// on 3306, 5432 or 6379.
pub fn reserved(app_dirs: &[PathBuf], usual_ports: Option<&BTreeMap<String, u16>>) -> BTreeSet<u16> {
    let real = reserved_ports(app_dirs, &|p: &std::path::Path| p.exists());
    real.into_iter()
        .map(|port| {
            let owner = Engine::ALL.into_iter().find(|e| e.usual_ports()[0].1 == port);
            owner.map_or(port, |e| usual_port(usual_ports, e.key(), port))
        })
        .collect()
}

const ADD_PORT_HINT: &str = "pass a different --port, or leave --port out and Yard picks the next free port";

/// `PORT_IN_USE` for a port asked for by name (`--port`, `yard service port`), naming who
/// has it and hinting at asking again. The registry's own message and hint are about
/// `yard ports set`, which does not fit either.
fn refuse_port(e: YardError, port: u16, registered_to: Option<String>, hint: String) -> YardError {
    if e.code != ErrorCode::PortInUse {
        return e;
    }
    let message = match registered_to {
        Some(other) => format!("port {port} is already registered to {other}"),
        None => format!("port {port} is held by {}", holder::label(holder::find_holder(port).as_ref())),
    };
    let mut refused = YardError::new(ErrorCode::PortInUse, message).with_hint(hint);
    if let Some(data) = e.data {
        refused = refused.with_data(data);
    }
    refused
}

// ---- Brew lookups ----

/// `brew info --json=v2 <name>`: the formula brew describes, `None` when brew cannot (no
/// such formula, a tap it will not load). An `Err` is only brew failing to run at all.
async fn describe(brew: &Brew, name: &str) -> Result<Option<Formula>, YardError> {
    let (ok, out, _) = brew_stdout(brew, &["info", "--json=v2", name]).await?;
    if !ok {
        return Ok(None);
    }
    Ok(brew_info::parse(&out).ok().and_then(|formulae| formulae.into_iter().next()))
}

/// The names `brew search --formula "/^<base>(@|$)/"` finds. A search with no match fails
/// in real brew, which is no names here.
async fn search(brew: &Brew, engine: Engine) -> Result<Vec<String>, YardError> {
    let pattern = format!("/^{}(@|$)/", engine.base_formula());
    let (ok, out, _) = brew_stdout(brew, &["search", "--formula", &pattern]).await?;
    if !ok {
        return Ok(Vec::new());
    }
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("==>"))
        .map(String::from)
        .collect())
}

fn cannot_describe(formula: &str) -> YardError {
    YardError::new(ErrorCode::FormulaMissing, format!("brew could not describe {formula}"))
        .with_hint("brew update, then try again")
}

/// The formula `add` installs or uses, decided before the job starts.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The formula recorded in `services.toml` and installed: `mysql@8.4`, `redis`,
    /// `minio/stable/minio` (or `minio`, when homebrew/core's is already installed).
    pub formula: String,
    /// What brew said of it, for the deprecation warning. `None` for a MinIO that is yet to
    /// be installed from its tap: brew refuses to describe an untrusted tap's formula.
    pub info: Option<Formula>,
}

/// Which formula serves `engine`'s `line` (spec §5), asking brew once per formula.
/// `SERVICE_VERSION_UNSUPPORTED` names the lines brew does have; a formula brew cannot
/// describe is `FORMULA_MISSING`, not a missing line.
pub async fn resolve(brew: &Brew, engine: Engine, line: Option<&str>) -> Result<Resolved, YardError> {
    match engine {
        Engine::Mysql | Engine::Postgres => {
            let line = line.ok_or_else(|| YardError::internal("a versioned engine needs a line"))?;
            let names = search(brew, engine).await?;
            let mut infos: BTreeMap<String, Option<Formula>> = BTreeMap::new();
            for name in candidate_names(engine, &names) {
                let info = describe(brew, &name).await?;
                infos.insert(name, info);
            }
            if let Some(name) = line_formula(engine, line, &names)
                && infos.get(&name).is_some_and(Option::is_none)
            {
                return Err(cannot_describe(&name));
            }
            let found = resolve_line(engine, line, &names, &|name| infos.get(name).cloned().flatten())?;
            Ok(Resolved { formula: found.full_name.clone(), info: Some(found) })
        }
        Engine::Redis => {
            let formula = engine.base_formula();
            let info = describe(brew, formula).await?;
            if info.is_none() && !keg(brew, formula).exists() {
                return Err(cannot_describe(formula));
            }
            Ok(Resolved { formula: formula.to_string(), info })
        }
        Engine::Minio => {
            const TAP_FORMULA: &str = "minio/stable/minio";
            // homebrew/core's minio and the tap's share `opt/minio`, so an installed keg's
            // tap is brew's to say (spec §15). A core one is used as it is.
            if keg(brew, TAP_FORMULA).exists() {
                let info = describe(brew, engine.base_formula()).await?;
                return Ok(match info {
                    Some(f) if f.tap == "homebrew/core" => {
                        Resolved { formula: f.full_name.clone(), info: Some(f) }
                    }
                    info => Resolved { formula: TAP_FORMULA.to_string(), info },
                });
            }
            Ok(Resolved { formula: TAP_FORMULA.to_string(), info: None })
        }
    }
}

/// The job line for a deprecated formula, naming brew's disable date.
fn deprecation_note(f: &Formula) -> Option<String> {
    if !f.deprecated && f.disable_date.is_none() {
        return None;
    }
    Some(match &f.disable_date {
        Some(date) => format!("warning: {} is deprecated; brew disables it on {date}", f.full_name),
        None => format!("warning: {} is deprecated; brew has set no date to disable it", f.full_name),
    })
}

// ---- Add ----

pub fn exists(instance: &str) -> YardError {
    YardError::new(ErrorCode::ServiceExists, format!("{instance} is already added"))
        .with_hint("yard services lists the instances")
}

pub fn not_found(instance: &str) -> YardError {
    YardError::new(ErrorCode::ServiceNotFound, format!("no service instance named {instance:?}"))
        .with_hint("yard services lists the instances")
}

/// What an add has created so far, so a failure or a cancel undoes exactly that.
#[derive(Default)]
struct Created {
    /// What `services/<i>/` held before the add; `None` when the directory did not exist.
    dir_before: Option<BTreeSet<String>>,
    /// The data directory had data the add did not make: `remove --keep-data`'s.
    reused_data: bool,
    ports: bool,
    entry: bool,
}

/// One `services.add`, checked by the handler. `run` is the job body.
pub struct Add {
    pub paths: Paths,
    pub brew: Brew,
    pub reconciler: Arc<Reconciler>,
    pub engine: Engine,
    pub instance: String,
    /// The line (`8.4`), `None` for an engine with none.
    pub version: Option<String>,
    pub resolved: Resolved,
    /// `--port`: the instance's (first) port, else the usual one or the next free.
    pub port: Option<u16>,
    pub usual_ports: Option<BTreeMap<String, u16>>,
    pub app_dirs: Vec<PathBuf>,
}

impl Add {
    /// Install the formula if its keg is missing, allocate the ports, initialize the data
    /// directory, write the `services.toml` entry, reconcile and wait for the health check.
    /// A failure or a cancel after that undoes what this add created.
    pub async fn run(self, job: JobCtx) -> Result<(), YardError> {
        // Jobs run one at a time, so a second add of this instance that passed the handler's
        // check meets the first one's entry here, before it touches anything.
        if load_services(&self.paths)?.services.contains_key(&self.instance) {
            return Err(exists(&self.instance));
        }
        if let Some(note) = self.resolved.info.as_ref().and_then(deprecation_note) {
            job.line(note);
        }
        let keg = keg(&self.brew, &self.resolved.formula);
        if !keg.exists() {
            // The handler gated this already; an install that waited behind another job is
            // gated again, since brew trusts a fully qualified formula without asking.
            if tap_of(&self.resolved.formula).is_some() {
                require_trusted(&self.brew, &self.resolved.formula).await?;
            }
            job.run(&self.brew.brew_bin(), &["install", &self.resolved.formula], &Brew::job_env()).await?;
        }
        let program =
            unit_command(&self.brew, &self.paths, self.engine, &self.instance, &self.resolved.formula, &[])
                .program;
        if !program.exists() {
            return Err(YardError::new(
                ErrorCode::FormulaMissing,
                format!("{} is installed but {} is missing", self.resolved.formula, program.display()),
            )
            .with_hint(format!("brew reinstall {}", self.resolved.formula)));
        }

        let mut created = Created::default();
        let result = self.provision(&job, &keg, &mut created).await;
        if let Err(e) = &result {
            job.line(format!("rolling back {}: {}", self.instance, e.message));
            self.roll_back(&job, &created).await;
        }
        result
    }

    async fn provision(
        &self,
        job: &JobCtx,
        keg: &std::path::Path,
        created: &mut Created,
    ) -> Result<(), YardError> {
        let (paths, instance) = (&self.paths, self.instance.as_str());
        created.dir_before = list_dir(&paths.service_dir(instance));
        let ports = self.allocate().await?;
        created.ports = true;
        job.line(format!(
            "{instance} gets {}",
            ports.iter().map(|p| format!("{} {}", p.name, p.port)).collect::<Vec<_>>().join(", ")
        ));

        let data = paths.service_data(instance);
        if is_empty(&data) {
            if let Some(init) = init_command(self.engine, instance, keg, paths, &macos_user()) {
                // mysqld makes `data`, but not the directory above it.
                std::fs::create_dir_all(paths.service_dir(instance)).map_err(|e| {
                    YardError::internal(format!(
                        "cannot create {}: {e}",
                        paths.service_dir(instance).display()
                    ))
                })?;
                job.line(format!("initializing {instance}'s data in {}", data.display()));
                let args: Option<Vec<&str>> = init.args.iter().map(|a| a.to_str()).collect();
                let args = args.ok_or_else(|| YardError::internal("an init argument is not UTF-8"))?;
                job.run(&init.program, &args, &init.env).await?;
            }
        } else {
            created.reused_data = true;
            job.line(format!("reusing the data in {}", data.display()));
        }

        TomlFile::edit(&paths.services_toml(), |file| {
            if file.doc().get("services").and_then(|s| s.get(instance)).is_some() {
                return Err(exists(instance));
            }
            let at = |key: &'static str| ["services", instance, key];
            file.set(&at("engine"), value(self.engine.key()));
            if let Some(version) = &self.version {
                file.set(&at("version"), value(version.as_str()));
            }
            file.set(&at("formula"), value(self.resolved.formula.as_str()));
            Ok(())
        })?;
        created.entry = true;

        let report = self.reconciler.reconcile().await?;
        job.check_cancelled()?;
        self.wait_ready(job, &report.warnings).await?;
        let port = ports.first().map_or(0, |p| p.port);
        job.line(format!("{instance} is running on 127.0.0.1:{port}"));
        Ok(())
    }

    /// Takes the instance's ports from `ports.toml` (`--port` for its first), pinned.
    async fn allocate(&self) -> Result<Vec<PortRef>, YardError> {
        let (paths, engine, instance) = (self.paths.clone(), self.engine, self.instance.clone());
        let (wanted, usual_ports, app_dirs) = (self.port, self.usual_ports.clone(), self.app_dirs.clone());
        // The probes bind sockets and the holder lookup runs lsof.
        tokio::task::spawn_blocking(move || {
            let reserved = reserved(&app_dirs, usual_ports.as_ref());
            let is_free = |port: u16| probe::is_free(port, Proto::Tcp);
            TomlFile::edit(&paths.ports_toml(), |file| {
                let mut registry = Registry::load(&paths)?;
                let mut granted = vec![];
                for (i, (name, key, usual)) in port_entries(engine, &instance).into_iter().enumerate() {
                    let port = match (i, wanted) {
                        (0, Some(port)) => {
                            let registered_to = registry.holder_of(port, &name).map(str::to_string);
                            registry
                                .allocate_exact(&name, port, &is_free)
                                .map_err(|e| refuse_port(e, port, registered_to, ADD_PORT_HINT.into()))?
                        }
                        _ => {
                            let usual = usual_port(usual_ports.as_ref(), &key, usual);
                            registry.allocate_from(&name, usual, &reserved, &is_free)?
                        }
                    };
                    granted.push(PortRef { name, port });
                }
                registry.save(file);
                Ok(granted)
            })
        })
        .await
        .map_err(YardError::internal)?
    }

    /// Waits for the unit to pass its health check. A unit that failed ends it with its
    /// last lines; reconcile starting none (a warning names why) ends it at once.
    async fn wait_ready(&self, job: &JobCtx, warnings: &[String]) -> Result<(), YardError> {
        let instance = self.instance.as_str();
        let supervisor = self.reconciler.supervisor();
        let deadline = Instant::now() + READY_WAIT;
        loop {
            job.check_cancelled()?;
            let unit = supervisor.list().into_iter().find(|u| u.name == instance);
            match unit.map(|u| u.state) {
                None => {
                    let why: Vec<&str> = warnings
                        .iter()
                        .filter(|w| w.starts_with(&format!("{instance}:")))
                        .map(String::as_str)
                        .collect();
                    let why = if why.is_empty() {
                        "reconcile started no unit for it".to_string()
                    } else {
                        why.join("; ")
                    };
                    return Err(YardError::new(
                        ErrorCode::JobFailed,
                        format!("{instance} did not start: {why}"),
                    ));
                }
                Some(UnitState::Running) => return Ok(()),
                Some(UnitState::Failed { last_lines }) => {
                    let last_error = unit_error(supervisor, instance);
                    let reason = failure_reason(instance, last_error.as_deref());
                    let failed = ServiceState::Failed { last_lines, reason };
                    return Err(failed.failure(instance).unwrap_or_else(|| {
                        YardError::new(ErrorCode::JobFailed, format!("{instance} failed to start"))
                    }));
                }
                Some(_) => {}
            }
            if Instant::now() >= deadline {
                return Err(YardError::new(
                    ErrorCode::JobFailed,
                    format!("{instance} did not pass its health check within {} s", READY_WAIT.as_secs()),
                )
                .with_hint(format!("yard service logs {instance}")));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Removes what this add created: the entry and the port entries, then (once the unit
    /// is stopped) the data it initialized and whatever else is new in `services/<i>/`.
    /// Data that was there before is never deleted. A problem here is a job line, not the
    /// job's error, which stays the one that caused the rollback.
    async fn roll_back(&self, job: &JobCtx, created: &Created) {
        let (paths, instance) = (&self.paths, self.instance.as_str());
        let undone = forget(paths, self.engine, instance, created.entry, created.ports);
        let stopped = match undone {
            Ok(()) if created.entry => stop_and_wait(&self.reconciler, instance).await,
            other => other,
        };
        let cleaned = match stopped {
            Ok(()) => delete_new(paths, instance, created).await,
            Err(e) => Err(e),
        };
        if let Err(e) = cleaned {
            job.line(format!("could not finish rolling back {instance}: {}", e.message));
        }
    }
}

/// The supervisor's `last_error` for `instance`'s unit.
fn unit_error(supervisor: &Supervisor, instance: &str) -> Option<String> {
    supervisor.list().into_iter().find(|u| u.name == instance).and_then(|u| u.last_error)
}

// ---- Removing ----

/// Drops `instance`'s `services.toml` entry (`entry`) and its port entries (`ports`).
fn forget(paths: &Paths, engine: Engine, instance: &str, entry: bool, ports: bool) -> Result<(), YardError> {
    if entry {
        TomlFile::edit(&paths.services_toml(), |file| {
            file.remove(&["services", instance]);
            Ok(())
        })?;
    }
    if ports {
        TomlFile::edit(&paths.ports_toml(), |file| {
            let mut registry = Registry::load(paths)?;
            for (name, ..) in port_entries(engine, instance) {
                registry.entries.remove(&name);
            }
            registry.save(file);
            Ok(())
        })?;
    }
    Ok(())
}

/// Reconciles, which stops the unit of an instance that left `services.toml`, and waits
/// until the supervisor no longer lists it, so nothing writes into a directory that is
/// about to go. A reconcile that fails (another file is invalid) leaves the unit running
/// and is the error.
async fn stop_and_wait(reconciler: &Reconciler, instance: &str) -> Result<(), YardError> {
    reconciler.reconcile().await?;
    let deadline = Instant::now() + STOP_TIMEOUT + Duration::from_secs(10);
    while reconciler.supervisor().list().iter().any(|u| u.name == instance) {
        if Instant::now() >= deadline {
            return Err(YardError::internal(format!("{instance} is still running, so its directory stays")));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// The entries of `dir`, `None` when it is not a directory.
fn list_dir(dir: &std::path::Path) -> Option<BTreeSet<String>> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

/// Absent or empty: what `mysqld --initialize` and `initdb` accept.
fn is_empty(dir: &std::path::Path) -> bool {
    list_dir(dir).is_none_or(|entries| entries.is_empty())
}

/// Deletes a file or a directory tree; a path that is already gone is fine.
fn remove_path(path: &std::path::Path) -> Result<(), YardError> {
    let removed = if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(YardError::internal(format!("cannot delete {}: {e}", path.display())))
        }
        _ => Ok(()),
    }
}

/// Deletes `services/<i>/`, or everything in it but `data/` with `keep_data`.
async fn delete_dir(paths: &Paths, instance: &str, keep_data: bool) -> Result<(), YardError> {
    let dir = paths.service_dir(instance);
    tokio::task::spawn_blocking(move || {
        if !keep_data {
            return remove_path(&dir);
        }
        let Some(entries) = list_dir(&dir) else { return Ok(()) };
        entries.iter().filter(|n| n.as_str() != "data").try_for_each(|name| remove_path(&dir.join(name)))
    })
    .await
    .map_err(YardError::internal)?
}

/// A rollback's cleanup of `services/<i>/`: the data directory unless the add reused it,
/// and every other entry the add did not find there (a file the user put in first stays).
/// The directory goes too when the add made it.
async fn delete_new(paths: &Paths, instance: &str, created: &Created) -> Result<(), YardError> {
    let dir = paths.service_dir(instance);
    let (before, reused) = (created.dir_before.clone(), created.reused_data);
    tokio::task::spawn_blocking(move || {
        let Some(entries) = list_dir(&dir) else { return Ok(()) };
        for name in entries {
            let ours =
                if name == "data" { !reused } else { before.as_ref().is_none_or(|b| !b.contains(&name)) };
            if ours {
                remove_path(&dir.join(&name))?;
            }
        }
        if before.is_none() {
            let _ = std::fs::remove_dir(&dir);
        }
        Ok(())
    })
    .await
    .map_err(YardError::internal)?
}

/// `services.remove`: drops the entry and the port entries, stops the instance, and only
/// then deletes `services/<i>/` (its `data/` stays with `keep_data`). The caller has
/// checked that the instance exists and that no site asks for it.
pub async fn remove(
    paths: &Paths,
    reconciler: &Reconciler,
    engine: Engine,
    instance: &str,
    keep_data: bool,
) -> Result<(), YardError> {
    forget(paths, engine, instance, true, true)?;
    stop_and_wait(reconciler, instance).await?;
    delete_dir(paths, instance, keep_data).await
}

// ---- Start, stop, port, logs ----

/// `services.stop` and `services.start`: sets `stopped` on the instance's entry, or removes
/// the key (not `stopped = false`), so the user's file is as it was before the stop. The
/// entry is checked inside the edit: one that a remove deleted since the caller looked is
/// `SERVICE_NOT_FOUND` and nothing is saved, where a `set` would write a stub
/// `[services.<i>] stopped = true` that `services.toml` cannot parse. The caller reconciles
/// afterwards.
pub fn set_stopped(paths: &Paths, instance: &str, stopped: bool) -> Result<(), YardError> {
    TomlFile::edit(&paths.services_toml(), |file| {
        if file.doc().get("services").and_then(|s| s.get(instance)).is_none() {
            return Err(not_found(instance));
        }
        if stopped {
            file.set(&["services", instance, "stopped"], value(true));
        } else {
            file.remove(&["services", instance, "stopped"]);
        }
        Ok(())
    })
}

/// `services.port`: pins the instance's first port (MinIO's API port) to `port`, which must
/// be unregistered or the instance's own, and free. A refusal names the holder and hints at
/// asking again. The port is probed and the holder looked up with lsof, so this blocks.
pub fn move_port(paths: &Paths, engine: Engine, instance: &str, port: u16) -> Result<(), YardError> {
    let Some((name, ..)) = port_entries(engine, instance).into_iter().next() else {
        return Err(YardError::internal(format!("{instance} has no port")));
    };
    TomlFile::edit(&paths.ports_toml(), |file| {
        let mut registry = Registry::load(paths)?;
        // Its own port is busy with its own process, and moving there is not a move.
        if registry.entries.get(&name).is_some_and(|e| e.port == port) {
            return Ok(());
        }
        let registered_to = registry.holder_of(port, &name).map(str::to_string);
        let hint = format!("pass a different port: yard service port {instance} <port>");
        registry
            .allocate_exact(&name, port, &|p| probe::is_free(p, Proto::Tcp))
            .map_err(|e| refuse_port(e, port, registered_to, hint))?;
        registry.save(file);
        Ok(())
    })
}

/// How far back from the end of a log the tail reads each time.
const TAIL_BLOCK: u64 = 8 * 1024;

/// The most lines `services.logs` returns, whatever a caller asks for.
pub const MAX_LOG_LINES: usize = 10_000;

/// The most bytes the tail reads, so one huge line cannot make it read the whole file; the
/// first line of such a tail may then be a partial one.
const MAX_TAIL_BYTES: u64 = 8 * 1024 * 1024;

/// The last `lines` lines (at most `MAX_LOG_LINES`) of the file at `path`, read from its end
/// in blocks, so a large log is never read whole: at most `MAX_TAIL_BYTES`, and each block's
/// newlines are counted once. Bytes that are not UTF-8 are replaced, not an error. A file
/// that is not there is no lines.
pub fn tail(path: &std::path::Path, lines: usize) -> Result<Vec<String>, YardError> {
    use std::io::{Read, Seek, SeekFrom};
    let unreadable = |e: std::io::Error| YardError::internal(format!("cannot read {}: {e}", path.display()));
    let lines = lines.min(MAX_LOG_LINES);
    if lines == 0 {
        return Ok(vec![]);
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(unreadable(e)),
    };
    let size = file.metadata().map_err(unreadable)?.len();
    let (mut at, mut newlines) = (size, 0usize);
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    // One newline more than `lines` means the first line kept is a whole one.
    while at > 0 && newlines <= lines && size - at < MAX_TAIL_BYTES {
        let block = at.min(TAIL_BLOCK);
        at -= block;
        file.seek(SeekFrom::Start(at)).map_err(unreadable)?;
        let mut chunk = vec![0; block as usize];
        file.read_exact(&mut chunk).map_err(unreadable)?;
        newlines += chunk.iter().filter(|b| **b == b'\n').count();
        chunks.push(chunk);
    }
    let bytes: Vec<u8> = chunks.into_iter().rev().flatten().collect();
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    Ok(all[all.len().saturating_sub(lines)..].iter().map(|l| l.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_console_url_leaves_out_a_default_port() {
        assert_eq!(console_url("yard", false, 8080, 8443), "http://minio.yard:8080");
        assert_eq!(console_url("yard", true, 8080, 8443), "https://minio.yard:8443");
        assert_eq!(console_url("test", false, 80, 443), "http://minio.test");
        assert_eq!(console_url("test", true, 80, 443), "https://minio.test");
        // The scheme's own port decides: https on 8443 keeps it even when http is on 80.
        assert_eq!(console_url("test", true, 80, 8443), "https://minio.test:8443");
    }

    #[test]
    fn a_keg_is_the_last_segment_of_the_formula() {
        let brew = Brew::new("/opt/homebrew");
        assert_eq!(keg(&brew, "mysql@8.4"), Path::new("/opt/homebrew/opt/mysql@8.4"));
        assert_eq!(keg(&brew, "minio/stable/minio"), Path::new("/opt/homebrew/opt/minio"));
    }

    #[test]
    fn port_entries_name_each_port() {
        let names: Vec<(String, String, u16)> = port_entries(Engine::Minio, "minio");
        assert_eq!(
            names,
            [
                ("minio".to_string(), "minio".to_string(), 9000),
                ("minio-console".into(), "minio-console".into(), 9001)
            ]
        );
        assert_eq!(
            port_entries(Engine::Mysql, "mysql-8.4"),
            [("mysql-8.4".to_string(), "mysql".to_string(), 3306)]
        );
    }
}
