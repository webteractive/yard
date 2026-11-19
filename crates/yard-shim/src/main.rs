//! `yard-shim`: `php`, `composer`, `pecl`, `phpize`, `php-config`, `node`, `npm` and `npx`
//! in Yard's `bin/`, chosen by `argv[0]` (spec §4.5, M4c spec §6). It resolves the version
//! for the current directory from `yard.toml`, `sites.toml` and the project's files, with no
//! daemon. A PHP shim `exec`s brew's binary with Yard's ini; `composer` runs the Composer
//! Yard installed that the project's PHP can run, under that PHP (runtimes spec §9); `php`
//! and `composer` first ask yardd to wake their site's services when `run/sleeping` lists
//! it (M4e spec §5.2). A
//! Node shim `exec`s the version fnm installed into Yard's `node/`, and never runs fnm or
//! downloads anything. Neither PATH reaches Herd's `php` or `node`.

use std::convert::Infallible;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;
use yard_core::brew::Brew;
use yard_core::config::yard::load_yard_for_shim;
use yard_core::config::yard::parse_yard;
use yard_core::config::{SiteEntry, SitesConfig, YardConfig, load_sites};
use yard_core::node::{LtsMap, install_hint, installed, pick};
use yard_core::php::full_version;
use yard_core::project::ProjectFile;
use yard_core::runtimes::Runtimes;
use yard_core::runtimes::resolve::{PinSource, pin_file_version, resolve_pin};
use yard_core::runtimes::select::{installed as runtime_installed, php_full_or_zero};
use yard_core::runtimes::shim::{Pick, pick_composer, recipe_command, refused};
use yard_core::runtimes::version::RuntimeVersion;
use yard_core::services::sleep::lists_site;
use yard_core::shim::{
    NODE_SHIMS, ShimCommand, Source, WAKE_SHIMS, command, node_command, resolve_node, resolve_version,
    site_at,
};
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
    if name == "composer" {
        return run_composer(args);
    }
    let php = Php::resolve(name)?;
    let (program, pre, env) = php.command(name)?;
    php.wake_if(WAKE_SHIMS.contains(&name))?;
    exec(&program, pre, args, env)
}

fn exec(
    program: &Path,
    pre: Vec<OsString>,
    args: Vec<OsString>,
    env: Vec<(String, String)>,
) -> Result<Infallible, YardError> {
    let e = Command::new(program).args(pre).args(args).envs(env).exec();
    Err(YardError::new(ErrorCode::Internal, format!("cannot run {}: {e}", program.display())))
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// The PHP a directory runs, as every PHP shim and `composer` resolve it.
struct Php {
    paths: Paths,
    yard: YardConfig,
    /// `yard.toml`'s error while it does not parse and no last good defaults exist.
    broken: Option<YardError>,
    sites: SitesConfig,
    cwd: PathBuf,
    v: PhpVersion,
    source: Source,
    brew: Brew,
    opt: PathBuf,
}

/// `yard.toml`, else the last good defaults with a warning (explicit-defaults spec §2.4).
/// With neither, a TOML being edited must not stop a pinned project: the error is printed
/// and kept, the config has no defaults, and [`unless_broken`] gives it back to an unpinned
/// one in place of "no default is written".
fn shim_yard(paths: &Paths) -> Result<(YardConfig, Option<YardError>), YardError> {
    match load_yard_for_shim(paths) {
        Ok((yard, warning)) => {
            if let Some(warning) = warning {
                eprintln!("{warning}");
            }
            Ok((yard, None))
        }
        Err(e) => {
            eprintln!("yard: {}", e.message);
            Ok((parse_yard(None, &paths.yard_toml())?.0, Some(e)))
        }
    }
}

/// `result`, except that a missing default while `yard.toml` is broken is the config's error.
fn unless_broken<T>(result: Result<T, YardError>, broken: &Option<YardError>) -> Result<T, YardError> {
    match (result, broken) {
        (Err(e), Some(broken)) if matches!(e.code, ErrorCode::RuntimeMissing | ErrorCode::NodeMissing) => {
            Err(broken.clone())
        }
        (result, _) => result,
    }
}

impl Php {
    fn resolve(name: &str) -> Result<Php, YardError> {
        let paths = Paths::from_env()?;
        let (yard, broken) = shim_yard(&paths)?;
        let sites = load_sites(&paths).unwrap_or_else(|e| {
            eprintln!("yard: {}; ignoring site isolation", e.message);
            SitesConfig::default()
        });
        let cwd = std::env::current_dir().map_err(|e| {
            YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
                .with_hint("cd into an existing directory")
        })?;
        let (v, source) =
            unless_broken(resolve_version(&cwd, &yard, &sites, &paths, &read_small, &canonical), &broken)?;
        if debug() {
            eprintln!("yard-shim: {name} runs PHP {v} ({source})");
        }
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
        Ok(Php { paths, yard, broken, sites, cwd, v, source, brew, opt })
    }

    fn command(&self, name: &str) -> Result<ShimCommand, YardError> {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        command(name, self.v, &self.opt, &self.paths, &inherited, &exists)
    }

    /// The keg's full version (`7.1.33`), or `x.y.0` when its link cannot be read.
    fn full(&self) -> RuntimeVersion {
        let target = std::fs::read_link(&self.opt).ok().map(|t| t.to_string_lossy().into_owned());
        php_full_or_zero(target.as_deref().and_then(full_version).as_deref(), self.v)
    }

    /// Asks yardd to wake the site's services first when `wakes` and `run/sleeping` lists
    /// the site (M4e spec §5.2).
    fn wake_if(&self, wakes: bool) -> Result<(), YardError> {
        if wakes
            && let Some(site) = site_at(&self.cwd, &self.sites, &canonical)
            && read_small(&self.paths.sleeping_file())
                .and_then(Result::ok)
                .is_some_and(|t| lists_site(&t, &site))
        {
            wake(&self.paths, &site)?;
        }
        Ok(())
    }
}

fn debug() -> bool {
    std::env::var_os("YARD_SHIM_DEBUG").is_some_and(|d| d == "1")
}

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

/// `composer`: the Composer the project's PHP can run, from Yard's installs (runtimes spec
/// §9), run under that PHP. `self-update` is refused, since the phar is Yard's.
fn run_composer(args: Vec<OsString>) -> Result<Infallible, YardError> {
    let runtimes = Runtimes::carried();
    let runtime =
        runtimes.get("composer").ok_or_else(|| YardError::new(ErrorCode::Internal, "no Composer recipe"))?;
    let recipe = runtime.recipe();
    if let Some(refuse) = recipe.shims.get("composer").and_then(|def| refused(def, &args)) {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("composer {} is managed by Yard", refuse.args.join(" ")),
        )
        .with_hint(refuse.hint.clone()));
    }
    let php = Php::resolve("composer")?;
    let pin = resolve_pin(
        &php.cwd,
        &recipe.pin_files,
        &php.sites,
        &php.paths,
        &read_small,
        &canonical,
        &|f: &ProjectFile| f.runtimes.get("composer").cloned(),
        &|s: &SiteEntry| s.runtimes.get("composer").cloned(),
        &pin_file_version,
    )?
    .or_else(|| php.yard.runtime_defaults.get("composer").map(|s| (s.clone(), PinSource::Default)));
    let read = |p: &Path| read_small(p).and_then(Result::ok);
    let installs = runtime_installed(&php.paths, "composer", &list_dir, &read);
    let brew_composer = Some(php.brew.prefix.join("bin/composer")).filter(|p| p.exists());
    let full = php.full();
    let php_cmd = php.command("php")?;
    let picked = pick_composer(
        recipe,
        pin.clone(),
        &full,
        &php.source.to_string(),
        &installs,
        brew_composer,
        &php.paths,
    );
    let (program, pre, env) = match unless_broken(picked, &php.broken)? {
        Pick::Run(v, home) => {
            if debug() {
                // A Yard-installed version runs only for a pin or the written default.
                let (spec, from) = pin.as_ref().map_or_else(
                    || (String::new(), PinSource::Default.to_string()),
                    |(spec, source)| (spec.to_string(), source.to_string()),
                );
                eprintln!("yard-shim: composer runs Composer {v} ({spec} from {from}) on PHP {full}");
            }
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            recipe_command(recipe, "composer", &home, &v, Some(&php_cmd), &php.paths, &inherited)?
        }
        Pick::BrewComposer(phar) => {
            eprintln!("yard: using brew's composer; yard runtime install composer for per-project versions");
            let (program, _, env) = php_cmd;
            (program, vec![phar.into_os_string()], env)
        }
    };
    php.wake_if(runtime.wakes("composer"))?;
    exec(&program, pre, args, env)
}

/// How long a shim waits for yardd to wake a site's services: yardd's own cap is 30 s.
const WAKE_TIMEOUT: Duration = Duration::from_secs(31);

/// Asks yardd to wake the services `site` uses and waits until they run or yardd gives up
/// (M4e spec §5.2). PHP runs either way, a wake that fails being the service's own error,
/// except when yardd refuses (`403`): another program holds a port the site's services need,
/// and could pose as the database, so PHP must not reach it.
fn wake(paths: &Paths, site: &str) -> Result<(), YardError> {
    eprintln!("yard: waking the services {site} uses …");
    let ask = || -> std::io::Result<String> {
        let mut stream = UnixStream::connect(paths.wake_socket())?;
        stream.set_read_timeout(Some(WAKE_TIMEOUT))?;
        stream.set_write_timeout(Some(WAKE_TIMEOUT))?;
        write!(stream, "GET /wake/{site}?wait=30 HTTP/1.1\r\nHost: yard\r\nConnection: close\r\n\r\n")?;
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer)?;
        Ok(String::from_utf8_lossy(&answer).into_owned())
    };
    match ask() {
        Ok(answer) if answer.starts_with("HTTP/1.1 403 ") => {
            let held = answer.split_once("\r\n\r\n").map_or("", |(_, body)| body.trim());
            Err(YardError::new(
                ErrorCode::PortInUse,
                format!("{site}'s services cannot start, so PHP did not run: {held}"),
            )
            .with_hint("yard doctor"))
        }
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("yard: could not wake {site}'s services ({e}); running anyway");
            Ok(())
        }
    }
}

fn run_node(name: &str, args: Vec<OsString>) -> Result<Infallible, YardError> {
    let paths = Paths::from_env()?;
    let (yard, broken) = shim_yard(&paths)?;
    let sites = load_sites(&paths).unwrap_or_else(|e| {
        eprintln!("yard: {}; ignoring site settings", e.message);
        SitesConfig::default()
    });
    let cwd = std::env::current_dir().map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read the current directory: {e}"))
            .with_hint("cd into an existing directory")
    })?;
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (spec, source) =
        unless_broken(resolve_node(&cwd, &yard, &sites, &paths, &read_small, &canonical), &broken)?;
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
            .with_hint(install_hint(&spec))
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
