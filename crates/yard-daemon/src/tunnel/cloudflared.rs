//! cloudflared, the first tunnel provider (tunnels spec §7): how its unit runs.

use super::Provider;
use crate::jobs::JobCtx;
use std::path::Path;
use yard_core::brew::Brew;
use yard_core::services::render::UnitCommand;
use yard_core::shim::child_path;
use yard_core::tunnel::cloudflared::{cert_dir, cert_path};
use yard_proto::{ErrorCode, TunnelKind, YardError};

/// The formula `brew install` names when cloudflared is missing.
pub const FORMULA: &str = "cloudflared";

pub struct Cloudflared;

/// `TUNNEL_NOT_LOGGED_IN`: `domain` has no certificate under `home`.
pub fn not_logged_in(domain: &str) -> YardError {
    YardError::new(ErrorCode::TunnelNotLoggedIn, format!("cloudflared is not signed in for {domain}"))
        .with_hint(format!("yard tunnels domain add {domain}"))
}

/// Signs cloudflared in for `domain` (P5): `tunnel login` with `HOME` at the domain's own
/// directory, so the certificate it writes is that domain's. Nothing runs when the certificate is
/// already there. Yard never reads the certificate; it only checks that it exists.
pub async fn login(job: &JobCtx, brew: &Brew, home: &Path, domain: &str) -> Result<(), YardError> {
    if cert_path(home, domain).exists() {
        job.line(format!("Already signed in for {domain}."));
        return Ok(());
    }
    let dir = cert_dir(home, domain);
    std::fs::create_dir_all(&dir)
        .map_err(|e| YardError::internal(format!("cannot create {}: {e}", dir.display())))?;
    job.line(format!("In the browser, pick the zone {domain}."));
    let ran = job
        .run(&brew.cloudflared_bin(), &["tunnel", "login"], &[("HOME".into(), dir.display().to_string())])
        .await;
    match ran {
        Err(e) if e.code == ErrorCode::JobCancelled => Err(e),
        _ if cert_path(home, domain).exists() => Ok(()),
        // The tail of what cloudflared printed, when it failed.
        Err(failed) => Err(not_logged_in(domain).with_data(failed.data.unwrap_or_default())),
        Ok(()) => Err(not_logged_in(domain)),
    }
}

impl Provider for Cloudflared {
    /// `tunnel --config <yml>`, and `run` for a named tunnel; a quick one starts from the yml's
    /// `url`. `HOME` is where cloudflared looks for `~/.cloudflared`; nothing else is inherited
    /// but PATH, without Herd.
    fn unit_command(&self, brew: &Brew, home: &Path, yml: &Path, kind: TunnelKind) -> UnitCommand {
        let mut args = vec!["tunnel".into(), "--config".into(), yml.into()];
        if kind == TunnelKind::Named {
            args.push("run".into());
        }
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        UnitCommand {
            program: brew.cloudflared_bin(),
            args,
            env: vec![
                ("HOME".into(), home.display().to_string()),
                ("PATH".into(), child_path(None, &inherited).to_string_lossy().into_owned()),
            ],
            stop_signal: libc::SIGTERM,
        }
    }

    /// Cloudflare's edge sets it to the visitor's address.
    fn real_ip_header(&self) -> &'static str {
        "CF-Connecting-IP"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn a_named_tunnel_runs_and_a_quick_one_does_not() {
        let brew = Brew::new("/opt/homebrew");
        let yml = Path::new("/r/tunnels/blog.yml");
        let quick = Cloudflared.unit_command(&brew, Path::new("/Users/me"), yml, TunnelKind::Quick);
        assert_eq!(quick.program, Path::new("/opt/homebrew/opt/cloudflared/bin/cloudflared"));
        let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(quick.args, args(&["tunnel", "--config", "/r/tunnels/blog.yml"]));
        assert!(quick.env.contains(&("HOME".into(), "/Users/me".into())));
        assert_eq!(quick.env.len(), 2);
        let named = Cloudflared.unit_command(&brew, Path::new("/Users/me"), yml, TunnelKind::Named);
        assert_eq!(named.args, args(&["tunnel", "--config", "/r/tunnels/blog.yml", "run"]));
    }
}
