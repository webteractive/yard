//! Yard's side effects: probing ports, spawning units, serving RPC.

pub mod helper_client;
pub mod holder;
pub mod probe;

/// Socket tests take this lock: macOS hands out ephemeral ports in sequence and lets a
/// `SO_REUSEADDR` bind land on a port in TIME_WAIT, so parallel tests can share a port.
#[cfg(test)]
pub(crate) static PORT_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
