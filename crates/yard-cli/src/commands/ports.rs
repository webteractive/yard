//! `yard ports`, `ports check` (offline too) and the commands that change the registry.

use super::{Ctx, Outcome};
use crate::cli::{PortsAction, ProfileArg};
use crate::client::CliError;
use crate::output::{for_agents, table};
use serde_json::Value;
use std::collections::BTreeSet;
use yard_core::ports::{Registry, mode_str};
use yard_daemon::portscan;
use yard_proto::methods::{self, PortSetParams, ProfileParams, RangeParams, UnitParams};
use yard_proto::{PortInfo, PortMode, PortStatus, Profile, YardError};

pub async fn list(ctx: &Ctx) -> Outcome {
    let infos: Vec<PortInfo> = ctx.client().await?.call(methods::PORTS_LIST, Value::Null).await?;
    print(ctx, &infos, "yard ports");
    Ok(0)
}

/// The daemon's probe when it runs, else the same probe in-process. Exit 1 when a pinned
/// or preferred port is held by something else, the same rule `doctor` fails on.
pub async fn check(ctx: &Ctx) -> Outcome {
    let infos: Vec<PortInfo> = match ctx.client().await {
        Ok(mut client) => client.call(methods::PORTS_CHECK, Value::Null).await?,
        Err(CliError::DaemonNotRunning) => {
            let registry = Registry::load(&ctx.paths)?;
            tokio::task::spawn_blocking(move || portscan::infos(&registry, &BTreeSet::new()))
                .await
                .map_err(YardError::internal)?
        }
        Err(e) => return Err(e),
    };
    print(ctx, &infos, "yard ports check");
    let blocked = infos
        .iter()
        .any(|i| i.status == PortStatus::InUse && matches!(i.mode, PortMode::Pinned | PortMode::Preferred));
    Ok(i32::from(blocked))
}

fn print(ctx: &Ctx, infos: &[PortInfo], command: &str) {
    ctx.emit(&infos, || {
        let rows: Vec<Vec<String>> = infos
            .iter()
            .map(|i| {
                let holder =
                    i.holder.as_ref().map_or("-".to_string(), |h| format!("{} (pid {})", h.name, h.pid));
                vec![
                    i.unit.clone(),
                    i.port.to_string(),
                    mode_str(i.mode).into(),
                    status(i.status).into(),
                    holder,
                ]
            })
            .collect();
        let mut out = table(&["UNIT", "PORT", "MODE", "STATUS", "HOLDER"], &rows);
        out.push('\n');
        out.push_str(&for_agents(command));
        out
    });
}

fn status(s: PortStatus) -> &'static str {
    match s {
        PortStatus::Free => "free",
        PortStatus::Yard => "yard",
        PortStatus::InUse => "in use",
        PortStatus::ExternalListening => "listening",
        PortStatus::ExternalNotListening => "not listening",
    }
}

pub async fn change(ctx: &Ctx, action: PortsAction) -> Outcome {
    let mut client = ctx.client().await?;
    let done = match action {
        PortsAction::Check => unreachable!("ports check is routed to check()"),
        PortsAction::Set { unit, port } => {
            client.call::<_, ()>(methods::PORTS_SET, PortSetParams { unit: unit.clone(), port }).await?;
            format!("{unit} now uses port {port}.")
        }
        PortsAction::Pin { unit } => {
            client.call::<_, ()>(methods::PORTS_PIN, UnitParams { unit: unit.clone() }).await?;
            format!("{unit} is pinned to its port.")
        }
        PortsAction::Unpin { unit } => {
            client.call::<_, ()>(methods::PORTS_UNPIN, UnitParams { unit: unit.clone() }).await?;
            format!("{unit} may move to a free port.")
        }
        PortsAction::Range { range } => {
            client.call::<_, ()>(methods::PORTS_RANGE, RangeParams { range: range.clone() }).await?;
            format!("Automatic ports now come from {range}.")
        }
        PortsAction::Profile { profile } => {
            let profile = match profile {
                ProfileArg::Trial => Profile::Trial,
                ProfileArg::Default => Profile::Default,
            };
            client.call::<_, ()>(methods::PORTS_PROFILE, ProfileParams { profile }).await?;
            format!(
                "Switched to the {} profile: .{} on ports {} and {}.",
                if profile == Profile::Trial { "trial" } else { "default" },
                profile.tld(),
                profile.http_port(),
                profile.https_port()
            )
        }
    };
    ctx.emit(&(), || format!("{done} yard ports shows the registry.\n"));
    Ok(0)
}
