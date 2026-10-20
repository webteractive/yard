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

    let mut nginx = Command::new(NGINX)
        .args(&args)
        .env("NGINX", format!("{fd};"))
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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
