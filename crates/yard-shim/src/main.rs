//! `yard-shim`: `php`, `composer`, `pecl`, `phpize` and `php-config` in Yard's `bin/`,
//! chosen by `argv[0]` (spec §4.5). It resolves the PHP version for the current
//! directory from `yard.toml` and `sites.toml`, with no daemon, and `exec`s brew's binary
//! with Yard's ini and a PATH that never reaches Herd's `php`.

use std::convert::Infallible;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::{load_sites, load_yard};
use yard_core::shim::{command, resolve_version};
use yard_proto::{ErrorCode, YardError};

fn main() -> ExitCode {
    let mut argv = std::env::args_os();
    let argv0 = argv.next().unwrap_or_default();
    let name = Path::new(&argv0).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Err(e) = run(&name, argv.collect());
    eprintln!("yard: {}", e.message);
    if let Some(hint) = &e.hint {
        eprintln!("  hint: {hint}");
    }
    ExitCode::from(1)
}

fn run(name: &str, args: Vec<OsString>) -> Result<Infallible, YardError> {
    let paths = Paths::from_env()?;
    let (yard, _) = load_yard(&paths)?;
    let sites = load_sites(&paths)?;
    let cwd = std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })?;
    let (v, source) = resolve_version(&cwd, &yard, &sites, &|p| std::fs::read_to_string(p).ok())?;
    if std::env::var_os("YARD_SHIM_DEBUG").is_some_and(|d| d == "1") {
        eprintln!("yard-shim: {name} runs PHP {v} ({source})");
    }
    let exists = |p: &Path| p.exists();
    // Tests point the shim at a fake prefix; everyone else has brew at /opt/homebrew.
    let brew = match std::env::var_os("YARD_BREW_PREFIX") {
        Some(prefix) => Brew::new(PathBuf::from(prefix)),
        None => Brew::detect(&exists)?,
    };
    let opt = brew.php_opt(v, &exists, &|p| std::fs::read_link(p).ok())?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let (program, pre, env) = command(name, v, &opt, &brew, &paths, &inherited, &exists)?;
    let e = Command::new(&program).args(pre).args(args).envs(env).exec();
    Err(YardError::new(ErrorCode::Internal, format!("cannot run {}: {e}", program.display())))
}
