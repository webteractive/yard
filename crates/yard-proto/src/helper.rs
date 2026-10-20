//! The root helper's fixed verb set (spec §3.1). Verbs take no paths.

use serde::{Deserialize, Serialize};

pub const RESOLVER_INSTALL: &str = "resolver.install";
pub const RESOLVER_REMOVE: &str = "resolver.remove";
pub const PORTS_BIND: &str = "ports.bind";
pub const STATUS: &str = "status";

/// Everything the helper accepts. CA trust is the user's own (`yard trust`), not root's.
pub const VERBS: [&str; 4] = [RESOLVER_INSTALL, RESOLVER_REMOVE, PORTS_BIND, STATUS];

/// The only ports `ports.bind` opens: what the privileged profile serves on.
pub const PRIVILEGED_PORTS: [u16; 2] = [80, 443];

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
pub struct BindParams {
    pub port: u16,
}

/// One address per socket handed over with the answer, in the same order:
/// `["127.0.0.1:80", "[::1]:80"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindResult {
    pub addresses: Vec<String>,
}

/// What `yard helper install` did: the helper now running, and the files M1's root
/// nginx left behind that it removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelperInstall {
    pub version: String,
    pub removed: Vec<String>,
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
        assert_eq!(VERBS, ["resolver.install", "resolver.remove", "ports.bind", "status"]);
    }
}
