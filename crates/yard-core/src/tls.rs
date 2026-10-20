//! When a site certificate needs issuing (spec §5.4). Pure: the daemon parses the
//! certificates and reads the clock.

/// The CA's validity.
pub const CA_DAYS: i64 = 3650;
/// A site certificate's validity; macOS trusts no TLS leaf valid for longer.
pub const LEAF_DAYS: i64 = 825;
/// A certificate this close to expiry is issued again.
pub const RENEW_BEFORE_DAYS: i64 = 30;

/// A CA with less than this left is made again, since a leaf issued now would outlive it.
pub const CA_RENEW_BEFORE_DAYS: i64 = LEAF_DAYS;

const DAY: i64 = 86_400;

/// Whether a CA that expires at `not_after` must be made again at `now` (unix seconds).
pub fn ca_needs_renewal(not_after: i64, now: i64) -> bool {
    not_after - now < CA_RENEW_BEFORE_DAYS * DAY
}

/// What reconcile reads from a site's certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertFacts {
    pub dns_names: Vec<String>,
    /// Unix seconds.
    pub not_after: i64,
    /// The current `ca/yard-ca.pem` signed it.
    pub signed_by_ca: bool,
}

/// Whether `host`'s certificate must be issued (again) at `now` (unix seconds): there is
/// none, another CA signed it, it does not name exactly `host` and `*.host` (the TLD
/// changed), or it expires within [`RENEW_BEFORE_DAYS`].
pub fn needs_issue(facts: Option<&CertFacts>, host: &str, now: i64) -> bool {
    let Some(f) = facts else { return true };
    let mut names: Vec<&str> = f.dns_names.iter().map(String::as_str).collect();
    names.sort_unstable();
    let wildcard = format!("*.{host}");
    let mut want = [host, wildcard.as_str()];
    want.sort_unstable();
    !f.signed_by_ca || names != want || f.not_after - now < RENEW_BEFORE_DAYS * DAY
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_ca_is_replaced_once_a_new_leaf_would_outlive_it() {
        assert!(!ca_needs_renewal(NOW + (LEAF_DAYS + 1) * DAY, NOW));
        assert!(ca_needs_renewal(NOW + (LEAF_DAYS - 1) * DAY, NOW));
        assert!(ca_needs_renewal(NOW - DAY, NOW), "an expired CA");
    }

    fn facts(names: &[&str], days_left: i64, signed: bool) -> CertFacts {
        CertFacts {
            dns_names: names.iter().map(|n| n.to_string()).collect(),
            not_after: NOW + days_left * DAY,
            signed_by_ca: signed,
        }
    }

    #[test]
    fn needs_issue_cases() {
        let host = "blog.yard";
        let both = ["blog.yard", "*.blog.yard"];
        assert!(needs_issue(None, host, NOW), "no certificate");
        assert!(!needs_issue(Some(&facts(&both, 800, true)), host, NOW), "fresh and matching");
        assert!(!needs_issue(Some(&facts(&["*.blog.yard", "blog.yard"], 800, true)), host, NOW), "any order");
        assert!(needs_issue(Some(&facts(&both, 29, true)), host, NOW), "29 days left");
        assert!(!needs_issue(Some(&facts(&both, 31, true)), host, NOW), "31 days left");
        assert!(needs_issue(Some(&facts(&["blog.test", "*.blog.test"], 800, true)), host, NOW), "old TLD");
        assert!(needs_issue(Some(&facts(&["blog.yard"], 800, true)), host, NOW), "no wildcard");
        assert!(needs_issue(Some(&facts(&both, 800, false)), host, NOW), "another CA signed it");
    }

    #[test]
    fn validity_constants_follow_the_spec() {
        assert_eq!((CA_DAYS, LEAF_DAYS, RENEW_BEFORE_DAYS), (3650, 825, 30));
    }
}
