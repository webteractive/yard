//! What the daemon last applied, and what went wrong since.

use crate::emailulator::Probe;
use crate::supervisor::UnitSpec;
use std::collections::BTreeMap;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::{ServicesConfig, SitesConfig, YardConfig};
use yard_core::generated::Hashes;
use yard_core::ports::Registry;
use yard_core::project::{IgnoredFiles, Projects};
use yard_core::sites::Resolution;
use yard_proto::YardError;

/// The last configuration that reconciled cleanly.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub yard: YardConfig,
    pub sites_cfg: SitesConfig,
    /// The service instances `services.toml` lists.
    pub services: ServicesConfig,
    /// The `.yard` each linked site is served from, as last read.
    pub projects: Projects,
    /// The managed `.yard` files that exist but are not used, by site.
    pub ignored_managed: IgnoredFiles,
    pub registry: Registry,
    pub tld: String,
    pub resolution: Resolution,
    /// The instances in `services` that are not stopped but have no unit, and why: their keg
    /// or binary is missing (`FORMULA_MISSING`, hint `brew install|reinstall <formula>`), or
    /// their port could not be had. `services.list` shows them as failed with that reason, and
    /// `services.start` and `restart` refuse with it.
    pub left_out: BTreeMap<String, YardError>,
    /// What emailulator said at this reconcile; `None` when Yard found no binary (M4d).
    pub emailulator: Option<Probe>,
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
    /// The last time emailulator was asked, whether or not that reconcile succeeded: its
    /// agent is kept when a later answer cannot be read.
    pub emailulator: Option<Probe>,
    /// The units the last reconcile to reach the supervisor asked for, and the unit each
    /// asleep instance would run: a wake starts from these while a TOML does not parse.
    pub applied: Vec<UnitSpec>,
    pub slept: BTreeMap<String, UnitSpec>,
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
            emailulator: None,
            applied: vec![],
            slept: BTreeMap::new(),
        })
    }
}
