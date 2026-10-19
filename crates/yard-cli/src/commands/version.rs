//! `yard version`, `--version` and `-V`: the CLI's build, plus yardd's and the helper's
//! when yardd answers.

use super::{Ctx, Outcome};
use crate::cli::LONG_VERSION;
use serde_json::Value;
use yard_proto::methods;
use yard_proto::{PROTOCOL_VERSION, StatusInfo, VersionInfo};

pub async fn version(ctx: &Ctx) -> Outcome {
    let (daemon, helper) = match ctx.client().await {
        Ok(mut client) => {
            let status: Option<StatusInfo> = client.call(methods::STATUS, Value::Null).await.ok();
            let daemon = format!("{} ({})", client.hello.daemon_version, client.hello.commit);
            (Some((client.hello.daemon_version.clone(), daemon)), status.and_then(|s| s.version.helper))
        }
        Err(_) => (None, None),
    };
    let info = VersionInfo {
        cli: Some(env!("CARGO_PKG_VERSION").into()),
        daemon: daemon.as_ref().map(|(v, _)| v.clone()),
        helper: helper.clone(),
        protocol: PROTOCOL_VERSION.into(),
        commit: env!("YARD_BUILD_COMMIT").into(),
        build_date: env!("YARD_BUILD_DATE").into(),
    };
    ctx.emit(&info, || {
        let daemon = daemon.map_or("yardd is not running".to_string(), |(_, line)| format!("yardd {line}"));
        let helper = helper.map_or("helper not reachable".to_string(), |v| format!("helper {v}"));
        format!("yard {LONG_VERSION}\n{daemon}\n{helper}\nprotocol {PROTOCOL_VERSION}\n")
    });
    Ok(0)
}
