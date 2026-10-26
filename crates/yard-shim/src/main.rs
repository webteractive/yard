//! `yard-shim`: `php`, `composer`, `pecl`, `phpize`, `php-config`, `node`, `npm` and `npx`
//! in Yard's `bin/`, chosen by `argv[0]` (spec §4.5, M4c spec §6). It resolves the version
//! for the current directory from `yard.toml`, `sites.toml` and the project's files, with no
//! daemon. A PHP shim `exec`s brew's binary with Yard's ini; a Node shim `exec`s the version
//! fnm installed into Yard's `node/`, and never runs fnm or downloads anything. Neither
//! PATH reaches Herd's `php` or `node`.

use std::convert::Infallible;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};
use yard_core::brew::Brew;
use yard_core::config::yard::parse_yard;
use yard_core::config::{SitesConfig, load_sites, load_yard};
use yard_core::node::{LtsMap, installed, pick};
use yard_core::shim::{NODE_SHIMS, Source, command, node_command, resolve_node, resolve_version};
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

/// The largest `.yard`, `.yard-php`, `.nvmrc`, `.node-version` or `lts.toml` the shim reads.
const FILE_MAX: u64 = 64 * 1024;

/// `file`'s text: `None` when nothing is there, and an error when it is there but not a
/// small regular text file, so the shim never falls through past it to other settings. It
/// never opens a FIFO, which would hang every `php` run below it. A directory is skipped:
/// the shim looks in every parent, and another tool may keep a `~/.yard` directory.
fn read_small(file: &Path) -> Option<Result<String, YardError>> {
    let unusable = |what: String| {
        let code = if file.file_name().is_some_and(|n| n == ".yard") {
            ErrorCode::ProjectFileInvalid
        } else {
            ErrorCode::InvalidArgument
        };
        Some(Err(YardError::new(code, format!("{} {what}", file.display()))
            .with_hint(format!("replace {} with a text file", file.display()))))
    };
    let meta = match std::fs::metadata(file) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return file
                .symlink_metadata()
                .is_ok()
                .then(|| unusable("is a symlink to nothing".into()))
                .flatten();
        }
        Err(e) => return unusable(format!("cannot be read: {e}")),
    };
    if meta.is_dir() {
        return None;
    }
    if !meta.is_file() {
        return unusable("is not a regular file".into());
    }
    if meta.len() > FILE_MAX {
        return unusable("is larger than 64 KiB".into());
    }
    match std::fs::read_to_string(file) {
        Ok(text) => Some(Ok(text)),
        Err(e) => unusable(format!("cannot be read: {e}")),
    }
}

fn run(name: &str, args: Vec<OsString>) -> Result<Infallible, YardError> {
    if NODE_SHIMS.contains(&name) {
        return run_node(name, args);
    }
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
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (v, source) = resolve_version(&cwd, &yard, &sites, &paths, &read_small, &canonical)?;
    if std::env::var_os("YARD_SHIM_DEBUG").is_some_and(|d| d == "1") {
        eprintln!("yard-shim: {name} runs PHP {v} ({source})");
    }
    let exists = |p: &Path| p.exists();
    let brew = Brew::from_env(&exists)?;
    let opt = brew.php_opt(v, &exists, &|p| std::fs::read_link(p).ok()).map_err(|e| match &source {
        // The project asks for it, so `yard init` there is what installs it.
        Source::Project(file) | Source::Managed(file) => YardError::new(
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

fn run_node(name: &str, args: Vec<OsString>) -> Result<Infallible, YardError> {
    let paths = Paths::from_env()?;
    let yard = match load_yard(&paths) {
        Ok((yard, _)) => yard,
        Err(e) => {
            eprintln!("yard: {}; using Node {}", e.message, yard_core::node::DEFAULT_SPEC);
            parse_yard(None, &paths.yard_toml())?.0
        }
    };
    let sites = load_sites(&paths).unwrap_or_else(|e| {
        eprintln!("yard: {}; ignoring site settings", e.message);
        SitesConfig::default()
    });
    let cwd = std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })?;
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (spec, source) = resolve_node(&cwd, &yard, &sites, &paths, &read_small, &canonical)?;
    let lts = read_small(&paths.lts_toml()).and_then(Result::ok).and_then(|t| LtsMap::parse(&t));
    let lts = lts.unwrap_or_else(LtsMap::builtin);
    let list = |dir: &Path| {
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    };
    let exists = |p: &Path| p.exists();
    let v = pick(&spec, &installed(&paths.node_versions_dir(), &list, &exists), &lts).ok_or_else(|| {
        YardError::new(ErrorCode::NodeMissing, format!("Node {spec} is not installed ({source})"))
            .with_hint(format!("yard node install {spec}"))
    })?;
    if std::env::var_os("YARD_SHIM_DEBUG").is_some_and(|d| d == "1") {
        eprintln!("yard-shim: {name} runs Node {v} ({spec} from {source})");
    }
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let extra_ca = std::env::var_os("NODE_EXTRA_CA_CERTS");
    let (program, pre, env) = node_command(name, v, &paths, &inherited, extra_ca.as_deref(), &exists);
    let e = Command::new(&program).args(pre).args(args).envs(env).exec();
    Err(YardError::new(ErrorCode::Internal, format!("cannot run {}: {e}", program.display())))
}
