//! The daemon API: NDJSON JSON-RPC over `yardd.sock` (spec §6.2).

mod db;
mod emailulator;
mod engines;
mod ext;
mod handlers;
mod info;
mod node;
mod php;
mod project;
mod runtimes;
pub mod server;
mod services;
mod sources;
mod tunnels;
