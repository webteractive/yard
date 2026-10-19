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

pub struct State {
    pub paths: Paths,
    pub brew: Brew,
    pub snapshot: Option<Snapshot>,
    /// Set while a TOML file does not parse; the snapshot keeps serving meanwhile.
    pub config_error: Option<YardError>,
    /// The resolver file the helper last wrote, as `(tld, port)`.
    pub applied_resolver: Option<(String, u16)>,
    pub hashes: Hashes,
}

impl State {
    pub fn new(paths: Paths, brew: Brew) -> Result<Self, YardError> {
        let hashes = Hashes::load(&paths.hashes())?;
        Ok(Self { paths, brew, snapshot: None, config_error: None, applied_resolver: None, hashes })
    }
}
