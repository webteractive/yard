//! cloudflared, the first tunnel provider (tunnels spec §7): how its unit runs.

use super::Provider;
use std::path::Path;
use yard_core::brew::Brew;
use yard_core::services::render::UnitCommand;
use yard_core::shim::child_path;
use yard_proto::TunnelKind;

/// The formula `brew install` names when cloudflared is missing.
pub const FORMULA: &str = "cloudflared";

pub struct Cloudflared;

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
