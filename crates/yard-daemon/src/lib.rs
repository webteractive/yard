//! Yard's side effects: probing ports, spawning units, serving RPC.

pub mod daemon;
pub mod doctor;
pub mod events;
pub mod ext;
mod fdpass;
mod fds;
pub mod helper_client;
pub mod holder;
pub mod jobs;
pub mod logsink;
pub mod nginx;
pub mod portscan;
pub mod privileged;
pub mod probe;
pub mod reconcile;
pub mod rpc;
pub mod state;
pub mod supervisor;
pub mod watch;

/// Socket tests take this lock: macOS hands out ephemeral ports in sequence and lets a
/// `SO_REUSEADDR` bind land on a port in TIME_WAIT, so parallel tests can share a port.
#[cfg(test)]
pub(crate) static PORT_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One line for a log, a unit's `last_error` or a reconcile warning: `CODE: message (hint)`.
pub(crate) fn describe(e: &yard_proto::YardError) -> String {
    let code =
        serde_json::to_value(e.code).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    match &e.hint {
        Some(hint) => format!("{code}: {} ({hint})", e.message),
        None => format!("{code}: {}", e.message),
    }
}
