//! `yard-shim`: `php`, `composer`, `pecl`, `phpize` and `php-config` in Yard's `bin/`,
//! chosen by `argv[0]` (spec §4.5). It resolves the PHP version for the current
//! directory from `yard.toml` and `sites.toml`, with no daemon, and `exec`s brew's binary
//! with Yard's ini and a PATH that never reaches Herd's `php`.

use std::convert::Infallible;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};
use yard_core::brew::Brew;
use yard_core::config::yard::parse_yard;
use yard_core::config::{SitesConfig, load_sites, load_yard};
use yard_core::shim::{Source, command, resolve_version};
use yard_core::{Paths, PhpVersion};
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

/// The largest `.yard` or `.yard-php` the shim reads.
const FILE_MAX: u64 = 64 * 1024;

/// `file` if it is a small regular file: a FIFO named `.yard` in a parent directory would
/// otherwise hang every `php` run below it.
fn read_small(file: &Path) -> Option<String> {
    let meta = std::fs::metadata(file).ok()?;
    if !meta.is_file() || meta.len() > FILE_MAX {
        return None;
    }
    std::fs::read_to_string(file).ok()
}

fn run(name: &str, args: Vec<OsString>) -> Result<Infallible, YardError> {
    let paths = Paths::from_env()?;
    // A TOML being edited must not stop every php on the machine: warn and carry on.
    let yard = match load_yard(&paths) {
        Ok((yard, _)) => yard,
        Err(e) => {
            eprintln!("yard: {}; using PHP {}", e.message, PhpVersion::DEFAULT);
            parse_yard(None, &paths.yard_toml())?.0
        }
    };
    let sites = load_sites(&paths).unwrap_or_else(|e| {
        eprintln!("yard: {}; ignoring site isolation", e.message);
        SitesConfig::default()
    });
    let cwd = std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })?;
    let (v, source) = resolve_version(&cwd, &yard, &sites, &read_small)?;
    if std::env::var_os("YARD_SHIM_DEBUG").is_some_and(|d| d == "1") {
        eprintln!("yard-shim: {name} runs PHP {v} ({source})");
    }
    let exists = |p: &Path| p.exists();
    let brew = Brew::from_env(&exists)?;
    let opt = brew.php_opt(v, &exists, &|p| std::fs::read_link(p).ok()).map_err(|e| match &source {
        // The project asks for it, so `yard init` there is what installs it.
        Source::Project(file) => YardError::new(
            ErrorCode::FormulaMissing,
            format!("PHP {v} is not installed ({})", file.display()),
        )
        .with_hint("yard init"),
        _ => e,
    })?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let (program, pre, env) = command(name, v, &opt, &brew, &paths, &inherited, &exists)?;
    let e = Command::new(&program).args(pre).args(args).envs(env).exec();
    Err(YardError::new(ErrorCode::Internal, format!("cannot run {}: {e}", program.display())))
}
