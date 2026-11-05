//! The connection to yardd that the `yard` CLI and Yard.app share.

pub mod client;

pub use client::{Client, not_running, protocol_version};
