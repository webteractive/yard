//! launchd property lists for `yardd` (LaunchAgent) and the helper (LaunchDaemon).

use crate::paths::{DAEMON_LABEL, HELPER_LABEL};
use plist::{Dictionary, Value};
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

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

/// What Yard.app needs from an installed daemon agent: the `yardd` it runs (so the `yard` beside
/// it can be found) and the data root it gives it, if not the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFacts {
    pub program: PathBuf,
    pub root: Option<PathBuf>,
}

/// Reads back the plist [`daemon_plist`] writes: `ProgramArguments[0]` and
/// `EnvironmentVariables.YARD_ROOT`. Anything else is `CONFIG_INVALID` naming what is missing.
pub fn read_agent(bytes: &[u8]) -> Result<AgentFacts, YardError> {
    let invalid = |what: &str| {
        YardError::new(ErrorCode::ConfigInvalid, format!("the yardd LaunchAgent plist {what}"))
            .with_hint("yard start rewrites it")
    };
    let value = Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| invalid(&format!("does not parse: {e}")))?;
    let dict = value.as_dictionary().ok_or_else(|| invalid("is not a dictionary"))?;
    let program = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_string)
        .ok_or_else(|| invalid("has no ProgramArguments"))?;
    let root = dict
        .get("EnvironmentVariables")
        .and_then(Value::as_dictionary)
        .and_then(|env| env.get("YARD_ROOT"))
        .and_then(Value::as_string)
        .map(PathBuf::from);
    Ok(AgentFacts { program: PathBuf::from(program), root })
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

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::ErrorCode;

    #[test]
    fn read_agent_reads_what_render_writes() {
        let log = Path::new("/r/logs/yardd.log");
        let yardd = Path::new("/Users/me/AI/yard/target/release/yardd");
        let plain = read_agent(&daemon_plist(yardd, None, log)).unwrap();
        assert_eq!(plain, AgentFacts { program: yardd.to_path_buf(), root: None });
        let rooted = read_agent(&daemon_plist(yardd, Some(Path::new("/tmp/yard dev")), log)).unwrap();
        assert_eq!(rooted.root.as_deref(), Some(Path::new("/tmp/yard dev")));
    }

    #[test]
    fn read_agent_names_a_missing_key() {
        let mut d = Dictionary::new();
        d.insert("Label".into(), DAEMON_LABEL.into());
        let e = read_agent(&to_xml(d)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.message.contains("ProgramArguments"), "{}", e.message);
        assert!(read_agent(b"not a plist").is_err());
    }
}
