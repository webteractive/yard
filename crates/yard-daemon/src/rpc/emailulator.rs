//! `services.*` for emailulator, which runs as its own LaunchAgent (M4d spec §4, option B):
//! `add` installs that agent when none serves mail and records where the binary is; every
//! other lifecycle method is `SERVICE_EXTERNAL`, naming emailulator's own command.

use super::handlers::snapshot;
use crate::daemon::Context;
use crate::emailulator::{find_and_probe, is_exec, serving_agent};
use crate::reconcile::serves_mail;
use crate::services::{host_url, tail};
use crate::state::Snapshot;
use std::path::PathBuf;
use yard_core::config::TomlFile;
use yard_core::emailulator::{HTTP_UNIT, LABEL, MAIL_HOST_LABEL, NAME, SMTP_UNIT, locate};
use yard_core::ports::{NGINX_HTTP, NGINX_HTTPS};
use yard_proto::methods::ServiceAddParams;
use yard_proto::{
    Account, ErrorCode, JobStarted, PortRef, ServiceInfo, ServiceManager, ServiceState, SettingSource,
    YardError,
};

/// Whether a `services.*` method names emailulator rather than an instance Yard runs.
pub(super) fn is_emailulator(name: &str) -> bool {
    name == NAME
}

/// `services.add emailulator`: always a job. It records the binary as `[emailulator] path`,
/// installs the agent unless one already catches mail, and reconciles so the registry and
/// `mail.<tld>` follow it.
pub(super) async fn add(ctx: &Context, p: ServiceAddParams) -> Result<JobStarted, YardError> {
    if p.version.is_some() {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            "emailulator has no versions; it updates itself",
        )
        .with_hint("emailulator self-update"));
    }
    if p.port == Some(0) {
        return Err(YardError::new(ErrorCode::InvalidArgument, "0 is not a port")
            .with_hint("pass a port such as --port 2525"));
    }
    // The CLI's path (from the user's PATH) first, then the recorded one, then the installer's.
    let given = p.path.as_deref().map(PathBuf::from).filter(|p| p.is_absolute());
    let snap = snapshot(ctx).await?;
    let recorded = snap.services.emailulator.as_ref().and_then(|e| e.path.clone());
    let bin = locate(given.as_deref().or(recorded.as_deref()), ctx.reconciler.home(), &is_exec)?;
    let (services_toml, reconciler, port) = (ctx.paths.services_toml(), ctx.reconciler.clone(), p.port);
    Ok(ctx.jobs.start("service-add", move |job| async move {
        let path = bin.display().to_string();
        TomlFile::edit(&services_toml, |file| {
            file.set(&[NAME, "path"], toml_edit::value(path.clone()));
            Ok(())
        })?;
        let probed = find_and_probe(Some(&bin), reconciler.home(), None).await;
        if let Some(agent) = serving_agent(probed.as_ref()) {
            job.line(format!(
                "emailulator's LaunchAgent {} already catches mail on {}.",
                agent.label, agent.smtp
            ));
            reconciler.reconcile().await?;
            return Ok(());
        }
        let smtp = port.map(|port| format!("127.0.0.1:{port}"));
        let mut args = vec!["service", "install"];
        if let Some(smtp) = &smtp {
            args.extend(["--", "--smtp-addr", smtp]);
        }
        job.run(&bin, &args, &[]).await?;
        reconciler.reconcile().await?;
        let now = reconciler.snapshot().await.and_then(|snap| snap.emailulator);
        if serving_agent(now.as_ref()).is_none() {
            return Err(YardError::new(
                ErrorCode::JobFailed,
                "emailulator service install finished, but its LaunchAgent is not loaded",
            )
            .with_hint("emailulator service status"));
        }
        Ok(())
    }))
}

/// `start|stop|restart|port|sleep.set|remove emailulator`: Yard never runs it.
pub(super) async fn refuse(ctx: &Context, removing: bool) -> YardError {
    let snap = match snapshot(ctx).await {
        Ok(snap) => snap,
        Err(e) => return e,
    };
    let agent = snap.emailulator.as_ref().and_then(|p| p.agent.as_ref());
    let label = agent.map_or(LABEL, |a| a.label.as_str());
    let pid = agent.and_then(|a| a.pid);
    let hint = if removing { "emailulator service uninstall" } else { "emailulator service status" };
    YardError::new(
        ErrorCode::ServiceExternal,
        format!("emailulator is run by its LaunchAgent {label}; Yard does not start or stop it"),
    )
    .with_hint(hint)
    .with_data(serde_json::json!({ "label": label, "pid": pid }))
}

/// `services.logs emailulator`: the tail of the log its agent names, read only.
pub(super) async fn logs(ctx: &Context, lines: usize) -> Result<Vec<String>, YardError> {
    let snap = snapshot(ctx).await?;
    let agent = snap.emailulator.as_ref().and_then(|p| p.agent.as_ref()).filter(|a| a.installed);
    let Some(agent) = agent else {
        return Err(YardError::new(ErrorCode::ServiceNotFound, "emailulator's LaunchAgent is not installed")
            .with_hint("yard service add emailulator"));
    };
    let Some(log) = agent.log.clone() else { return Ok(vec![]) };
    tokio::task::spawn_blocking(move || tail(&log, lines)).await.map_err(YardError::internal)?
}

/// The `services.list` row while emailulator's LaunchAgent is installed: answering is
/// running, loaded and not answering is failed, and not loaded is stopped.
pub(super) fn info(snap: &Snapshot) -> Option<ServiceInfo> {
    let probe = snap.emailulator.as_ref()?;
    let agent = probe.agent.as_ref().filter(|a| a.installed)?;
    let state = if agent.answering {
        ServiceState::Running
    } else if agent.loaded {
        let reason = YardError::new(
            ErrorCode::ServiceExternal,
            format!("emailulator's LaunchAgent {} is loaded but does not answer", agent.label),
        )
        .with_hint("emailulator service status");
        ServiceState::Failed { last_lines: vec![], reason: Some(reason) }
    } else {
        ServiceState::Stopped
    };
    let console = (serving_agent(Some(probe)).is_some() && serves_mail(&snap.resolution.sites)).then(|| {
        let profile = snap.registry.profile;
        let port_of = |unit: &str, usual: u16| snap.registry.entries.get(unit).map_or(usual, |e| e.port);
        let http = port_of(NGINX_HTTP, profile.http_port());
        let https = port_of(NGINX_HTTPS, profile.https_port());
        host_url(MAIL_HOST_LABEL, &snap.tld, snap.yard.secure_by_default, http, https)
    });
    Some(ServiceInfo {
        name: NAME.to_string(),
        engine: NAME.to_string(),
        version: probe.version.map(|(major, minor, patch)| format!("{major}.{minor}.{patch}")),
        // It comes from its installer, not brew: where it is stands in for the formula.
        formula: probe.bin.display().to_string(),
        ports: vec![
            PortRef { name: SMTP_UNIT.to_string(), port: agent.smtp.port() },
            PortRef { name: HTTP_UNIT.to_string(), port: agent.http.port() },
        ],
        state,
        stopped_by_user: false,
        account: Account { user: None, password: None },
        console,
        used_by: snap
            .projects
            .iter()
            .filter(|(_, project)| project.ok().is_some_and(|file| file.emailulator))
            .map(|(site, _)| site.clone())
            .collect(),
        sleeping_since: None,
        sleep_after_secs: None,
        sleep_after_source: SettingSource::Default,
        managed_by: ServiceManager::LaunchAgent { label: agent.label.clone(), pid: agent.pid },
    })
}
