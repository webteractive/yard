//! launchd property lists for `yardd` (LaunchAgent) and the helper (LaunchDaemon).

use crate::paths::{DAEMON_LABEL, HELPER_LABEL};
use plist::{Dictionary, Value};
use std::path::Path;

fn s(p: &Path) -> Value {
    Value::String(p.display().to_string())
}

fn to_xml(dict: Dictionary) -> Vec<u8> {
    let mut out = Vec::new();
    // Writing a dictionary of strings, bools and arrays into memory cannot fail.
    if let Err(e) = Value::Dictionary(dict).to_writer_xml(&mut out) {
        unreachable!("plist serialization failed: {e}");
    }
    out
}

/// Restarted by launchd on crash, but not after a clean `yard stop`.
pub fn daemon_plist(yardd: &Path, root: Option<&Path>, log: &Path) -> Vec<u8> {
    let mut d = Dictionary::new();
    d.insert("Label".into(), DAEMON_LABEL.into());
    d.insert("ProgramArguments".into(), Value::Array(vec![s(yardd)]));
    d.insert("RunAtLoad".into(), true.into());
    let mut keep_alive = Dictionary::new();
    keep_alive.insert("SuccessfulExit".into(), false.into());
    d.insert("KeepAlive".into(), Value::Dictionary(keep_alive));
    d.insert("ProcessType".into(), "Interactive".into());
    d.insert("StandardOutPath".into(), s(log));
    d.insert("StandardErrorPath".into(), s(log));
    if let Some(root) = root {
        let mut env = Dictionary::new();
        env.insert("YARD_ROOT".into(), s(root));
        d.insert("EnvironmentVariables".into(), Value::Dictionary(env));
    }
    to_xml(d)
}

/// Everything the helper trusts is fixed here at install time, never sent by a caller.
pub fn helper_plist(binary: &Path, uid: u32, user: &str, root: &Path, brew_prefix: &Path) -> Vec<u8> {
    let mut d = Dictionary::new();
    d.insert("Label".into(), HELPER_LABEL.into());
    let args = vec![
        s(binary),
        "--uid".into(),
        uid.to_string().into(),
        "--user".into(),
        user.into(),
        "--root".into(),
        s(root),
        "--brew-prefix".into(),
        s(brew_prefix),
    ];
    d.insert("ProgramArguments".into(), Value::Array(args));
    d.insert("RunAtLoad".into(), true.into());
    d.insert("KeepAlive".into(), true.into());
    d.insert("StandardErrorPath".into(), format!("/var/log/{HELPER_LABEL}.log").into());
    to_xml(d)
}
