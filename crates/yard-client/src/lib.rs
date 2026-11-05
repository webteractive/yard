//! The connection to yardd that the `yard` CLI and Yard.app share.

pub mod api;
pub mod client;
pub mod connection;

pub use client::{Client, not_running, protocol_version, stale};
pub use connection::Connection;
