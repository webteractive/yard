//! What the daemon last applied, and what went wrong since.

use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::{SitesConfig, YardConfig};
use yard_core::generated::Hashes;
use yard_core::ports::Registry;
use yard_core::sites::Resolution;
use yard_proto::YardError;

/// The last configuration that reconciled cleanly.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub yard: YardConfig,
    pub sites_cfg: SitesConfig,
    pub registry: Registry,
    pub tld: String,
    pub resolution: Resolution,
}

#[derive(Clone)]
pub struct State {
    pub paths: Paths,
    pub brew: Brew,
    pub snapshot: Option<Snapshot>,
    /// Why the last reconcile failed (a TOML that does not parse, nginx rejecting the
    /// config, …), until one succeeds. The snapshot keeps serving meanwhile.
    pub last_error: Option<YardError>,
    /// The resolver file the helper last wrote, as `(tld, port)`.
    pub applied_resolver: Option<(String, u16)>,
    pub hashes: Hashes,
    /// A certificate was issued that the running nginx has not loaded yet, kept across
    /// a reconcile that failed before it could reload.
    pub nginx_reload_pending: bool,
}

impl State {
    pub fn new(paths: Paths, brew: Brew) -> Result<Self, YardError> {
        let hashes = Hashes::load(&paths.hashes())?;
        Ok(Self {
            paths,
            brew,
            snapshot: None,
            last_error: None,
            applied_resolver: None,
            hashes,
            nginx_reload_pending: false,
        })
    }
}
