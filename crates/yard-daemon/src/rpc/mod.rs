//! The daemon API: NDJSON JSON-RPC over `yardd.sock` (spec §6.2).

mod ext;
mod handlers;
mod info;
mod node;
mod php;
mod project;
pub mod server;
mod services;
mod sources;
