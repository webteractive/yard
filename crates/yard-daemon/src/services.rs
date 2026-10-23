//! Service instances at run time: where each one's keg and unit command are, the orphans a
//! killed yardd left behind, and the `ServiceInfo` every service method answers with
//! (M4b spec §4 to §6, §15).

use crate::orphans;
use crate::reconcile::serves_console;
use crate::state::Snapshot;
use crate::supervisor::Supervisor;
use std::path::PathBuf;
use std::time::Duration;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::{ServicesConfig, load_services};
use yard_core::ports::{NGINX_HTTP, NGINX_HTTPS};
use yard_core::project::ServiceWant;
use yard_core::services::render::{UnitCommand, command};
use yard_core::services::{Engine, instance_name};
use yard_proto::{PortRef, ServiceInfo, ServiceState, UnitState};

/// How long a service gets to stop after its stop signal before SIGKILL: a database
/// finishes its checkpoint (spec §15).
pub const STOP_TIMEOUT: Duration = Duration::from_secs(30);

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
fn used_by(snapshot: &Snapshot, engine: Engine, instance: &str) -> Vec<String> {
    let services = &snapshot.services;
    snapshot
        .projects
        .iter()
        .filter_map(|(site, project)| Some((site, project.as_ref().ok()?.services.get(&engine)?)))
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
        match supervisor.list().into_iter().find(|u| u.name == name).map(|u| u.state) {
            Some(UnitState::Running) => ServiceState::Running,
            Some(UnitState::Starting | UnitState::Backoff { .. }) => ServiceState::Starting,
            Some(UnitState::Failed { last_lines }) => ServiceState::Failed { last_lines },
            Some(UnitState::Stopped) | None => ServiceState::Stopped,
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
    })
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
