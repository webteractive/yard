//! Opt-in checks against Homebrew's real nginx, which the other suites replace with
//! `yard-fake-unit`. Run with `cargo test -p yard-daemon --test real_nginx -- --ignored`.

mod support;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use support::{inheritable_listener, wait_until};

const NGINX: &str = "/opt/homebrew/opt/nginx/bin/nginx";

/// nginx, killed when the test ends, a failed assertion included: a master left running keeps
/// the test's output pipe open, so `cargo test … | grep` would never finish.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl std::ops::Deref for Running {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Running {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// The body of `GET /`, or an empty string when nothing answers.
fn get(port: u16) -> String {
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return String::new() };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.write_all(b"GET / HTTP/1.0\r\nHost: spike\r\n\r\n");
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out.split("\r\n\r\n").nth(1).unwrap_or_default().to_string()
}

fn nginx_args(dir: &Path) -> Vec<String> {
    let p = |f: &str| dir.join(f).display().to_string();
    vec![
        "-p".into(),
        format!("{}/", dir.display()),
        "-c".into(),
        p("nginx.conf"),
        "-e".into(),
        p("error.log"),
    ]
}

/// The privileged profile rests on this (spec §3.1): a non-root nginx serves on a socket
/// it inherits through `NGINX`, keeps it across a reload, and passes `nginx -t` with it.
/// The test keeps its own copy of the listener, so nginx binding the port itself would
/// fail with "Address already in use": an answer proves the socket was inherited.
#[tokio::test]
#[ignore = "needs brew nginx"]
async fn real_nginx_serves_on_an_inherited_socket() {
    assert!(Path::new(NGINX).exists(), "brew install nginx");
    let dir = tempfile::Builder::new().prefix("yard-test-").tempdir_in("/tmp").unwrap();
    let (listener, port) = inheritable_listener();
    let fd = listener.as_raw_fd();
    let conf = format!(
        "daemon off;\nmaster_process on;\npid {pid};\nerror_log {log};\nevents {{}}\nhttp {{\n    access_log off;\n    server {{\n        listen 127.0.0.1:{port};\n        location / {{ return 200 \"inherited\"; }}\n    }}\n}}\n",
        pid = dir.path().join("nginx.pid").display(),
        log = dir.path().join("error.log").display(),
    );
    std::fs::write(dir.path().join("nginx.conf"), conf).unwrap();
    let args = nginx_args(dir.path());

    let without = Command::new(NGINX).arg("-t").args(&args).output().unwrap();
    eprintln!(
        "nginx -t without NGINX: {} — {}",
        without.status,
        String::from_utf8_lossy(&without.stderr).trim()
    );

    let mut nginx = Running(
        Command::new(NGINX)
            .args(&args)
            .env("NGINX", format!("{fd};"))
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let pid = nginx.id() as i32;

    let served = wait_until(|| get(port) == "inherited", Duration::from_secs(5)).await;
    let log = std::fs::read_to_string(dir.path().join("error.log")).unwrap_or_default();
    assert!(served, "(1) nginx serves on the inherited socket; error.log:\n{log}");

    // SAFETY: kill(2) on the nginx this test spawned.
    unsafe { libc::kill(pid, libc::SIGHUP) };
    tokio::time::sleep(Duration::from_millis(500)).await;
    let log = std::fs::read_to_string(dir.path().join("error.log")).unwrap_or_default();
    assert_eq!(get(port), "inherited", "(2) it still serves after a reload; error.log:\n{log}");
    assert!(nginx.try_wait().unwrap().is_none(), "the master survived the reload");

    let with = Command::new(NGINX).arg("-t").args(&args).env("NGINX", format!("{fd};")).output().unwrap();
    assert!(with.status.success(), "(3) nginx -t passes: {}", String::from_utf8_lossy(&with.stderr));

    // SAFETY: as above.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let _ = nginx.wait();
    eprintln!("error.log:\n{}", std::fs::read_to_string(dir.path().join("error.log")).unwrap_or_default());
}

/// How the stand-in for `wake.sock` answers.
const HANG: u8 = 0;
const ERROR_500: u8 = 1;
const NO_CONTENT_AFTER_300_MS: u8 = 2;
const FORBIDDEN: u8 = 3;

/// The head of the last request the stand-in for `wake.sock` read.
static LAST_ASK: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// A `wake.sock` that answers each request as `mode` says when it arrives.
fn fake_wake(socket: &Path, mode: &'static std::sync::atomic::AtomicU8) {
    let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut head = [0u8; 4096];
                let n = stream.read(&mut head).unwrap_or(0);
                *LAST_ASK.lock().unwrap() = String::from_utf8_lossy(&head[..n]).into_owned();
                match mode.load(std::sync::atomic::Ordering::SeqCst) {
                    HANG => std::thread::sleep(Duration::from_secs(30)),
                    ERROR_500 => {
                        let _ = stream
                            .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n");
                    }
                    FORBIDDEN => {
                        let body = "redis: port 6380 is held by nc (pid 9)\n";
                        let _ = write!(
                            stream,
                            "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        );
                    }
                    _ => {
                        std::thread::sleep(Duration::from_millis(300));
                        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
                    }
                }
            });
        }
    });
}

/// The status line of `GET <path>`, and how long it took.
fn status(port: u16, path: &str) -> (String, Duration) {
    let (line, took, _) = response(port, path);
    (line, took)
}

/// The status line, how long it took and the whole answer of `GET <path>`.
fn response(port: u16, path: &str) -> (String, Duration, String) {
    let started = std::time::Instant::now();
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return (String::new(), started.elapsed(), String::new());
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = write!(s, "GET {path} HTTP/1.0\r\nHost: blog.yard\r\nCookie: session=secret\r\n\r\n");
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    (out.lines().next().unwrap_or_default().to_string(), started.elapsed(), out)
}

/// M4e spec §5.1: the wake hook Yard renders never stops a PHP request because yardd cannot
/// answer. With no socket, a socket that answers 500, or one that never answers, the request
/// is served; a socket that answers 204 holds it until then. A 403 (another program holds a
/// port the site's services need) is the one refusal: a 503 that says so.
#[tokio::test]
#[ignore = "needs brew nginx"]
async fn the_wake_hook_fails_open() {
    assert!(Path::new(NGINX).exists(), "brew install nginx");
    let dir = tempfile::Builder::new().prefix("yard-test-").tempdir_in("/tmp").unwrap();
    let www = dir.path().join("www");
    std::fs::create_dir_all(&www).unwrap();
    std::fs::write(www.join("index.php"), "static\n").unwrap();
    let socket = dir.path().join("wake.sock");
    let wake = yard_core::render::nginx::wake_location(&socket, "blog");
    let port = support::free_port();
    let conf = |wake: &str| {
        format!(
            "daemon off;\nmaster_process off;\npid {pid};\nerror_log {log};\nevents {{}}\nhttp {{\n    access_log off;\n    server {{\n        listen 127.0.0.1:{port};\n        root {www};\n\n    location ~ \\.php$ {{\n{ask}        default_type text/plain;\n    }}\n\n{wake}    }}\n}}\n",
            pid = dir.path().join("nginx.pid").display(),
            log = dir.path().join("error.log").display(),
            www = www.display(),
            ask = yard_core::render::nginx::WAKE_ASK,
        )
    };
    let args = nginx_args(dir.path());
    std::fs::write(dir.path().join("nginx.conf"), conf(&wake)).unwrap();
    let test = Command::new(NGINX).arg("-t").args(&args).output().unwrap();
    assert!(
        test.status.success(),
        "nginx -t passes on the rendered block: {}",
        String::from_utf8_lossy(&test.stderr)
    );

    // A socket that never answers is given up on after 1 s here, not 15.
    let short = wake.replace("proxy_read_timeout 15s;", "proxy_read_timeout 1s;");
    assert_ne!(short, wake, "the rendered block sets proxy_read_timeout 15s");
    std::fs::write(dir.path().join("nginx.conf"), conf(&short)).unwrap();
    let nginx = Running(Command::new(NGINX).args(&args).stdin(Stdio::null()).spawn().unwrap());
    let up = wait_until(|| std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(), Duration::from_secs(5))
        .await;
    assert!(up, "nginx listens");

    let log = || std::fs::read_to_string(dir.path().join("error.log")).unwrap_or_default();
    let (line, _) = status(port, "/index.php");
    assert!(line.contains(" 200 "), "no socket: {line}\n{}", log());

    static MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(ERROR_500);
    fake_wake(&socket, &MODE);
    let (line, _) = status(port, "/index.php");
    assert!(line.contains(" 200 "), "a 500 from the socket: {line}\n{}", log());

    MODE.store(HANG, std::sync::atomic::Ordering::SeqCst);
    let (line, took) = status(port, "/index.php");
    assert!(line.contains(" 200 "), "a socket that never answers: {line} after {took:?}\n{}", log());

    MODE.store(NO_CONTENT_AFTER_300_MS, std::sync::atomic::Ordering::SeqCst);
    let (line, took) = status(port, "/index.php");
    assert!(line.contains(" 200 "), "a 204: {line}\n{}", log());
    assert!(took >= Duration::from_millis(300), "the request waited for the wake: {took:?}");
    let ask = LAST_ASK.lock().unwrap().clone();
    assert!(ask.starts_with("GET /wake/blog?wait=10 "), "{ask}");
    assert!(!ask.contains("secret"), "the browser's cookie stays out of the ask: {ask}");

    MODE.store(FORBIDDEN, std::sync::atomic::Ordering::SeqCst);
    let (line, _, page) = response(port, "/index.php");
    assert!(line.contains(" 503 "), "a 403 from the socket: {line}\n{}", log());
    assert!(page.contains("yard doctor") && !page.contains("static"), "PHP never ran: {page}");

    drop(nginx);
}
