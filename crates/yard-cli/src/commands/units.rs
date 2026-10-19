//! `yard status` and `yard restart`.

use super::{Ctx, Outcome};
use crate::output::{for_agents, table};
use serde_json::Value;
use yard_proto::methods::{self, UnitParams};
use yard_proto::{Profile, StatusInfo, UnitInfo, UnitState};

pub async fn status(ctx: &Ctx) -> Outcome {
    let info: StatusInfo = ctx.client().await?.call(methods::STATUS, Value::Null).await?;
    ctx.emit(&info, || {
        let profile = if info.profile == Profile::Trial { "trial" } else { "default" };
        let parked = if info.parked.is_empty() { "none".to_string() } else { info.parked.join(", ") };
        let config = match &info.config_error {
            None => "ok".to_string(),
            Some(e) => match &e.hint {
                Some(hint) => format!("{}\n          hint: {hint}", e.message),
                None => e.message.clone(),
            },
        };
        let mut out = format!(
            "Profile   {profile}\nTLD       .{}\nParked    {parked}\nSites     {}\nConfig    {config}\n\n",
            info.tld, info.sites
        );
        if info.units.is_empty() {
            out.push_str("No units yet; yardd starts them after its first reconcile.\n");
        } else {
            let rows: Vec<Vec<String>> = info.units.iter().map(unit_row).collect();
            out.push_str(&table(&["UNIT", "STATE", "PID", "RESTARTS"], &rows));
        }
        out.push('\n');
        out.push_str(&for_agents("yard status"));
        out
    });
    Ok(0)
}

fn unit_row(u: &UnitInfo) -> Vec<String> {
    let state = match &u.state {
        UnitState::Stopped => "stopped".to_string(),
        UnitState::Starting => "starting".to_string(),
        UnitState::Running => "running".to_string(),
        UnitState::Backoff { retry_in_ms } => {
            format!("backoff (retry in {:.1} s)", *retry_in_ms as f64 / 1000.0)
        }
        UnitState::Failed { .. } => "failed (yard doctor shows why)".to_string(),
    };
    let pid = u.pid.map_or("-".to_string(), |p| p.to_string());
    vec![u.name.clone(), state, pid, u.restarts.to_string()]
}

pub async fn restart(ctx: &Ctx, unit: String) -> Outcome {
    ctx.client().await?.call::<_, ()>(methods::UNITS_RESTART, UnitParams { unit: unit.clone() }).await?;
    ctx.emit(&(), || format!("Restarted {unit}.\n"));
    Ok(0)
}
