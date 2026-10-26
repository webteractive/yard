//! Wire types shared by `yardd`, `yard-helper`, the `yard` CLI and Yard.app.

pub mod error;
pub mod events;
pub mod helper;
pub mod methods;
pub mod rpc;
pub mod types;

pub use error::{ErrorCode, YardError};
pub use events::{Event, Topic};
pub use types::*;

/// Version of the JSON-RPC protocol spoken over `yardd.sock` and the helper socket.
/// Bump the minor for additive changes and the major for breaking ones.
pub const PROTOCOL_VERSION: &str = "2.8";
