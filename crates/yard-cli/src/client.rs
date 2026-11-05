//! The CLI's side of `yard_client`: exit 3 when yardd is not running.

use yard_core::Paths;
use yard_proto::{ErrorCode, YardError};

pub use yard_client::{Client, protocol_version};

#[derive(Debug)]
pub enum CliError {
    /// Nothing answers on the socket: exit 3, `yardd is not running → yard start`.
    DaemonNotRunning,
    Yard(YardError),
}

impl From<YardError> for CliError {
    fn from(e: YardError) -> Self {
        match e.code {
            ErrorCode::DaemonNotRunning => CliError::DaemonNotRunning,
            _ => CliError::Yard(e),
        }
    }
}

/// Connects to the yardd that serves `paths` as `yard <version>`.
pub async fn connect(paths: &Paths) -> Result<Client, CliError> {
    let name = format!("yard {}", env!("CARGO_PKG_VERSION"));
    Ok(Client::connect(&paths.daemon_socket(), &name).await?)
}
