//! Pure Yard logic: parse, render, decide. No spawning, binding or sockets here.

pub mod config;
pub mod paths;
pub mod php;
pub mod ports;

pub use paths::Paths;
pub use php::PhpVersion;
