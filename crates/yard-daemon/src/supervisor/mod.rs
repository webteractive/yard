//! Keeps the desired units running (spec §6.1). One tokio task per unit; `apply`
//! diffs the desired set against what runs.

mod unit;

use crate::events::EventBus;
use crate::helper_client::HelperClient;
use crate::logsink::LogSink;
use crate::probe::Proto;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use unit::{Control, Runner};
use yard_core::supervise::PolicyConfig;
use yard_proto::{ErrorCode, UnitInfo, UnitState, YardError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Tcp(u16),
    Unix(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitKind {
    Process {
        program: PathBuf,
        args: Vec<OsString>,
        env: Vec<(String, String)>,
    },
    /// Started, reloaded and stopped through the root helper.
    Nginx,
}

/// A changed spec means a restart, so equality is the whole diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSpec {
    pub name: String,
    pub kind: UnitKind,
    pub health: Health,
    /// Checked with a bind probe before every start (spec §7, check 2).
    pub port: Option<(u16, Proto)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub policy: PolicyConfig,
    pub health_interval: Duration,
    pub start_grace: Duration,
    pub stop_timeout: Duration,
    pub log_max_bytes: u64,
    pub log_keep: usize,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            policy: PolicyConfig::default(),
            health_interval: Duration::from_secs(5),
            start_grace: Duration::from_secs(10),
            stop_timeout: Duration::from_secs(5),
            log_max_bytes: 10 * 1024 * 1024,
            log_keep: 5,
        }
    }
}

pub(crate) type Table = Arc<Mutex<BTreeMap<String, UnitInfo>>>;

struct Handle {
    spec: UnitSpec,
    control: mpsc::UnboundedSender<Control>,
    task: JoinHandle<()>,
}

pub struct Supervisor {
    cfg: SupervisorConfig,
    logs_dir: PathBuf,
    helper: HelperClient,
    bus: EventBus,
    table: Table,
    units: tokio::sync::Mutex<BTreeMap<String, Handle>>,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig, logs_dir: PathBuf, helper: HelperClient, bus: EventBus) -> Self {
        Self {
            cfg,
            logs_dir,
            helper,
            bus,
            table: Arc::default(),
            units: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Starts new units, stops removed ones and restarts the ones whose spec changed.
    pub async fn apply(&self, desired: Vec<UnitSpec>) {
        let mut units = self.units.lock().await;
        let wanted: BTreeMap<&str, &UnitSpec> = desired.iter().map(|s| (s.name.as_str(), s)).collect();
        let stale: Vec<String> = units
            .iter()
            .filter(|(name, h)| wanted.get(name.as_str()).is_none_or(|spec| **spec != h.spec))
            .map(|(name, _)| name.clone())
            .collect();
        for name in stale {
            if let Some(handle) = units.remove(&name) {
                stop(handle).await;
            }
            if !wanted.contains_key(name.as_str()) {
                lock(&self.table).remove(&name);
            }
        }
        for spec in desired {
            if !units.contains_key(&spec.name) {
                let handle = self.spawn(spec);
                units.insert(handle.spec.name.clone(), handle);
            }
        }
    }

    /// `yard restart <unit>`: forget past failures and start again, whatever the state.
    pub async fn restart(&self, name: &str) -> Result<(), YardError> {
        let units = self.units.lock().await;
        let handle = units.get(name).ok_or_else(|| no_unit(name))?;
        handle.control.send(Control::Restart).map_err(|_| no_unit(name))
    }

    /// nginx reloads through the helper and php-fpm on SIGUSR2; other units restart.
    pub async fn reload(&self, name: &str) -> Result<(), YardError> {
        let kind = {
            let units = self.units.lock().await;
            units.get(name).ok_or_else(|| no_unit(name))?.spec.kind.clone()
        };
        match kind {
            UnitKind::Nginx => self.helper.reload_nginx().await,
            UnitKind::Process { .. } if name.starts_with("php-") => {
                let pid = lock(&self.table).get(name).and_then(|u| u.pid);
                match pid {
                    Some(pid) => unit::signal(pid, libc::SIGUSR2),
                    None => self.restart(name).await,
                }
            }
            UnitKind::Process { .. } => self.restart(name).await,
        }
    }

    pub fn list(&self) -> Vec<UnitInfo> {
        lock(&self.table).values().cloned().collect()
    }

    /// Stops every unit and waits for each to exit. The units stay listed as stopped.
    pub async fn shutdown(&self) {
        let mut units = self.units.lock().await;
        for (_, handle) in std::mem::take(&mut *units) {
            stop(handle).await;
        }
    }

    fn spawn(&self, spec: UnitSpec) -> Handle {
        let path = self.logs_dir.join(format!("{}.log", spec.name));
        let (max, keep) = (self.cfg.log_max_bytes, self.cfg.log_keep);
        let sink = LogSink::open(&path, max, keep, spec.name.clone(), self.bus.clone()).unwrap_or_else(|e| {
            eprintln!("yardd: cannot open {}: {e}; {} logs only to memory", path.display(), spec.name);
            LogSink::detached(&path, max, keep, spec.name.clone(), self.bus.clone())
        });
        lock(&self.table).insert(
            spec.name.clone(),
            UnitInfo {
                name: spec.name.clone(),
                state: UnitState::Stopped,
                pid: None,
                restarts: 0,
                last_error: None,
            },
        );
        let (control, rx) = mpsc::unbounded_channel();
        let runner = Runner::new(
            spec.clone(),
            self.cfg,
            self.helper.clone(),
            self.bus.clone(),
            sink,
            self.table.clone(),
            rx,
        );
        Handle { spec, control, task: tokio::spawn(runner.run()) }
    }
}

async fn stop(handle: Handle) {
    let (ack, done) = oneshot::channel();
    if handle.control.send(Control::Stop(ack)).is_ok() {
        let _ = done.await;
    }
    let _ = handle.task.await;
}

fn no_unit(name: &str) -> YardError {
    YardError::new(ErrorCode::InvalidArgument, format!("no unit named {name:?}"))
        .with_hint("`yard status` lists the units")
}

/// A poisoned table is still a consistent map; keep serving it.
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
