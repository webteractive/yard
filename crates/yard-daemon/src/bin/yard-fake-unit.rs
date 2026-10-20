//! Test stand-in for nginx, dnsmasq, php-fpm, php, pecl and brew, chosen by `argv[0]`. Each reads the
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
        _ => mode(&args),
    }
}

fn mode(args: &[String]) -> ExitCode {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("listen-tcp") => listen_tcp(arg(1).and_then(|p| p.parse().ok())),
        Some("signals") => signals(arg(1).and_then(|p| p.parse().ok())),
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
fn brew(argv0: &Path, args: &[String]) -> ExitCode {
    let Some(prefix) = argv0.parent().and_then(Path::parent) else {
        return fail(format!("brew at {} has no prefix", argv0.display()));
    };
    let line = args.join(" ");
    println!("==> fake brew {line}");
    let log = prefix.join("fake-brew.log");
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::write(&log, format!("{logged}{line}\n"));
    let failing = std::fs::read_to_string(prefix.join("fake-brew-fail")).unwrap_or_default();
    if args.iter().any(|a| !failing.trim().is_empty() && a == failing.trim()) {
        eprintln!("Error: fake failure for {}", failing.trim());
        return ExitCode::from(1);
    }
    let formulas = std::fs::read_to_string(prefix.join("fake-ext-formulas")).unwrap_or_default();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or_default();
    let (verb, formula) = match arg(0) {
        "info" => (arg(0), arg(2)),
        verb => (verb, arg(1)),
    };
    let me = std::env::current_exe().unwrap_or_default();
    let result = if let Some(v) = formula.strip_prefix("shivammathur/php/php@") {
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

/// `php -m`: lists the module each `extension=`/`zend_extension=` line under
/// `PHP_INI_SCAN_DIR` loads. A path containing `broken` fails to load, with PHP's warning.
fn php(args: &[String]) -> ExitCode {
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
                if value.contains("broken") {
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
/// in `<prefix>/fake-brew-fail`.
fn pecl(argv0: &Path, args: &[String]) -> ExitCode {
    let line = args.join(" ");
    println!("==> fake pecl {line}");
    let prefix = argv0.ancestors().nth(4).unwrap_or(Path::new("/"));
    let failing = std::fs::read_to_string(prefix.join("fake-brew-fail")).unwrap_or_default();
    if args.iter().any(|a| !failing.trim().is_empty() && a == failing.trim()) {
        eprintln!("No releases available for package \"pecl.php.net/{}\"", failing.trim());
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
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
    // nginx reloads on SIGHUP; the fake must survive one.
    // SAFETY: setting a signal disposition to SIG_IGN has no preconditions.
    unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
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
            let broken = path.extension().is_some_and(|e| e == "conf")
                && std::fs::read_to_string(&path).is_ok_and(|t| t.contains("yard-test-broken"));
            broken.then_some(path)
        }
    })
}

fn fail(message: String) -> ExitCode {
    eprintln!("yard-fake-unit: {message}");
    ExitCode::from(70)
}
