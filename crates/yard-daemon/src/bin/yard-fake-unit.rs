//! Test stand-in for nginx, dnsmasq and php-fpm, chosen by `argv[0]`. Each reads the
//! config Yard rendered and listens where it says. Never shipped.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

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

/// `-p <prefix> -c <conf> [-e <log>] [-t]`. A conf with `daemon off;`, as Yard renders,
/// runs in the foreground, listening itself. Otherwise it "daemonizes": starts a listener
/// child, writes the child's pid to the conf's `pid` path and exits.
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
    let Ok(text) = std::fs::read_to_string(conf) else { return fail(format!("cannot read {conf}")) };
    let port = text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("listen 127.0.0.1:")?;
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u16>().ok()
    });
    let pid_file = text.lines().find_map(|l| {
        let value = l.trim().strip_prefix("pid ")?.trim_end_matches(';').trim();
        Some(value.trim_matches('"').to_string())
    });
    if text.lines().any(|l| l.trim() == "daemon off;") {
        return listen_tcp(port);
    }
    let (Some(port), Some(pid_file)) = (port, pid_file) else {
        return fail(format!("{conf} has no `listen 127.0.0.1:N` or no `pid`"));
    };
    let Ok(exe) = std::env::current_exe() else { return fail("no current_exe".into()) };
    let child = Command::new(exe)
        .arg0("yard-fake-unit")
        .args(["listen-tcp", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(child) = child else { return fail("cannot spawn the listener".into()) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if Instant::now() > deadline {
            return fail(format!("listener on {port} did not come up"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    match std::fs::write(&pid_file, format!("{}\n", child.id())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("cannot write {pid_file}: {e}")),
    }
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
