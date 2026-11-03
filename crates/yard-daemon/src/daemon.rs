//! `yardd`: owns the socket, runs the first reconcile, then serves until shutdown while
//! the watchers and the port scan run alongside.

use crate::describe;
use crate::events::EventBus;
use crate::helper_client::HelperClient;
use crate::jobs::Jobs;
use crate::privileged::PrivilegedSockets;
use crate::reconcile::Reconciler;
use crate::sleep::{SleepConfig, Sleeper};
use crate::supervisor::{Supervisor, SupervisorConfig};
use crate::{portscan, rpc, services, sleep, tunnel, wake, watch};
use std::collections::BTreeMap;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_proto::{ErrorCode, YardError};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("YARD_BUILD_COMMIT");
pub const BUILD_DATE: &str = env!("YARD_BUILD_DATE");

/// macOS `sun_path` is 104 bytes including the terminating NUL.
const MAX_SOCKET_PATH: usize = 103;

#[derive(Debug, Clone)]
pub struct DaemonOptions {
    pub paths: Paths,
    pub brew_prefix: PathBuf,
    pub helper_socket: PathBuf,
    /// Where `doctor` looks for the resolver file the helper writes.
    pub resolver_dir: PathBuf,
    pub supervisor: SupervisorConfig,
    pub debounce: Duration,
    pub port_scan_interval: Duration,
    /// The `yard-shim` binary `bin/` links to; `None` leaves `bin/` alone.
    pub shim: Option<PathBuf>,
    /// `/usr/bin/security`, which `ca.info` asks whether the login keychain trusts the CA.
    pub security: PathBuf,
    /// The port each service is offered first, keyed `mysql`, `postgres`, `redis`, `minio`
    /// and `minio-console`. `None` is the engine table's (3306, 5432, 6379, 9000, 9001);
    /// tests set every one from `free_port()`, so they never bind a real database's port.
    pub usual_ports: Option<BTreeMap<String, u16>>,
    /// Where DBngin.app and Postgres.app would be installed. Their usual ports stay unpinned
    /// while either is, since a probe only sees a running server.
    pub app_dirs: Vec<PathBuf>,
    /// How the idle loop samples service instances and keeps time (M4e).
    pub sleep: SleepConfig,
    /// The user's home, where emailulator's installer puts its binary (`.local/bin`), and the
    /// `HOME` cloudflared runs with.
    pub home: PathBuf,
    /// How often each running tunnel's metrics server is asked whether it is ready (M5).
    pub tunnel_poll: Duration,
    /// How long `tunnels.add` waits for a tunnel to reach Cloudflare.
    pub tunnel_ready_wait: Duration,
}

impl DaemonOptions {
    /// The spec's intervals: 500 ms FSEvents debounce, a port scan every 30 s.
    pub fn production(paths: Paths, brew_prefix: PathBuf) -> Self {
        Self {
            paths,
            brew_prefix,
            helper_socket: PathBuf::from(yard_core::paths::HELPER_SOCKET),
            resolver_dir: PathBuf::from(yard_core::paths::RESOLVER_DIR),
            supervisor: SupervisorConfig::default(),
            debounce: Duration::from_millis(500),
            port_scan_interval: Duration::from_secs(30),
            // Installed next to yardd.
            shim: std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("yard-shim"))),
            security: PathBuf::from("/usr/bin/security"),
            usual_ports: None,
            app_dirs: app_dirs(),
            sleep: SleepConfig::production(),
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
            tunnel_poll: Duration::from_secs(2),
            tunnel_ready_wait: Duration::from_secs(30),
        }
    }
}

/// `/Applications` and `~/Applications`.
fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    dirs.extend(std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications")));
    dirs
}

/// What every connection and background task shares.
pub struct Context {
    pub paths: Paths,
    pub brew: Brew,
    pub helper: HelperClient,
    pub bus: EventBus,
    pub reconciler: Arc<Reconciler>,
    pub resolver_dir: PathBuf,
    pub jobs: Jobs,
    pub security: PathBuf,
    /// `DaemonOptions.usual_ports` and `app_dirs`, which `services.add` allocates ports from.
    pub usual_ports: Option<BTreeMap<String, u16>>,
    pub app_dirs: Vec<PathBuf>,
    /// Which service instances sleep; the reconciler holds the same one.
    pub sleeper: Arc<Sleeper>,
    /// The quick tunnels and what running tunnels said; the reconciler holds the same one.
    pub tunnels: Arc<tunnel::Runtime>,
    /// `DaemonOptions.tunnel_ready_wait`.
    pub tunnel_ready_wait: Duration,
    /// The asks that may wait on a wake at once (`wake::WAKE_SLOTS`).
    pub wake_slots: Arc<tokio::sync::Semaphore>,
}

impl Context {
    pub fn supervisor(&self) -> &Arc<Supervisor> {
        self.reconciler.supervisor()
    }
}

pub async fn run(opts: DaemonOptions, shutdown: impl Future<Output = ()>) -> Result<(), YardError> {
    let socket = opts.paths.daemon_socket();
    let wake_socket = opts.paths.wake_socket();
    for socket in [&socket, &wake_socket] {
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!("the socket path {} is longer than {MAX_SOCKET_PATH} bytes", socket.display()),
            )
            .with_hint("use a shorter --root or YARD_ROOT"));
        }
    }
    // `sites/` too, so the watcher sees the first managed `.yard` written into it, and `run/`
    // for the wake socket.
    for dir in [opts.paths.root().to_path_buf(), opts.paths.managed_sites_dir(), opts.paths.run_dir()] {
        std::fs::create_dir_all(&dir)
            .map_err(|e| YardError::internal(format!("cannot create {}: {e}", dir.display())))?;
    }
    private_root(opts.paths.root())?;
    let bus = EventBus::new();
    let helper = HelperClient::new(&opts.helper_socket);
    let brew = Brew::new(&opts.brew_prefix);
    let sockets = Arc::new(PrivilegedSockets::new(helper.clone()));
    let supervisor = Arc::new(Supervisor::new(opts.supervisor, opts.paths.logs_dir(), bus.clone(), sockets));
    let sleeper = Arc::new(Sleeper::new(opts.sleep.default_after));
    // Quick tunnels end with yardd: a new one starts with none.
    let tunnels = Arc::new(tunnel::Runtime::default());
    let reconciler = Arc::new(
        Reconciler::new(
            opts.paths.clone(),
            brew.clone(),
            supervisor.clone(),
            helper.clone(),
            bus.clone(),
            opts.resolver_dir.clone(),
        )?
        .with_shim(opts.shim.clone())
        .with_services(opts.usual_ports.clone(), opts.app_dirs.clone())
        .with_sleeper(sleeper.clone())
        .with_tunnels(tunnels.clone())
        .with_home(opts.home.clone()),
    );
    let jobs = Jobs::new(bus.clone());
    let ctx = Arc::new(Context {
        paths: opts.paths.clone(),
        brew,
        helper,
        bus,
        reconciler,
        resolver_dir: opts.resolver_dir.clone(),
        jobs,
        security: opts.security.clone(),
        usual_ports: opts.usual_ports.clone(),
        app_dirs: opts.app_dirs.clone(),
        sleeper,
        tunnels,
        tunnel_ready_wait: opts.tunnel_ready_wait,
        wake_slots: Arc::new(tokio::sync::Semaphore::new(crate::wake::WAKE_SLOTS)),
    });

    claim(&socket).await?;
    let listener = UnixListener::bind(&socket)
        .map_err(|e| YardError::internal(format!("cannot listen on {}: {e}", socket.display())))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| YardError::internal(format!("cannot set the mode of {}: {e}", socket.display())))?;
    // yardd.sock is ours now, so a wake.sock left behind is a killed yardd's.
    let _ = std::fs::remove_file(&wake_socket);
    let wake_listener = UnixListener::bind(&wake_socket)
        .map_err(|e| YardError::internal(format!("cannot listen on {}: {e}", wake_socket.display())))?;
    std::fs::set_permissions(&wake_socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| YardError::internal(format!("cannot set the mode of {}: {e}", wake_socket.display())))?;

    // A service a killed yardd left running holds its port and data directory. This runs one
    // instance after another, up to the stop timeout (30 s) for each orphan that ignores its
    // stop signal, before the watchers start; the socket is bound, so clients wait meanwhile.
    for (instance, pid) in services::stop_orphans(&opts.paths, &ctx.brew).await {
        tracing::warn!("stopped {instance} (pid {pid}), which an earlier yardd left running");
    }
    // Every instance starts awake (M4e spec §6.4), so a list a killed yardd left is stale.
    let sleeping = opts.paths.sleeping_file();
    if let Err(e) = yard_core::generated::write_atomic(&sleeping, "") {
        tracing::warn!("cannot empty {}: {e}", sleeping.display());
    }
    match ctx.reconciler.reconcile().await {
        Ok(report) => report.warnings.iter().for_each(|w| tracing::warn!("{w}")),
        Err(e) => tracing::error!("first reconcile failed: {}", describe(&e)),
    }
    // Docroots are rechecked on the port scan's cadence.
    let watcher = watch::spawn(ctx.clone(), opts.debounce, opts.port_scan_interval).await;
    let scan = portscan::spawn(ctx.clone(), opts.port_scan_interval);
    let idle = sleep::spawn(ctx.clone(), opts.sleep);
    let wakes = tokio::spawn(wake::serve(wake_listener, ctx.clone()));
    let monitor = tunnel::monitor::spawn(ctx.clone(), opts.tunnel_poll);

    rpc::server::serve(listener, ctx.clone(), shutdown).await;

    watcher.abort();
    scan.abort();
    idle.abort();
    wakes.abort();
    monitor.abort();
    let _ = std::fs::remove_file(&wake_socket);
    ctx.supervisor().shutdown().await;
    // The port goes back to the system: the next yardd asks the helper again.
    ctx.supervisor().sockets().release_except(&[]);
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Keeps the data root `0700`. `~/.yard` sits in a home folder other local users can list,
/// and the root holds logs, configs and service data that no one else should read; the keys
/// and sockets in it carry their own `0600` as well.
fn private_root(root: &Path) -> Result<(), YardError> {
    let mode = std::fs::metadata(root)
        .map_err(|e| YardError::internal(format!("cannot read {}: {e}", root.display())))?
        .permissions()
        .mode();
    if mode & 0o077 == 0 {
        return Ok(());
    }
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
        YardError::internal(format!("cannot make {} private: {e}", root.display()))
            .with_hint(format!("chmod 700 {}", root.display()))
    })
}

/// A socket that still answers belongs to a running yardd; one that refuses is stale.
async fn claim(socket: &Path) -> Result<(), YardError> {
    if std::fs::symlink_metadata(socket).is_err() {
        return Ok(());
    }
    if UnixStream::connect(socket).await.is_ok() {
        return Err(YardError::internal(format!("yardd is already running on {}", socket.display()))
            .with_hint("yard status talks to it; yard stop stops it"));
    }
    std::fs::remove_file(socket)
        .map_err(|e| YardError::internal(format!("cannot remove the stale {}: {e}", socket.display())))
}
