//! Test stand-in for nginx, dnsmasq and php-fpm, chosen by `argv[0]`. Each reads the
//! config Yard rendered and listens where it says. Never shipped.

use std::io::Write;
use std::net::TcpListener;
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
        _ => mode(&args),
    }
}

fn mode(args: &[String]) -> ExitCode {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("listen-tcp") => listen_tcp(arg(1).and_then(|p| p.parse().ok())),
        Some("listen-unix") => listen_unix(arg(1).map(PathBuf::from)),
        Some("exit") => {
            let code = arg(1).and_then(|c| c.parse::<u8>().ok()).unwrap_or(1);
            let after = arg(2).and_then(|ms| ms.parse().ok()).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(after));
            ExitCode::from(code)
        }
        Some("hang") => hang(),
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
/// `daemon off;` asks, listening on the conf's first `listen 127.0.0.1:N`.
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
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    listen_tcp(text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("listen 127.0.0.1:")?;
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u16>().ok()
    }))
}

/// `--conf-file=<path>` (or `--conf-file <path>`); listens on its `port=` over TCP.
fn dnsmasq(args: &[String]) -> ExitCode {
    let Some(conf) = flag(args, "--conf-file") else { return fail("dnsmasq needs --conf-file".into()) };
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    listen_tcp(text.lines().find_map(|l| l.trim().strip_prefix("port=")?.parse().ok()))
}

/// `-y <fpm.conf>`; listens on its `listen = "<path>"` unix socket.
fn php_fpm(args: &[String]) -> ExitCode {
    let Some(conf) = flag(args, "-y") else { return fail("php-fpm needs -y".into()) };
    let text = std::fs::read_to_string(conf).unwrap_or_default();
    let socket = text.lines().find_map(|l| {
        let value = l.trim().strip_prefix("listen")?.trim_start().strip_prefix('=')?;
        Some(PathBuf::from(value.trim().trim_matches('"')))
    });
    listen_unix(socket)
}

fn listen_tcp(port: Option<u16>) -> ExitCode {
    let Some(port) = port else { return fail("listen-tcp needs a port".into()) };
    // nginx reloads on SIGHUP; the fake must survive one.
    // SAFETY: setting a signal disposition to SIG_IGN has no preconditions.
    unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => return fail(format!("cannot listen on {port}: {e}")),
    };
    for stream in listener.incoming() {
        drop(stream);
    }
    ExitCode::SUCCESS
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
