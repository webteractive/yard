//! The connection to yardd that the `yard` CLI and Yard.app share.

pub mod api;
pub mod client;
pub mod connection;
pub mod session;

pub use client::{Client, not_running, protocol_version, stale};
pub use connection::Connection;
pub use session::{ConnectionState, LogStream, Session, SessionConfig, SessionInfo, Update};
