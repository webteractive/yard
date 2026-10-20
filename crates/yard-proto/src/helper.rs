//! The root helper's fixed verb set (spec §3.1). Verbs take no paths.

use serde::{Deserialize, Serialize};

pub const RESOLVER_INSTALL: &str = "resolver.install";
pub const RESOLVER_REMOVE: &str = "resolver.remove";
pub const STATUS: &str = "status";

/// Everything the helper accepts. `ca.trust` / `ca.untrust` join in M3.
pub const VERBS: [&str; 3] = [RESOLVER_INSTALL, RESOLVER_REMOVE, STATUS];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolverParams {
    pub tld: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TldParams {
    pub tld: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelperStatus {
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_are_the_fixed_set() {
        assert_eq!(VERBS, ["resolver.install", "resolver.remove", "status"]);
    }
}
