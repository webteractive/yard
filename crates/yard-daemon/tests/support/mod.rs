//! Shared test fixtures: a temp data root, a fake brew prefix whose nginx, dnsmasq and
//! php-fpm are `yard-fake-unit`, and an in-process helper.
#![allow(dead_code)]

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
        // nginx is a copy, not a symlink: the helper only signals a pid whose executable is
        // the canonical nginx path, and a plain `yard-fake-unit hang` child must fail that.
        place(&brew_dir.path().join("bin/brew"), |dst| std::fs::write(dst, ""));
        place(&brew.nginx_bin(), |dst| std::fs::copy(&fake, dst).map(drop));
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

    /// The smallest nginx.conf the fake nginx understands: a pid path and one listener.
    pub fn write_nginx_conf(&self, port: u16) {
        let conf = format!(
            "pid \"{pid}\";\nhttp {{\n    server {{\n        listen 127.0.0.1:{port};\n    }}\n    include \"{sites}\";\n}}\n",
            pid = self.paths.nginx_pid().display(),
            sites = self.paths.nginx_sites_dir().join("*.conf").display(),
        );
        std::fs::write(self.paths.nginx_conf(), conf).unwrap();
    }
}

/// A test that panics never reaches its shutdown; stop the fake nginx it may have left.
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(pid) =
            std::fs::read_to_string(self.paths.nginx_pid()).unwrap_or_default().trim().parse::<i32>()
        {
            // SAFETY: kill(2) on the pid the fake nginx wrote for this fixture.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
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

/// An OS-assigned port, released at once. macOS allocates ephemeral ports in sequence,
/// so it is not handed out again soon.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Runs `serve` in-process as the current user, its socket under a short `/tmp` dir.
pub async fn spawn_helper(fx: &Fixture, resolver_dir: &Path) -> HelperClient {
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
