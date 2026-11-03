//! `tunnels.*` (tunnels spec §8).

use crate::daemon::Context;
use crate::tunnel;
use std::sync::PoisonError;
use yard_proto::{TunnelInfo, YardError};

/// Every tunnel the last reconcile ran, by site.
pub async fn list(ctx: &Context) -> Result<Vec<TunnelInfo>, YardError> {
    let Some(snapshot) = ctx.reconciler.snapshot().await else { return Ok(vec![]) };
    let units = ctx.supervisor().list();
    let live = ctx.tunnels.live.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(snapshot
        .tunnels
        .iter()
        .map(|(site, planned)| {
            let unit = units.iter().find(|u| u.name == yard_core::tunnel::unit(site));
            tunnel::info(site, planned, unit, live.get(site))
        })
        .collect())
}
