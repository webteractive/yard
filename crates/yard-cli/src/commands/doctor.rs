//! `yard doctor`: yardd's checks when it runs, else the same checks in-process.

use super::{Ctx, Outcome};
use crate::client::CliError;
use crate::output::for_agents;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_core::brew::{Brew, DEFAULT_PREFIX};
use yard_core::paths::{HELPER_SOCKET, RESOLVER_DIR};
use yard_daemon::doctor::{self, DoctorInput};
use yard_daemon::helper_client::HelperClient;
use yard_proto::methods::{self, DoctorParams};
use yard_proto::{CheckStatus, DoctorReport};

/// Status column width plus the id column width, so continuation lines line up.
const ID_WIDTH: usize = 16;

pub async fn doctor(ctx: &Ctx) -> Outcome {
    let report: DoctorReport = match ctx.client().await {
        Ok(mut client) => client.call(methods::DOCTOR, DoctorParams { client_env: client_env() }).await?,
        Err(CliError::DaemonNotRunning) => offline(ctx).await,
        Err(e) => return Err(e),
    };
    ctx.emit(&report, || human(&report));
    Ok(doctor::exit_code(&report))
}

/// What this shell sets that brew reads and that yardd, started by launchd, may not have:
/// `XDG_CONFIG_HOME` (it moves brew's trust file) and every `HOMEBREW_FORBIDDEN_*`. Nothing
/// else leaves the shell: a forwarded variable could switch a brew check off as easily as on.
fn client_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| k == "XDG_CONFIG_HOME" || k.starts_with("HOMEBREW_FORBIDDEN_"))
        .collect()
}

async fn offline(ctx: &Ctx) -> DoctorReport {
    // A missing Homebrew is the brew check's to report, so fall back to its usual prefix.
    let brew = Brew::from_env(&|p: &Path| p.exists()).unwrap_or_else(|_| Brew::new(DEFAULT_PREFIX));
    let helper = HelperClient::new(HELPER_SOCKET);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    doctor::run(DoctorInput {
        paths: &ctx.paths,
        brew: &brew,
        helper: &helper,
        state: None,
        units: vec![],
        daemon_running: false,
        resolver_dir: Path::new(RESOLVER_DIR),
        security: &ctx.security,
        client_env: Default::default(),
        sleeping: Default::default(),
        home: &home,
        tunnels: vec![],
        resolve_host: yard_daemon::tunnel::resolves_with_getaddrinfo,
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
        // A space after the id always, so a long id (`project-file:<site>`) never runs into its message.
        let id_width = ID_WIDTH - 1;
        out.push_str(&format!("{status:<6}{:<id_width$} {}\n", c.id, lines.next().unwrap_or_default()));
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

#[cfg(test)]
mod tests {
    use super::human;
    use yard_proto::{Check, CheckStatus, DoctorReport};

    #[test]
    fn an_id_wider_than_its_column_keeps_a_space_before_the_message() {
        let check = |id: &str| Check {
            id: id.into(),
            status: CheckStatus::Ok,
            message: "the message".into(),
            hint: None,
        };
        let report = DoctorReport { checks: vec![check("brew"), check("project-file:all-about-circuits")] };
        let out = human(&report);
        assert!(out.contains("ok    brew            the message\n"), "{out}");
        assert!(out.contains("ok    project-file:all-about-circuits the message\n"), "{out}");
    }
}
