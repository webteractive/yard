//! The CA bundle Yard's PHP verifies against (`php/<v>/ca-bundle.pem`): brew's public roots and
//! Yard's CA, so PHP can call `https://<site>.<tld>` (`yard trust` reaches only the keychain,
//! which brew's OpenSSL and curl never read).

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use yard_core::Paths;
use yard_core::brew::Brew;

/// macOS's own roots, for a brew without `ca-certificates` (every `openssl@<n>` depends on it).
const SYSTEM_ROOTS: &str = "/etc/ssl/cert.pem";

/// The public roots the bundle starts with: brew's `ca-certificates`, else macOS's.
pub fn public_roots(brew: &Brew) -> PathBuf {
    let roots = brew.ca_certificates();
    if roots.is_file() { roots } else { PathBuf::from(SYSTEM_ROOTS) }
}

/// What a bundle was made from: each input's path, size and modification time (through
/// symlinks), so a rescan can tell brew updated its roots or the CA was made again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources(Vec<(PathBuf, u64, Option<SystemTime>)>);

/// The two inputs, read.
pub struct Inputs {
    pub public: String,
    pub ca: String,
    pub sources: Sources,
}

fn stamp(path: &Path) -> Option<(PathBuf, u64, Option<SystemTime>)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((path.to_path_buf(), meta.len(), meta.modified().ok()))
}

/// The inputs' fingerprint now; `None` while there is no Yard CA or no public roots, when
/// PHP keeps brew's own roots.
pub fn sources(paths: &Paths, brew: &Brew) -> Option<Sources> {
    Some(Sources(vec![stamp(&public_roots(brew))?, stamp(&paths.ca_cert())?]))
}

/// The inputs for the bundle, or `None` (see `sources`). Stamped before reading, so a change
/// made while reading is seen by the next rescan.
pub fn read(paths: &Paths, brew: &Brew) -> Option<Inputs> {
    let sources = sources(paths, brew)?;
    let public = std::fs::read_to_string(public_roots(brew)).ok()?;
    let ca = std::fs::read_to_string(paths.ca_cert()).ok()?;
    Some(Inputs { public, ca, sources })
}
