//! cloudflared, the first tunnel provider (tunnels spec §7): how its unit runs.

use super::Provider;
use crate::jobs::JobCtx;
use std::path::{Path, PathBuf};
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

/// A tunnel `tunnel list` shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Listed {
    pub id: String,
    pub name: String,
}

/// cloudflared's management commands for one domain's certificate (P5), run in a job so their
/// output streams and a cancel stops them.
pub struct Manage<'a> {
    pub job: &'a JobCtx,
    pub program: PathBuf,
    pub cert: PathBuf,
    /// The user's home: cloudflared puts an adopted tunnel's credentials under it.
    pub home: &'a Path,
}

impl Manage<'_> {
    fn env(&self) -> [(String, String); 1] {
        [("HOME".into(), self.home.display().to_string())]
    }

    fn args<'b>(&'b self, rest: &[&'b str]) -> Vec<&'b str> {
        let cert = self.cert.to_str().unwrap_or_default();
        let mut args = vec!["tunnel", "--origincert", cert];
        args.extend_from_slice(rest);
        args
    }

    /// The tunnel named `name` in the account, if any.
    pub async fn find(&self, name: &str) -> Result<Option<Listed>, YardError> {
        let args = self.args(&["list", "--name", name, "-o", "json"]);
        let (ok, lines) = self.job.capture(&self.program, &args, &self.env()).await?;
        let text = lines.join("\n");
        let json = match (text.find('['), text.rfind(']')) {
            (Some(a), Some(b)) if ok && a < b => &text[a..=b],
            _ => {
                return Err(YardError::new(
                    ErrorCode::JobFailed,
                    format!("cloudflared tunnel list --name {name} failed"),
                )
                .with_data(serde_json::json!({ "tail": lines })));
            }
        };
        let listed: Vec<Listed> = serde_json::from_str(json).map_err(|e| {
            YardError::new(ErrorCode::JobFailed, format!("cannot read cloudflared's tunnel list: {e}"))
        })?;
        Ok(listed.into_iter().find(|t| t.name == name))
    }

    pub async fn create(&self, name: &str, credentials: &Path) -> Result<(), YardError> {
        let credentials = credentials.to_str().unwrap_or_default();
        self.job
            .run(&self.program, &self.args(&["create", "--credentials-file", credentials, name]), &self.env())
            .await
    }

    /// `route dns`: a record that is already there is `TUNNEL_DNS_EXISTS` unless `overwrite`;
    /// any other failure is `TUNNEL_DNS_FAILED` with cloudflared's output.
    pub async fn route(
        &self,
        site: &str,
        id: &str,
        host: &str,
        domain: &str,
        overwrite: bool,
    ) -> Result<(), YardError> {
        let rest: Vec<&str> =
            if overwrite { vec!["route", "dns", "-f", id, host] } else { vec!["route", "dns", id, host] };
        let (ok, lines) = self.job.capture(&self.program, &self.args(&rest), &self.env()).await?;
        lines.iter().for_each(|l| self.job.line(l.clone()));
        if ok {
            return Ok(());
        }
        let output = lines.join("\n");
        if output.contains("already exists") {
            return Err(YardError::new(
                ErrorCode::TunnelDnsExists,
                format!("{host} already has a DNS record"),
            )
            .with_hint(format!("yard tunnel {site} --host {host} --overwrite-dns")));
        }
        Err(YardError::new(
            ErrorCode::TunnelDnsFailed,
            format!("cloudflared could not route {host} to the tunnel"),
        )
        .with_hint(format!("check that {domain} is in the Cloudflare account you signed in to"))
        .with_data(serde_json::json!({ "output": output })))
    }

    /// `cleanup` then `delete`: the tunnel's stale connections first, so the delete is not
    /// refused for them.
    pub async fn delete(&self, id: &str) -> Result<(), YardError> {
        self.job.run(&self.program, &self.args(&["cleanup", id]), &self.env()).await?;
        self.job.run(&self.program, &self.args(&["delete", id]), &self.env()).await
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
