//! Tunnels (tunnels spec): which hosts and domains are valid, and how they match.

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
