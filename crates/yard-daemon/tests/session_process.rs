//! Yard.app's session against a real `yardd` process, killed: the session must notice.

mod support;

use std::process::{Child, Command, Stdio};
use std::time::Duration;
use support::{Fixture, free_port, ports_toml};
use yard_client::session::{ConnectionState, Session, SessionConfig, Update};

/// The yardd this test started, killed however the test ends.
struct Yardd(Child);

impl Drop for Yardd {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_notices_a_killed_yardd() {
    // A short root: the socket path must fit sun_path. Its units are the fixture's fakes on ports
    // of their own, never brew's nginx and dnsmasq on 8080, 8443 and 53535.
    let root = tempfile::Builder::new().prefix("ys").tempdir_in("/tmp").unwrap();
    let fx = Fixture::new();
    std::fs::write(root.path().join("ports.toml"), ports_toml(free_port(), free_port(), free_port()))
        .unwrap();
    let mut yardd = Yardd(
        Command::new(env!("CARGO_BIN_EXE_yardd"))
            .args(["--root", root.path().to_str().unwrap()])
            .env("YARD_BREW_PREFIX", fx.brew.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = root.path().join("yardd.sock");
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_session, mut updates) = Session::start(SessionConfig::new(socket, "test".into()));
    let state = |u: Option<Update>| match u {
        Some(Update::State(s)) => Some(s),
        _ => None,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !matches!(
        state(tokio::time::timeout_at(deadline, updates.recv()).await.unwrap()),
        Some(ConnectionState::Connected(_))
    ) {}
    // SAFETY: signals the child this test started.
    unsafe { libc::kill(yardd.0.id() as i32, libc::SIGTERM) };
    let _ = yardd.0.wait();
    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(ConnectionState::Disconnected(e)) = state(updates.recv().await) {
                return e;
            }
        }
    })
    .await;
    assert!(gone.is_ok(), "no Disconnected within 5 s of yardd exiting");
}
