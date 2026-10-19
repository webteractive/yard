//! Command-line surface of `yard`. Parsing only; commands live in `commands/`.

use clap::{Parser, Subcommand};

/// `0.1.0 (abc1234, 2026-10-19)` — clap prefixes the binary name.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("YARD_BUILD_COMMIT"),
    ", ",
    env!("YARD_BUILD_DATE"),
    ")"
);

#[derive(Debug, Parser)]
#[command(
    name = "yard",
    version = LONG_VERSION,
    about = "Local PHP development environment for macOS",
    after_help = "For agents and scripts: every command accepts --json."
)]
pub struct Cli {
    /// Print machine-readable JSON instead of human output.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show the CLI, daemon, helper and protocol versions.
    Version,
}
