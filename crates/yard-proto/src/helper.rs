//! The root helper's fixed verb set (spec §3.1). Verbs take no paths.

use serde::{Deserialize, Serialize};

pub const NGINX_START: &str = "nginx.start";
pub const NGINX_STOP: &str = "nginx.stop";
pub const NGINX_RELOAD: &str = "nginx.reload";
pub const NGINX_TEST_CONFIG: &str = "nginx.test_config";
pub const RESOLVER_INSTALL: &str = "resolver.install";
pub const RESOLVER_REMOVE: &str = "resolver.remove";
pub const STATUS: &str = "status";

/// Everything the helper accepts. `ca.trust` / `ca.untrust` join in M3.
pub const VERBS: [&str; 7] = [
    NGINX_START,
    NGINX_STOP,
    NGINX_RELOAD,
    NGINX_TEST_CONFIG,
    RESOLVER_INSTALL,
    RESOLVER_REMOVE,
    STATUS,
];

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
pub struct TestConfigResult {
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelperStatus {
    pub version: String,
    pub nginx_pid: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_are_the_fixed_set() {
        assert_eq!(
            VERBS,
            [
                "nginx.start",
                "nginx.stop",
                "nginx.reload",
                "nginx.test_config",
                "resolver.install",
                "resolver.remove",
                "status"
            ]
        );
    }
}
