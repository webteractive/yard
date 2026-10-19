//! Naming the process that holds a port, so a conflict says "Herd" rather than a pid.

pub use yard_proto::HolderKind;

/// Rules in order: Herd, brew services, Docker, Yard, else unknown.
pub fn classify(
    name: &str,
    path: Option<&str>,
    ppid: Option<u32>,
    launchd_label: Option<&str>,
) -> HolderKind {
    let path = path.unwrap_or_default();
    if path.contains("/Herd.app/") || path.contains("/Library/Application Support/Herd/") {
        return HolderKind::Herd;
    }
    let launched_by_launchd = ppid == Some(1);
    if launchd_label.is_some_and(|l| l.starts_with("homebrew.mxcl."))
        || (launched_by_launchd && path.starts_with("/opt/homebrew/"))
    {
        return HolderKind::BrewServices;
    }
    if [name, path].iter().any(|s| s.contains("com.docker") || s.contains("vpnkit")) {
        return HolderKind::Docker;
    }
    if path.contains("/Yard.app/") || path.ends_with("/yardd") {
        return HolderKind::Yard;
    }
    HolderKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_herd_holder() {
        let path = "/Applications/Herd.app/Contents/Resources/bin/nginx-arm";
        assert_eq!(classify("nginx", Some(path), Some(1), None), HolderKind::Herd);
        let support = "/Users/me/Library/Application Support/Herd/bin/dnsmasq";
        assert_eq!(classify("dnsmasq", Some(support), Some(1), None), HolderKind::Herd);
    }

    #[test]
    fn classifies_brew_services_and_docker_and_unknown() {
        assert_eq!(
            classify("redis-server", None, Some(1), Some("homebrew.mxcl.redis")),
            HolderKind::BrewServices
        );
        assert_eq!(
            classify("mysqld", Some("/opt/homebrew/Cellar/mysql/9.0/bin/mysqld"), Some(1), None),
            HolderKind::BrewServices
        );
        assert_eq!(
            classify("mysqld", Some("/opt/homebrew/Cellar/mysql/9.0/bin/mysqld"), Some(4242), None),
            HolderKind::Unknown,
            "a brew binary started by hand is not brew services"
        );
        assert_eq!(classify("com.docker.backend", None, Some(1), None), HolderKind::Docker);
        assert_eq!(
            classify("vpnkit-bridge", Some("/Applications/Docker.app/x/vpnkit-bridge"), None, None),
            HolderKind::Docker
        );
        assert_eq!(classify("yardd", Some("/usr/local/bin/yardd"), Some(1), None), HolderKind::Yard);
        assert_eq!(classify("node", Some("/usr/local/bin/node"), Some(77), None), HolderKind::Unknown);
    }
}
