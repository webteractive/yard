//! `yardd`, the per-user LaunchAgent (`dev.more.yard.daemon`). Logs go to stderr,
//! which launchd sends to `logs/yardd.log`.

use std::path::PathBuf;
use std::process::ExitCode;
use tokio::signal::unix::{SignalKind, signal};
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_daemon::daemon::{BUILD_DATE, COMMIT, DaemonOptions, VERSION, run};

const USAGE: &str = "usage: yardd [--root <path>]\n       yardd --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("yardd {VERSION} ({COMMIT}, {BUILD_DATE})");
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--root" => match it.next() {
                Some(path) => root = Some(PathBuf::from(path)),
                None => return usage("--root needs a path"),
            },
            other => return usage(&format!("unknown argument {other}")),
        }
    }
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_ansi(false).init();

    let paths = match root {
        Some(root) => Paths::new(root),
        None => match Paths::from_env() {
            Ok(p) => p,
            Err(e) => return fail(&e.message),
        },
    };
    let brew = match Brew::detect(&|p| p.exists()) {
        Ok(b) => b,
        Err(e) => return fail(&e.message),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return fail(&format!("cannot start the runtime: {e}")),
    };
    let opts = DaemonOptions::production(paths, brew.prefix);
    match runtime.block_on(run(opts, terminated())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e.message),
    }
}

/// SIGTERM (launchd, `yard stop`) or SIGINT (a terminal).
async fn terminated() {
    let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt()))
    else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

fn usage(message: &str) -> ExitCode {
    eprintln!("yardd: {message}\n{USAGE}");
    ExitCode::from(2)
}

fn fail(message: &str) -> ExitCode {
    tracing::error!("{message}");
    eprintln!("yardd: {message}");
    ExitCode::FAILURE
}
