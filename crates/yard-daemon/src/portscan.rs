//! Spec §7 check 3: foreign listeners on registered ports whose unit is idle. One
//! `PortConflict` per free → held transition, not one per scan.

use crate::daemon::Context;
use crate::reconcile::port_owner;
use crate::{holder, probe};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use yard_core::ports::Registry;
use yard_proto::{Event, PortInfo, PortMode, PortStatus, UnitInfo, UnitState};

/// Every registered port as `ports.list`, `ports check` and `doctor` show it. A port its
/// own unit holds (`own_units`) is `Yard`; any other holder is named. Blocks while it probes.
pub fn infos(registry: &Registry, running: &BTreeSet<String>) -> Vec<PortInfo> {
    registry
        .entries
        .iter()
        .map(|(unit, e)| {
            let owner = port_owner(unit);
            let (status, holder) = if e.mode == PortMode::External {
                let up = probe::is_listening(e.port);
                (if up { PortStatus::ExternalListening } else { PortStatus::ExternalNotListening }, None)
            } else if running.contains(owner) {
                (PortStatus::Yard, None)
            } else if probe::is_free(e.port, probe::proto_for(owner)) {
                (PortStatus::Free, None)
            } else {
                (PortStatus::InUse, holder::find_holder(e.port))
            };
            PortInfo {
                unit: unit.clone(),
                port: e.port,
                mode: e.mode,
                preferred: e.preferred,
                status,
                holder,
            }
        })
        .collect()
}

/// The units that own their registered ports now: running, or started and not yet
/// healthy. A starting nginx already holds 8080, and is not a foreign holder.
pub fn own_units(units: &[UnitInfo]) -> BTreeSet<String> {
    units
        .iter()
        .filter(|u| matches!(u.state, UnitState::Starting | UnitState::Running))
        .map(|u| u.name.clone())
        .collect()
}

pub fn spawn(ctx: Arc<Context>, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut held: BTreeMap<String, bool> = BTreeMap::new();
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            // emailulator's agent runs outside Yard, so only asking it shows a change (M4d).
            if ctx.reconciler.emailulator_moved().await
                && let Err(e) = ctx.reconciler.reconcile().await
            {
                tracing::warn!("reconcile after emailulator changed: {}", crate::describe(&e));
            }
            let Some(snapshot) = ctx.reconciler.snapshot().await else { continue };
            let busy_units = own_units(&ctx.supervisor().list());
            let idle: Vec<(String, u16)> = snapshot
                .registry
                .entries
                .iter()
                .filter(|(unit, e)| e.mode != PortMode::External && !busy_units.contains(port_owner(unit)))
                .map(|(unit, e)| (unit.clone(), e.port))
                .collect();
            held.retain(|unit, _| idle.iter().any(|(u, _)| u == unit));
            let probed = tokio::task::spawn_blocking(move || {
                idle.into_iter()
                    .map(|(unit, port)| {
                        let busy = !probe::is_free(port, probe::proto_for(port_owner(&unit)));
                        (unit, port, busy)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            for (unit, port, busy) in probed {
                let was = held.insert(unit.clone(), busy).unwrap_or(false);
                if busy && !was {
                    let holder =
                        tokio::task::spawn_blocking(move || holder::find_holder(port)).await.ok().flatten();
                    tracing::warn!("port {port} registered to {unit} is held by another process");
                    ctx.bus.emit(Event::PortConflict { unit, port, holder });
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::path::Path;

    #[test]
    fn a_starting_unit_owns_its_port() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let text = format!("[ports]\nnginx-http = {{ port = {port}, mode = \"pinned\" }}\n");
        let registry = Registry::parse(Some(&text), Path::new("/r/ports.toml")).unwrap();
        // nginx has started but not yet passed its first health check.
        let units = [UnitInfo {
            name: "nginx".into(),
            state: UnitState::Starting,
            pid: Some(1),
            restarts: 0,
            last_error: None,
        }];

        let infos = infos(&registry, &own_units(&units));
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].status, PortStatus::Yard, "{infos:?}");
    }

    #[test]
    fn a_running_minio_owns_its_console_port() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (api, console) =
            (TcpListener::bind("127.0.0.1:0").unwrap(), TcpListener::bind("127.0.0.1:0").unwrap());
        let text = format!(
            "[ports]\nminio = {{ port = {}, mode = \"pinned\" }}\nminio-console = {{ port = {}, mode = \"pinned\" }}\n",
            api.local_addr().unwrap().port(),
            console.local_addr().unwrap().port()
        );
        let registry = Registry::parse(Some(&text), Path::new("/r/ports.toml")).unwrap();
        let units = [UnitInfo {
            name: "minio".into(),
            state: UnitState::Running,
            pid: Some(1),
            restarts: 0,
            last_error: None,
        }];

        let infos = infos(&registry, &own_units(&units));
        assert_eq!(infos.len(), 2);
        assert!(infos.iter().all(|i| i.status == PortStatus::Yard), "{infos:?}");
    }
}
