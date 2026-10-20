//! `yard doctor`: yardd's checks when it runs, else the same checks in-process.

use super::{Ctx, Outcome};
use crate::client::CliError;
use crate::output::for_agents;
use serde_json::Value;
use std::path::Path;
use yard_core::brew::{Brew, DEFAULT_PREFIX};
use yard_core::paths::{HELPER_SOCKET, RESOLVER_DIR};
use yard_daemon::doctor::{self, DoctorInput};
use yard_daemon::helper_client::HelperClient;
use yard_proto::methods;
use yard_proto::{CheckStatus, DoctorReport};

/// Status column width plus the id column width, so continuation lines line up.
const ID_WIDTH: usize = 16;

pub async fn doctor(ctx: &Ctx) -> Outcome {
    let report: DoctorReport = match ctx.client().await {
        Ok(mut client) => client.call(methods::DOCTOR, Value::Null).await?,
        Err(CliError::DaemonNotRunning) => offline(ctx).await,
        Err(e) => return Err(e),
    };
    ctx.emit(&report, || human(&report));
    Ok(doctor::exit_code(&report))
}

async fn offline(ctx: &Ctx) -> DoctorReport {
    // A missing Homebrew is the brew check's to report, so fall back to its usual prefix.
    let brew = Brew::from_env(&|p: &Path| p.exists()).unwrap_or_else(|_| Brew::new(DEFAULT_PREFIX));
    let helper = HelperClient::new(HELPER_SOCKET);
    doctor::run(DoctorInput {
        paths: &ctx.paths,
        brew: &brew,
        helper: &helper,
        state: None,
        units: vec![],
        daemon_running: false,
        resolver_dir: Path::new(RESOLVER_DIR),
        security: &ctx.security,
    })
    .await
}

fn human(report: &DoctorReport) -> String {
    let indent = " ".repeat(6 + ID_WIDTH);
    let mut out = String::new();
    for c in &report.checks {
        let status = match c.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "FAIL",
        };
        let mut lines = c.message.lines();
        out.push_str(&format!("{status:<6}{:<ID_WIDTH$}{}\n", c.id, lines.next().unwrap_or_default()));
        for line in lines {
            out.push_str(&format!("{indent}{line}\n"));
        }
        if let Some(hint) = &c.hint {
            out.push_str(&format!("{indent}→ {hint}\n"));
        }
    }
    let count = |s: CheckStatus| report.checks.iter().filter(|c| c.status == s).count();
    let (failed, warned) = (count(CheckStatus::Fail), count(CheckStatus::Warn));
    out.push('\n');
    out.push_str(&match (failed, warned) {
        (0, 0) => "Everything checks out.\n".to_string(),
        (f, w) => format!("{f} failed, {w} warned.\n"),
    });
    out.push_str(&for_agents("yard doctor"));
    out
}
