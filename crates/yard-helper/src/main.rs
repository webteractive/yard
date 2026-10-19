//! `dev.more.yard.helper`: started by launchd as root with the arguments fixed at install.

use std::process::ExitCode;
use tokio::signal::unix::{SignalKind, signal};
use yard_helper::{HelperConfig, VERSION, serve};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!("yard-helper {VERSION}");
        return ExitCode::SUCCESS;
    }
    let cfg = match HelperConfig::from_args(args) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("yard-helper: {e}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("yard-helper: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(cfg, terminated())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yard-helper: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn terminated() {
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            term.recv().await;
        }
        Err(_) => std::future::pending().await,
    }
}
