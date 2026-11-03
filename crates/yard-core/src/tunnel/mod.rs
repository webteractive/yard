//! Tunnels (tunnels spec): which hosts and domains are valid, and how they match.

pub mod cloudflared;
pub mod facts;

/// Every tunnel unit's name starts with this.
pub const UNIT_PREFIX: &str = "tunnel-";

/// The unit running a site's tunnel, and its `ports.toml` entry for the metrics port.
pub fn unit(site: &str) -> String {
    format!("{UNIT_PREFIX}{site}")
}

/// The `ports.toml` entry for nginx's loopback listener a site's tunnel connects to.
pub fn origin_entry(site: &str) -> String {
    format!("nginx-tunnel-{site}")
}

/// A lowercase DNS name with at least two labels: letters, digits and hyphens, each label
/// 1–63 characters and not starting or ending with `-`, 253 characters at most.
pub fn valid_host(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').count() >= 2
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The domain set up for `host`: the longest one `host` equals or ends with as `.<domain>`.
pub fn domain_for<'a>(host: &str, domains: &'a [String]) -> Option<&'a str> {
    domains
        .iter()
        .filter(|d| {
            host == d.as_str() || host.strip_suffix(d.as_str()).is_some_and(|rest| rest.ends_with('.'))
        })
        .max_by_key(|d| d.len())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_for_picks_the_longest_match() {
        let d = vec!["example.com".to_string(), "dev.example.com".to_string()];
        assert_eq!(domain_for("a.dev.example.com", &d), Some("dev.example.com"));
        assert_eq!(domain_for("example.com", &d), Some("example.com"));
        assert_eq!(domain_for("badexample.com", &d), None);
    }

    #[test]
    fn tunnel_paths() {
        use crate::Paths;
        use std::path::Path;
        let p = Paths::new("/r");
        assert_eq!(p.tunnels_dir(), Path::new("/r/tunnels"));
        assert_eq!(p.tunnel_yml("my.app"), Path::new("/r/tunnels/my.app.yml"));
        assert_eq!(p.tunnel_overrides_dir(), Path::new("/r/nginx/tunnel-overrides"));
        assert_eq!(p.tunnel_override("blog"), Path::new("/r/nginx/tunnel-overrides/blog.conf"));
        assert_eq!(p.tunnels_state(), Path::new("/r/state/tunnels.json"));
        assert_eq!(
            crate::brew::Brew::new("/opt/homebrew").cloudflared_bin(),
            Path::new("/opt/homebrew/opt/cloudflared/bin/cloudflared")
        );
        assert_eq!(
            (unit("blog"), origin_entry("blog")),
            ("tunnel-blog".to_string(), "nginx-tunnel-blog".to_string())
        );
        assert!(unit("blog").starts_with(UNIT_PREFIX));
        let home = Path::new("/h");
        assert_eq!(cloudflared::cert_dir(home, "example.com"), Path::new("/h/.cloudflared/yard/example.com"));
        assert_eq!(
            cloudflared::cert_path(home, "example.com"),
            Path::new("/h/.cloudflared/yard/example.com/.cloudflared/cert.pem")
        );
        assert_eq!(
            cloudflared::created_credentials(home, "example.com", "yard-blog"),
            Path::new("/h/.cloudflared/yard/example.com/yard-blog.json")
        );
        assert_eq!(cloudflared::default_credentials(home, "1b2c"), Path::new("/h/.cloudflared/1b2c.json"));
    }

    #[test]
    fn valid_hosts() {
        for ok in ["blog.example.com", "a.b", "x-1.example.co"] {
            assert!(valid_host(ok), "{ok}");
        }
        let long = format!("{}.com", "a".repeat(64));
        for bad in ["Blog.example.com", "localhost", "a..b", "-a.b", "a-.b", "a_b.com", "", long.as_str()] {
            assert!(!valid_host(bad), "{bad}");
        }
    }
}
