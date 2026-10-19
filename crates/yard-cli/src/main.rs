mod cli;
mod client;
mod commands;
mod output;

use clap::Parser;
use cli::Cli;
use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let code = commands::run(Cli::parse()).await;
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
