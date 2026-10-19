//! `yardd`: owns the socket, runs the first reconcile, then serves until shutdown while
//! the watchers and the port scan run alongside.

use crate::describe;
use crate::events::EventBus;
use crate::helper_client::HelperClient;
use crate::reconcile::Reconciler;
use crate::supervisor::{Supervisor, SupervisorConfig};
use crate::{portscan, rpc, watch};
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
    pub supervisor: SupervisorConfig,
    pub debounce: Duration,
    pub port_scan_interval: Duration,
}

impl DaemonOptions {
    /// The spec's intervals: 500 ms FSEvents debounce, a port scan every 30 s.
    pub fn production(paths: Paths, brew_prefix: PathBuf) -> Self {
        Self {
            paths,
            brew_prefix,
            helper_socket: PathBuf::from(yard_core::paths::HELPER_SOCKET),
            supervisor: SupervisorConfig::default(),
            debounce: Duration::from_millis(500),
            port_scan_interval: Duration::from_secs(30),
        }
    }
}

/// What every connection and background task shares.
pub struct Context {
    pub paths: Paths,
    pub brew: Brew,
    pub helper: HelperClient,
    pub bus: EventBus,
    pub reconciler: Arc<Reconciler>,
}

impl Context {
    pub fn supervisor(&self) -> &Arc<Supervisor> {
        self.reconciler.supervisor()
    }
}

pub async fn run(opts: DaemonOptions, shutdown: impl Future<Output = ()>) -> Result<(), YardError> {
    let socket = opts.paths.daemon_socket();
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("the socket path {} is longer than {MAX_SOCKET_PATH} bytes", socket.display()),
        )
        .with_hint("use a shorter --root or YARD_ROOT"));
    }
    std::fs::create_dir_all(opts.paths.root())
        .map_err(|e| YardError::internal(format!("cannot create {}: {e}", opts.paths.root().display())))?;
    let bus = EventBus::new();
    let helper = HelperClient::new(&opts.helper_socket);
    let brew = Brew::new(&opts.brew_prefix);
    let supervisor =
        Arc::new(Supervisor::new(opts.supervisor, opts.paths.logs_dir(), helper.clone(), bus.clone()));
    let reconciler = Arc::new(Reconciler::new(
        opts.paths.clone(),
        brew.clone(),
        supervisor.clone(),
        helper.clone(),
        bus.clone(),
    )?);
    let ctx = Arc::new(Context { paths: opts.paths.clone(), brew, helper, bus, reconciler });

    claim(&socket).await?;
    let listener = UnixListener::bind(&socket)
        .map_err(|e| YardError::internal(format!("cannot listen on {}: {e}", socket.display())))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| YardError::internal(format!("cannot set the mode of {}: {e}", socket.display())))?;

    match ctx.reconciler.reconcile().await {
        Ok(report) => report.warnings.iter().for_each(|w| tracing::warn!("{w}")),
        Err(e) => tracing::error!("first reconcile failed: {}", describe(&e)),
    }
    let watcher = watch::spawn(ctx.clone(), opts.debounce);
    let scan = portscan::spawn(ctx.clone(), opts.port_scan_interval);

    rpc::server::serve(listener, ctx.clone(), shutdown).await;

    watcher.abort();
    scan.abort();
    ctx.supervisor().shutdown().await;
    match ctx.helper.stop_nginx().await {
        Err(e) if e.code != ErrorCode::HelperNotInstalled => {
            tracing::warn!("stopping nginx: {}", describe(&e))
        }
        _ => {}
    }
    let _ = std::fs::remove_file(&socket);
    Ok(())
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
