//! Wire types shared by `yardd`, `yard-helper`, the `yard` CLI and Yard.app.

/// Version of the JSON-RPC protocol spoken over `yardd.sock` and the helper socket.
/// Bump the minor for additive changes and the major for breaking ones.
pub const PROTOCOL_VERSION: &str = "1.0";
