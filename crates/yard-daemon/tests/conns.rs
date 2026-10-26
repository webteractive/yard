//! Counting a service instance's client connections (M4e spec §7), against `yard-fake-unit`.

mod support;

use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use support::{FAKE, free_port};
use yard_daemon::conns::client_connections;

/// Kills the fake when the test ends, however it ends.
struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(arg0: &str, args: &[&str], port: u16) -> Fake {
    let child = Command::new(FAKE).arg0(arg0).args(args).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "{arg0} never listened on {port}");
        std::thread::sleep(Duration::from_millis(10));
    }
    Fake(child)
}

/// Waits up to 2 s for the count to be `want`.
fn settles(pid: u32, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let got = client_connections(pid);
        if got == Ok(want) {
            return;
        }
        assert!(Instant::now() < deadline, "wanted {want} connections, last counted {got:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn connections_are_counted_across_the_process_tree() {
    let port = free_port();
    let fake = spawn("postgres", &["-c", &format!("port={port}")], port);
    let pid = fake.0.id();
    // The probe in `spawn` connected once; its backend goes when it closes.
    settles(pid, 0);
    let a = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let b = TcpStream::connect(("127.0.0.1", port)).unwrap();
    settles(pid, 2);
    drop((a, b));
    settles(pid, 0);
}

#[test]
fn a_redis_connection_is_counted() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("redis.conf");
    std::fs::write(&conf, format!("port {port}\n")).unwrap();
    let fake = spawn("redis-server", &[conf.to_str().unwrap()], port);
    let pid = fake.0.id();
    settles(pid, 0);
    let _held = TcpStream::connect(("127.0.0.1", port)).unwrap();
    settles(pid, 1);
}
