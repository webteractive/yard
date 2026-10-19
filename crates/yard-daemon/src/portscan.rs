//! Spec §7 check 3: foreign listeners on registered ports whose unit is idle. One
//! `PortConflict` per free → held transition, not one per scan.

use crate::daemon::Context;
use crate::reconcile::port_owner;
use crate::{holder, probe};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use yard_proto::{Event, PortMode, UnitState};

pub fn spawn(ctx: Arc<Context>, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut held: BTreeMap<String, bool> = BTreeMap::new();
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            let Some(snapshot) = ctx.reconciler.snapshot().await else { continue };
            // A starting or running unit is expected to hold its port.
            let busy_units: BTreeSet<String> = ctx
                .supervisor()
                .list()
                .into_iter()
                .filter(|u| matches!(u.state, UnitState::Starting | UnitState::Running))
                .map(|u| u.name)
                .collect();
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
