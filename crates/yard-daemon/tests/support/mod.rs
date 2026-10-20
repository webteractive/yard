//! Shared test fixtures: a temp data root, a fake brew prefix whose nginx, dnsmasq and
//! php-fpm are `yard-fake-unit`, and an in-process helper.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use yard_core::brew::Brew;
use yard_core::supervise::PolicyConfig;
use yard_core::{Paths, PhpVersion};
use yard_daemon::helper_client::HelperClient;
use yard_daemon::supervisor::SupervisorConfig;
use yard_helper::HelperConfig;

pub const FAKE: &str = env!("CARGO_BIN_EXE_yard-fake-unit");

pub struct Fixture {
    pub root: TempDir,
    pub brew: TempDir,
    pub paths: Paths,
    pub fake: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let root = TempDir::new().unwrap();
        let brew_dir = TempDir::new().unwrap();
        let paths = Paths::new(root.path());
        let brew = Brew::new(brew_dir.path());
        let fake = PathBuf::from(FAKE);
        place(&brew_dir.path().join("bin/brew"), |dst| symlink(&fake, dst));
        place(&brew.nginx_bin(), |dst| symlink(&fake, dst));
        place(&brew.dnsmasq_bin(), |dst| symlink(&fake, dst));
        let php_opt = brew_dir.path().join(format!("opt/php@{}", PhpVersion::DEFAULT));
        place(&Brew::php_fpm(&php_opt), |dst| symlink(&fake, dst));
        for file in [brew.nginx_mime_types(), brew.nginx_fastcgi_params()] {
            place(&file, |dst| std::fs::write(dst, ""));
        }
        std::fs::create_dir_all(brew.php_conf_d(PhpVersion::DEFAULT)).unwrap();
        for dir in [
            paths.nginx_sites_dir(),
            paths.nginx_dir().join("overrides"),
            paths.nginx_default_dir(),
            paths.logs_dir(),
            paths.run_dir(),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Self { root, brew: brew_dir, paths, fake }
    }
}

fn place(dst: &Path, make: impl FnOnce(&Path) -> std::io::Result<()>) {
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    make(dst).unwrap();
}

/// Millisecond backoff and health checks, so supervised tests finish quickly.
pub fn fast() -> SupervisorConfig {
    SupervisorConfig {
        policy: PolicyConfig {
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_millis(200),
            failure_window: Duration::from_secs(60),
            max_failures: 5,
        },
        health_interval: Duration::from_millis(20),
        start_grace: Duration::from_secs(2),
        stop_timeout: Duration::from_millis(500),
        log_max_bytes: 1024 * 1024,
        log_keep: 2,
    }
}

/// A `ports.toml` pinning nginx and dnsmasq to the given ports.
pub fn ports_toml(http: u16, https: u16, dns: u16) -> String {
    format!(
        "[ports]\nnginx-http = {{ port = {http}, mode = \"pinned\" }}\nnginx-https = {{ port = {https}, mode = \"pinned\" }}\ndnsmasq = {{ port = {dns}, mode = \"pinned\" }}\n"
    )
}

/// An OS-assigned port, released at once, and never handed to another test in this
/// binary. macOS allocates ephemeral ports in sequence, but every health check and probe
/// connection takes one too, so the sequence wraps within seconds and `:0` would give a
/// second test a port the first has not bound yet.
pub fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<BTreeSet<u16>> = std::sync::Mutex::new(BTreeSet::new());
    let mut given = GIVEN.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        if given.insert(port) {
            return port;
        }
    }
}

/// The helper, in-process as the current user, its socket under a short `/tmp` dir. `privileged_ports` stands in for 80 and
/// 443, which a test cannot bind.
pub async fn spawn_helper(fx: &Fixture, resolver_dir: &Path, privileged_ports: &[u16]) -> HelperClient {
    let dir = tempfile::Builder::new().prefix("yard-test-").tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("helper.sock");
    let cfg = HelperConfig {
        // SAFETY: getuid cannot fail.
        uid: unsafe { libc::getuid() },
        user: std::env::var("USER").unwrap_or_default(),
        root: fx.paths.root().to_path_buf(),
        brew_prefix: fx.brew.path().to_path_buf(),
        socket: socket.clone(),
        resolver_dir: resolver_dir.to_path_buf(),
        privileged_ports: privileged_ports.to_vec(),
    };
    tokio::spawn(async move {
        let _dir = dir;
        yard_helper::serve(cfg, std::future::pending::<()>()).await.unwrap();
    });
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)).await, "helper socket appears");
    HelperClient::new(socket)
}

pub async fn wait_until(mut done: impl FnMut() -> bool, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    done()
}

/// Opens a listener in the state macOS leaves every new socket in until FD_CLOEXEC is
/// set: inheritable by a child spawned from another thread in that window.
pub fn inheritable_listener() -> (std::net::TcpListener, u16) {
    use std::os::fd::AsRawFd;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // SAFETY: clearing FD_CLOEXEC on a descriptor this function owns.
    assert_eq!(unsafe { libc::fcntl(l.as_raw_fd(), libc::F_SETFD, 0) }, 0);
    let port = l.local_addr().unwrap().port();
    (l, port)
}

/// The parent of `pid`, from `ps`; 0 when it cannot be read.
pub fn ppid(pid: u32) -> u32 {
    let out =
        std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

/// Something accepted a connection on `127.0.0.1:port` and closed it, as the fake units
/// do. A listener nobody accepts on still takes the connect into its backlog, so a bare
/// connect cannot tell a served socket from one yardd merely holds.
pub fn served(port: u16) -> bool {
    use std::io::Read;
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return false };
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.read_to_end(&mut Vec::new()).is_ok()
}

/// How many times nginx started on sockets it inherited, from the line real nginx logs
/// (and the fake copies): `using inherited sockets from "…"`.
pub fn inherited_starts(paths: &Paths) -> usize {
    let log = std::fs::read_to_string(paths.logs_dir().join("nginx.log")).unwrap_or_default();
    log.lines().filter(|l| l.contains("using inherited sockets from")).count()
}
