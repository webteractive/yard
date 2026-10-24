//! Test stand-in for nginx, dnsmasq, php-fpm, php, pecl, brew, security and the service
//! engines (mysqld, postgres, initdb, redis-server, minio), chosen by `argv[0]`. Each reads the
//! config Yard rendered and listens where it says. Never shipped.

use std::io::Write;
use std::net::TcpListener;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let mut argv = std::env::args();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<String> = argv.collect();
    match Path::new(&argv0).file_name().and_then(|n| n.to_str()) {
        Some("nginx") => nginx(&args),
        Some("dnsmasq") => dnsmasq(&args),
        Some("php-fpm") => php_fpm(&args),
        Some("brew") => brew(Path::new(&argv0), &args),
        Some("php") => php(&args),
        Some("pecl") => pecl(Path::new(&argv0), &args),
        Some("security") => security(Path::new(&argv0), &args),
        Some("mysqld") => mysqld(Path::new(&argv0), &args),
        Some("initdb") => initdb(Path::new(&argv0), &args),
        Some("postgres") => postgres(&args),
        Some("redis-server") => redis_server(&args),
        Some("minio") => minio(&args),
        _ => mode(&args),
    }
}

fn mode(args: &[String]) -> ExitCode {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("listen-tcp") => listen_tcp(arg(1).and_then(|p| p.parse().ok())),
        Some("signals") => signals(arg(1).and_then(|p| p.parse().ok())),
        Some("stubborn") => stubborn(arg(1).and_then(|p| p.parse().ok())),
        Some("listen-unix") => listen_unix(arg(1).map(PathBuf::from)),
        Some("exit") => {
            let code = arg(1).and_then(|c| c.parse::<u8>().ok()).unwrap_or(1);
            let after = arg(2).and_then(|ms| ms.parse().ok()).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(after));
            ExitCode::from(code)
        }
        Some("hang") => hang(),
        Some("print-path") => {
            println!("PATH={}", std::env::var("PATH").unwrap_or_default());
            ExitCode::SUCCESS
        }
        Some("lines") => {
            let n: u32 = arg(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            let mut out = std::io::stdout().lock();
            for i in 1..=n {
                let _ = writeln!(out, "line {i}");
            }
            let _ = out.flush();
            drop(out);
            hang()
        }
        _ => fail(format!("unknown mode {args:?}")),
    }
}

/// `-p <prefix> -c <conf> [-e <log>] [-t]`. Runs in the foreground, as Yard's
/// `daemon off;` asks. With `NGINX="fd;…"` it serves on every inherited socket, as nginx
/// does, and logs the line real nginx logs; otherwise it listens on the conf's first
/// `listen 127.0.0.1:N`.
fn nginx(args: &[String]) -> ExitCode {
    let (Some(prefix), Some(conf)) = (flag(args, "-p"), flag(args, "-c")) else {
        return fail("nginx needs -p and -c".into());
    };
    if args.iter().any(|a| a == "-t") {
        if let Some(bad) = find_broken(Path::new(prefix)) {
            eprintln!("nginx: [emerg] yard-test-broken in {}", bad.display());
            eprintln!("nginx: configuration file {conf} test failed");
            return ExitCode::from(1);
        }
        if let Some(missing) = missing_certificate(Path::new(prefix)) {
            eprintln!(
                "nginx: [emerg] cannot load certificate \"{}\": BIO_new_file() failed",
                missing.display()
            );
            eprintln!("nginx: configuration file {conf} test failed");
            return ExitCode::from(1);
        }
        eprintln!("nginx: configuration file {conf} test is successful");
        return ExitCode::SUCCESS;
    }
    // nginx shuts down gracefully on SIGQUIT. Dying of it instead would leave a crash
    // report and hold up every stop while ReportCrash writes it.
    extern "C" fn quit(_: libc::c_int) {
        // SAFETY: _exit(2) is async-signal-safe.
        unsafe { libc::_exit(0) };
    }
    // SAFETY: installing a handler that makes only an async-signal-safe call.
    unsafe { libc::signal(libc::SIGQUIT, quit as libc::sighandler_t) };
    if let Ok(fds) = std::env::var("NGINX") {
        eprintln!("nginx: [notice] using inherited sockets from \"{fds}\"");
        let fds: Vec<RawFd> = fds.split(';').filter_map(|fd| fd.parse().ok()).collect();
        let Some((&first, rest)) = fds.split_first() else {
            return fail(format!("NGINX={fds:?} names no descriptor"));
        };
        for &fd in rest {
            // SAFETY: yardd handed these listening sockets to nginx, which owns them from here.
            let listener = unsafe { TcpListener::from_raw_fd(fd) };
            std::thread::spawn(move || serve(listener, true));
        }
        // SAFETY: as above.
        return serve(unsafe { TcpListener::from_raw_fd(first) }, true);
    }
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    let port = text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("listen 127.0.0.1:")?;
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u16>().ok()
    });
    let Some(port) = port else { return fail(format!("{conf} has no `listen 127.0.0.1:N`")) };
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => serve(listener, true),
        Err(e) => fail(format!("cannot listen on {port}: {e}")),
    }
}

/// `--conf-file=<path>` (or `--conf-file <path>`); listens on its `port=` over TCP.
fn dnsmasq(args: &[String]) -> ExitCode {
    let Some(conf) = flag(args, "--conf-file") else { return fail("dnsmasq needs --conf-file".into()) };
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    listen_tcp(text.lines().find_map(|l| l.trim().strip_prefix("port=")?.parse().ok()))
}

/// `-y <fpm.conf>`; listens on its `listen = "<path>"` unix socket.
/// Like php-fpm's master, it survives SIGUSR2 (a graceful reload) and logs
/// `fake php-fpm reloaded` to stderr for each one.
fn php_fpm(args: &[String]) -> ExitCode {
    extern "C" fn reloaded(_: libc::c_int) {
        let line = b"fake php-fpm reloaded\n";
        // SAFETY: write(2) is async-signal-safe; the buffer is static.
        unsafe { libc::write(2, line.as_ptr().cast(), line.len()) };
    }
    // SAFETY: installing a handler that only calls write(2).
    unsafe { libc::signal(libc::SIGUSR2, reloaded as libc::sighandler_t) };
    let Some(conf) = flag(args, "-y") else { return fail("php-fpm needs -y".into()) };
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    let socket = text.lines().find_map(|l| {
        let value = l.trim().strip_prefix("listen")?.trim_start().strip_prefix('=')?;
        Some(PathBuf::from(value.trim().trim_matches('"')))
    });
    listen_unix(socket)
}

/// `brew` at `<prefix>/bin/brew`. It logs each call to `<prefix>/fake-brew.log` and fails
/// when an argument equals `<prefix>/fake-brew-fail`. `install`/`uninstall` of
/// `shivammathur/php/php@<v>` create or remove a keg whose binaries are this fake;
/// extension formulas exist when listed in `<prefix>/fake-ext-formulas`; installing one
/// puts its `.so` in its keg and writes the ini brew's formula would, and uninstalling it
/// removes the keg and leaves the ini, as brew does.
///
/// Service formulae (M4b) come from `<prefix>/fake-formulae.json`, an array of the objects
/// `brew info --json=v2` has under `formulae`: `search --formula "/^<base>(@|$)/"` prints
/// the matching names, `info --json=v2 <name>...` prints `{"formulae":[<entry>...],"casks":[]}`
/// (or fails as brew does, when any name is unknown or from a tap `fake-trust.json` does not
/// trust; `fake-brew-info-delay` holds each `info` for that many milliseconds), and `install <name>` makes `opt/<name>/bin/<binaries>` for the
/// engine (`mysqld`; `postgres` and `initdb`; `redis-server`; `minio`), each this fake.
/// `--version` prints `Homebrew ` and the contents of `fake-brew-version` (default `7.0.10`); `trust --json=v1`
/// prints `fake-trust.json` (default: nothing trusted), and `trust`/`untrust --formula|--tap
/// <x>` edit it. Each call appends the `HOMEBREW_*` keys of its environment to
/// `fake-brew-env.log`, so a test can see what Yard set.
fn brew(argv0: &Path, args: &[String]) -> ExitCode {
    let Some(prefix) = argv0.parent().and_then(Path::parent) else {
        return fail(format!("brew at {} has no prefix", argv0.display()));
    };
    let line = args.join(" ");
    let log = prefix.join("fake-brew.log");
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::write(&log, format!("{logged}{line}\n"));
    let mut keys: Vec<String> =
        std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("HOMEBREW_")).collect();
    keys.sort();
    let env_log = prefix.join("fake-brew-env.log");
    let logged = std::fs::read_to_string(&env_log).unwrap_or_default();
    let _ = std::fs::write(&env_log, format!("{logged}{}\n", keys.join(" ")));
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or_default();
    // Questions and trust answer with no banner: their stdout is parsed.
    if let Some(code) = brew_query(prefix, args) {
        return code;
    }
    println!("==> fake brew {line}");
    if std::env::var_os("HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK").is_some_and(|v| v == "1") {
        println!("==> fake brew: dependents check off");
    }
    // `<prefix>/fake-brew-delay` holds milliseconds every install waits, so a job stays busy.
    if arg(0) == "install"
        && let Some(ms) =
            std::fs::read_to_string(prefix.join("fake-brew-delay")).ok().and_then(|t| t.trim().parse().ok())
    {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let failing = std::fs::read_to_string(prefix.join("fake-brew-fail")).unwrap_or_default();
    if args.iter().any(|a| !failing.trim().is_empty() && a == failing.trim()) {
        eprintln!("Error: fake failure for {}", failing.trim());
        return ExitCode::from(1);
    }
    let formulas = std::fs::read_to_string(prefix.join("fake-ext-formulas")).unwrap_or_default();
    let (verb, formula) = match arg(0) {
        "info" => (arg(0), arg(2)),
        verb => (verb, arg(1)),
    };
    let me = std::env::current_exe().unwrap_or_default();
    let service = (verb == "install").then(|| fake_formula(prefix, formula)).flatten();
    let result = if let Some(entry) = service {
        let name = entry["name"].as_str().unwrap_or_default();
        (|| {
            for bin in engine_bins(name) {
                let dst = prefix.join("opt").join(name).join("bin").join(bin);
                std::fs::create_dir_all(dst.parent().unwrap_or(prefix))?;
                let _ = std::fs::remove_file(&dst);
                std::os::unix::fs::symlink(&me, &dst)?;
            }
            Ok(())
        })()
    } else if let Some(v) = formula.strip_prefix("shivammathur/php/php@") {
        let opt = prefix.join(format!("opt/php@{v}"));
        match verb {
            "install" => (|| {
                for bin in ["sbin/php-fpm", "bin/php", "bin/pecl", "bin/phpize", "bin/php-config"] {
                    let dst = opt.join(bin);
                    std::fs::create_dir_all(dst.parent().unwrap_or(&opt))?;
                    let _ = std::fs::remove_file(&dst);
                    std::os::unix::fs::symlink(&me, &dst)?;
                }
                std::fs::create_dir_all(prefix.join(format!("etc/php/{v}/conf.d")))
            })(),
            "uninstall" => std::fs::remove_dir_all(&opt),
            _ => Ok(()),
        }
    } else if let Some((name, v)) =
        formula.strip_prefix("shivammathur/extensions/").and_then(|f| f.split_once('@'))
    {
        let ini = prefix.join(format!("etc/php/{v}/conf.d/20-{name}.ini"));
        match verb {
            "info" if !formulas.lines().any(|l| l.trim() == name) => {
                eprintln!("Error: No available formula with the name \"{formula}\".");
                return ExitCode::from(1);
            }
            "install" => std::fs::create_dir_all(prefix.join(format!("etc/php/{v}/conf.d"))).and_then(|()| {
                // As shivammathur's formulas do: the .so stays in the formula's keg.
                let so = prefix.join(format!("opt/{name}@{v}/{name}.so"));
                std::fs::create_dir_all(so.parent().unwrap_or(prefix))?;
                std::fs::write(&so, "")?;
                std::fs::write(&ini, format!("extension=\"{}\"\n", so.display()))
            }),
            // Like brew: the keg goes, the ini in etc/ stays.
            "uninstall" => std::fs::remove_dir_all(prefix.join(format!("opt/{name}@{v}"))),
            _ => Ok(()),
        }
    } else {
        Ok(())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("fake brew {line}: {e}")),
    }
}

/// The entry of `<prefix>/fake-formulae.json` that `name` (a name, full name or alias) means.
fn fake_formula(prefix: &Path, name: &str) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(prefix.join("fake-formulae.json")).ok()?;
    let entries: Vec<serde_json::Value> = serde_json::from_str(&text).ok()?;
    entries.into_iter().find(|e| {
        e["name"] == name
            || e["full_name"] == name
            || e["aliases"].as_array().is_some_and(|a| a.iter().any(|alias| alias == name))
    })
}

/// The binaries the engine's keg holds, by formula name.
fn engine_bins(name: &str) -> &'static [&'static str] {
    if name.starts_with("mysql") {
        &["mysqld"]
    } else if name.starts_with("postgresql@") {
        &["postgres", "initdb"]
    } else if name == "redis" {
        &["redis-server"]
    } else if name == "minio" {
        &["minio"]
    } else {
        &[]
    }
}

/// The brew commands that print something Yard parses, or change what brew trusts. `None`
/// for any other, which `brew` handles with its banner.
fn brew_query(prefix: &Path, args: &[String]) -> Option<ExitCode> {
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or_default();
    let trust_file = prefix.join("fake-trust.json");
    let trust = || -> serde_json::Value {
        std::fs::read_to_string(&trust_file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({"taps": [], "formulae": [], "casks": [], "commands": []}))
    };
    let failing = std::fs::read_to_string(prefix.join("fake-brew-fail")).unwrap_or_default();
    if args.iter().any(|a| !failing.trim().is_empty() && a == failing.trim()) {
        eprintln!("Error: fake failure for {}", failing.trim());
        return Some(ExitCode::from(1));
    }
    match (arg(0), arg(1)) {
        ("--version", _) => {
            let version = std::fs::read_to_string(prefix.join("fake-brew-version")).unwrap_or_default();
            let version = version.trim();
            println!("Homebrew {}", if version.is_empty() { "7.0.10" } else { version });
            Some(ExitCode::SUCCESS)
        }
        ("trust", "--json=v1") => {
            println!("{}", serde_json::to_string_pretty(&trust()).unwrap_or_default());
            Some(ExitCode::SUCCESS)
        }
        (verb @ ("trust" | "untrust"), kind @ ("--formula" | "--tap")) => {
            let key = if kind == "--tap" { "taps" } else { "formulae" };
            let target = arg(2);
            let mut doc = trust();
            let mut list: Vec<serde_json::Value> = doc[key].as_array().cloned().unwrap_or_default();
            list.retain(|v| v != target);
            if verb == "trust" {
                list.push(target.into());
            }
            doc[key] = list.into();
            if let Err(e) = std::fs::write(&trust_file, doc.to_string()) {
                return Some(fail(format!("fake brew {verb}: {e}")));
            }
            let what = if kind == "--tap" { "tap" } else { "formula" };
            println!("{} {what}: {target}", if verb == "trust" { "Trusted" } else { "Untrusted" });
            Some(ExitCode::SUCCESS)
        }
        ("search", _) => {
            // `--formula "/^<base>(@|$)/"`
            let base = args.iter().find_map(|a| a.strip_prefix("/^")?.strip_suffix("(@|$)/"))?;
            let text = std::fs::read_to_string(prefix.join("fake-formulae.json")).unwrap_or_default();
            let entries: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
            for name in entries.iter().filter_map(|e| e["name"].as_str()) {
                if name == base || name.strip_prefix(base).is_some_and(|rest| rest.starts_with('@')) {
                    println!("{name}");
                }
            }
            Some(ExitCode::SUCCESS)
        }
        ("info", "--json=v2") => {
            // Several names answer together, and one brew does not know fails the whole call.
            let mut entries = vec![];
            // `<prefix>/fake-brew-info-delay` holds milliseconds `info` waits, for a hung brew.
            if let Some(ms) = std::fs::read_to_string(prefix.join("fake-brew-info-delay"))
                .ok()
                .and_then(|t| t.trim().parse().ok())
            {
                std::thread::sleep(Duration::from_millis(ms));
            }
            for name in args.iter().skip(2) {
                let Some(entry) = fake_formula(prefix, name) else {
                    eprintln!("Error: No available formula or cask with the name \"{name}\".");
                    return Some(ExitCode::from(1));
                };
                // As brew 6 does: a formula of a tap that is not trusted is not loaded.
                let (tap, full) = (entry["tap"].as_str().unwrap_or_default(), entry["full_name"].as_str());
                let official = tap.is_empty() || tap.starts_with("homebrew/");
                let listed = |key: &str, want: &str| {
                    trust()[key].as_array().is_some_and(|l| l.iter().any(|v| v == want))
                };
                if !official && !listed("taps", tap) && !full.is_some_and(|f| listed("formulae", f)) {
                    eprintln!("Error: {} is from the untrusted tap {tap}.", full.unwrap_or(name));
                    return Some(ExitCode::from(1));
                }
                entries.push(entry);
            }
            println!("{}", serde_json::json!({"formulae": entries, "casks": []}));
            Some(ExitCode::SUCCESS)
        }
        _ => None,
    }
}

/// `php -m`: lists the module each `extension=`/`zend_extension=` line under
/// `PHP_INI_SCAN_DIR` loads. A path containing `broken` fails to load, with PHP's warning.
fn php(args: &[String]) -> ExitCode {
    // `php -n -m`: no ini, so only what is compiled in.
    if args == ["-n", "-m"] {
        println!("[PHP Modules]\nCore\ndate\njson\nmbstring\npcre\n\n[Zend Modules]\n");
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) != Some("-m") {
        return fail(format!("fake php supports only -m, not {args:?}"));
    }
    let mut modules = vec!["Core".to_string()];
    let scan = std::env::var("PHP_INI_SCAN_DIR").unwrap_or_default();
    for dir in scan.split(':').filter(|d| !d.is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut inis: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        inis.sort();
        for ini in inis.iter().filter(|p| p.extension().is_some_and(|e| e == "ini")) {
            let text = std::fs::read_to_string(ini).unwrap_or_default();
            for line in text.lines() {
                let Some((key, value)) = line.trim().split_once('=') else { continue };
                if !matches!(key.trim(), "extension" | "zend_extension") {
                    continue;
                }
                let value = value.trim().trim_matches('"');
                if value.contains("broken") || (value.starts_with('/') && !Path::new(value).exists()) {
                    eprintln!("PHP Warning:  PHP Startup: Unable to load dynamic library '{value}'");
                    continue;
                }
                let base = value.rsplit('/').next().unwrap_or(value);
                modules.push(base.strip_suffix(".so").unwrap_or(base).to_string());
            }
        }
    }
    println!("[PHP Modules]\n{}\n\n[Zend Modules]\n", modules.join("\n"));
    ExitCode::SUCCESS
}

/// `pecl install <name>` at `<prefix>/opt/php@<v>/bin/pecl`: succeeds unless `<name>` is
/// in `<prefix>/fake-brew-fail`, and puts `<name>.so` (a `-<version>` dropped) in its
/// `ext_dir`, `<prefix>/lib/php/pecl/20240924`; `uninstall` deletes it, and
/// `config-get ext_dir` prints that directory.
fn pecl(argv0: &Path, args: &[String]) -> ExitCode {
    let line = args.join(" ");
    println!("==> fake pecl {line}");
    println!("PATH={}", std::env::var("PATH").unwrap_or_default());
    for flags in ["CPPFLAGS", "LDFLAGS"] {
        println!("{flags}={}", std::env::var(flags).unwrap_or_default());
    }
    let prefix = argv0.ancestors().nth(4).unwrap_or(Path::new("/"));
    let failing = std::fs::read_to_string(prefix.join("fake-brew-fail")).unwrap_or_default();
    if args.iter().any(|a| !failing.trim().is_empty() && a == failing.trim()) {
        eprintln!("No releases available for package \"pecl.php.net/{}\"", failing.trim());
        return ExitCode::from(1);
    }
    let ext_dir = prefix.join("lib/php/pecl/20240924");
    let verb = args.iter().position(|a| matches!(a.as_str(), "install" | "uninstall" | "config-get"));
    let (verb, operand) = match verb {
        Some(i) => (args[i].as_str(), args.get(i + 1).map(String::as_str).unwrap_or_default()),
        None => return ExitCode::SUCCESS,
    };
    let so = ext_dir.join(format!("{}.so", operand.split('-').next().unwrap_or(operand)));
    let done = match verb {
        "config-get" => {
            println!("{}", ext_dir.display());
            Ok(())
        }
        "install" => std::fs::create_dir_all(&ext_dir).and_then(|()| std::fs::write(&so, "")),
        _ => std::fs::remove_file(&so),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("fake pecl {line}: {e}")),
    }
}

/// `mysqld --initialize-insecure --datadir=D` creates `D/ibdata1`. `mysqld --defaults-file=F`
/// listens on the `port` in `F`, and exits 1 with `unknown variable` when the file `F`
/// includes (`!include <path>`, Yard's `overrides.conf`) contains `bad`, as mysqld refuses
/// a line it does not know.
fn mysqld(argv0: &Path, args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--initialize-insecure") {
        let Some(data) = flag(args, "--datadir") else { return fail("mysqld needs --datadir".into()) };
        return init_data(argv0, Path::new(data), "ibdata1");
    }
    let Some(conf) = flag(args, "--defaults-file") else {
        return fail("mysqld needs --defaults-file".into());
    };
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    let included = text.lines().find_map(|l| l.trim().strip_prefix("!include ")).map(str::trim);
    if let Some(overrides) = included
        && std::fs::read_to_string(overrides).is_ok_and(|t| t.contains("bad"))
    {
        eprintln!("mysqld: [ERROR] unknown variable 'bad' in {overrides}");
        return ExitCode::from(1);
    }
    let port = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("port")?.trim_start().strip_prefix('=')?.trim().parse().ok());
    // Real mysqld opens its port only after crash recovery or a data upgrade.
    delay_from(argv0, "fake-listen-delay");
    listen_tcp(port)
}

/// Sleeps the milliseconds in `<prefix>/<file>`, if it exists. The prefix is three directories
/// above the binary: `<prefix>/opt/<formula>/bin/<name>`.
fn delay_from(argv0: &Path, file: &str) {
    let prefix = argv0.ancestors().nth(4).unwrap_or(Path::new("/"));
    if let Some(ms) = std::fs::read_to_string(prefix.join(file)).ok().and_then(|t| t.trim().parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// `initdb -D D` creates `D/PG_VERSION`.
fn initdb(argv0: &Path, args: &[String]) -> ExitCode {
    let Some(data) = flag(args, "-D") else { return fail("initdb needs -D".into()) };
    init_data(argv0, Path::new(data), "PG_VERSION")
}

/// What `mysqld --initialize-insecure` and `initdb` share: wait `<prefix>/fake-init-delay`
/// milliseconds, fail when `<prefix>/fake-init-fail` exists, else create `data/marker`.
/// The prefix is three directories above the binary: `<prefix>/opt/<formula>/bin/<name>`.
fn init_data(argv0: &Path, data: &Path, marker: &str) -> ExitCode {
    let prefix = argv0.ancestors().nth(4).unwrap_or(Path::new("/"));
    delay_from(argv0, "fake-init-delay");
    if prefix.join("fake-init-fail").exists() {
        eprintln!("{}: initialization failed", argv0.display());
        return ExitCode::from(1);
    }
    match std::fs::create_dir_all(data).and_then(|()| std::fs::write(data.join(marker), "")) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("cannot initialize {}: {e}", data.display())),
    }
}

/// `postgres -D D -c listen_addresses=… -c port=N …`; listens on `N`.
fn postgres(args: &[String]) -> ExitCode {
    let port = args.iter().find_map(|a| a.strip_prefix("port=")?.parse().ok());
    listen_tcp(port)
}

/// `redis-server <conf>`; listens on the `port` in the conf. Like real redis it exits 1 at
/// config load when the conf's `dir` is not a directory.
fn redis_server(args: &[String]) -> ExitCode {
    let Some(conf) = args.first() else { return fail("redis-server needs a config file".into()) };
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    let dir = text.lines().find_map(|l| Some(l.trim().strip_prefix("dir ")?.trim().trim_matches('"')));
    if let Some(dir) = dir
        && !Path::new(dir).is_dir()
    {
        eprintln!("Can't chdir to '{dir}': No such file or directory");
        return ExitCode::from(1);
    }
    listen_tcp(text.lines().find_map(|l| l.trim().strip_prefix("port ")?.trim().parse().ok()))
}

/// `minio server <data> --address A --console-address C`; listens on both.
fn minio(args: &[String]) -> ExitCode {
    let port = |name: &str| flag(args, name).and_then(|a| a.rsplit(':').next()?.parse::<u16>().ok());
    let (Some(api), Some(console)) = (port("--address"), port("--console-address")) else {
        return fail(format!("minio needs --address and --console-address: {args:?}"));
    };
    match TcpListener::bind(("127.0.0.1", console)) {
        Ok(listener) => {
            std::thread::spawn(move || serve(listener, false));
        }
        Err(e) => return fail(format!("cannot listen on {console}: {e}")),
    }
    listen_tcp(Some(api))
}

fn listen_tcp(port: Option<u16>) -> ExitCode {
    let Some(port) = port else { return fail("listen-tcp needs a port".into()) };
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => serve(listener, false),
        Err(e) => fail(format!("cannot listen on {port}: {e}")),
    }
}

/// Accepts and closes connections until killed. As nginx (`http`), answers each with a
/// status line first, which is what the supervisor's HTTP health check looks for.
fn serve(listener: TcpListener, http: bool) -> ExitCode {
    // nginx reloads on SIGHUP; the fake survives one and, as nginx, logs
    // `fake nginx reloaded` to stderr.
    extern "C" fn reloaded(_: libc::c_int) {
        let line = b"fake nginx reloaded\n";
        // SAFETY: write(2) is async-signal-safe; the buffer is static.
        unsafe { libc::write(2, line.as_ptr().cast(), line.len()) };
    }
    let on_hup = if http { reloaded as libc::sighandler_t } else { libc::SIG_IGN };
    // SAFETY: installing SIG_IGN, or a handler that only calls write(2).
    unsafe { libc::signal(libc::SIGHUP, on_hup) };
    for stream in listener.incoming().flatten() {
        if http {
            std::thread::spawn(move || answer(stream));
        }
    }
    ExitCode::SUCCESS
}

/// Reads the request head before answering, as nginx does: closing with the request
/// still unread makes macOS reset the connection, and the client loses the answer. A
/// client that sends nothing gets its answer after a second.
fn answer(mut stream: std::net::TcpStream) {
    use std::io::Read;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let _ = stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n");
}

/// Like `listen-tcp`, and says which of SIGQUIT and SIGTERM stopped it.
fn signals(port: Option<u16>) -> ExitCode {
    extern "C" fn quit(_: libc::c_int) {
        const MSG: &[u8] = b"got SIGQUIT\n";
        // SAFETY: write(2) and _exit(2) are async-signal-safe.
        unsafe {
            libc::write(1, MSG.as_ptr().cast(), MSG.len());
            libc::_exit(0);
        }
    }
    extern "C" fn term(_: libc::c_int) {
        const MSG: &[u8] = b"got SIGTERM\n";
        // SAFETY: as above.
        unsafe {
            libc::write(1, MSG.as_ptr().cast(), MSG.len());
            libc::_exit(0);
        }
    }
    // SAFETY: installing handlers that make only async-signal-safe calls.
    unsafe {
        libc::signal(libc::SIGQUIT, quit as libc::sighandler_t);
        libc::signal(libc::SIGTERM, term as libc::sighandler_t);
    }
    listen_tcp(port)
}

/// Like `listen-tcp`, but ignores SIGTERM and SIGQUIT, so only SIGKILL (or SIGINT) stops it: a
/// service that outlasts a stop timeout. SIGINT keeps its default action, which a shell's
/// background job would otherwise have ignored.
fn stubborn(port: Option<u16>) -> ExitCode {
    // SAFETY: installing SIG_IGN and SIG_DFL.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
    listen_tcp(port)
}

fn listen_unix(path: Option<PathBuf>) -> ExitCode {
    let Some(path) = path else { return fail("listen-unix needs a path".into()) };
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return fail(format!("cannot listen on {}: {e}", path.display())),
    };
    for stream in listener.incoming() {
        drop(stream);
    }
    ExitCode::SUCCESS
}

fn hang() -> ExitCode {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// macOS's `security`, which logs each call to `security.log` beside it. `verify-cert`
/// passes while a `fake-trusted` file sits there too, and never returns beside `fake-hang`.
fn security(argv0: &Path, args: &[String]) -> ExitCode {
    let dir = argv0.parent().unwrap_or(Path::new("."));
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("security.log"));
    if let Ok(log) = &mut log {
        let _ = writeln!(log, "{}", args.join(" "));
    }
    match args.first().map(String::as_str) {
        // A stalled trustd, which leaves security waiting.
        Some("verify-cert") if dir.join("fake-hang").exists() => hang(),
        Some("verify-cert") if dir.join("fake-trusted").exists() => ExitCode::SUCCESS,
        Some("verify-cert") => ExitCode::from(1),
        _ => fail(format!("security {args:?} is not faked")),
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, a)| {
        if a == name { args.get(i + 1).map(String::as_str) } else { a.strip_prefix(name)?.strip_prefix('=') }
    })
}

fn find_broken(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    entries.into_iter().find_map(|path| {
        if path.is_dir() {
            find_broken(&path)
        } else {
            if path.extension().is_none_or(|e| e != "conf") {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            if text.contains("yard-test-broken") {
                return Some(path);
            }
            // Like real nginx, a file a conf includes from outside the prefix is tested too
            // (MinIO's `services/minio/overrides.conf`).
            included(&text)
                .into_iter()
                .find(|file| std::fs::read_to_string(file).is_ok_and(|t| t.contains("yard-test-broken")))
        }
    })
}

/// The single files `include "<path>";` lines name; globs (`sites/*.conf`) are left to the walk.
fn included(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("include "))
        .map(|rest| rest.trim().trim_end_matches(';').trim().trim_matches('"'))
        .filter(|file| !file.contains('*'))
        .map(PathBuf::from)
        .collect()
}

/// The first `ssl_certificate` or `ssl_certificate_key` file a conf under `dir` names
/// that does not exist, which real nginx refuses at `-t`.
fn missing_certificate(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    entries.into_iter().find_map(|path| {
        if path.is_dir() {
            return missing_certificate(&path);
        }
        if path.extension().is_none_or(|e| e != "conf") {
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        text.lines().find_map(|line| {
            let line = line.trim();
            let rest = line
                .strip_prefix("ssl_certificate_key ")
                .or_else(|| line.strip_prefix("ssl_certificate "))?;
            let file = PathBuf::from(rest.trim_end_matches(';').trim_matches('"'));
            (!file.exists()).then_some(file)
        })
    })
}

fn fail(message: String) -> ExitCode {
    eprintln!("yard-fake-unit: {message}");
    ExitCode::from(70)
}
