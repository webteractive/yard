//! On-demand services (M4e spec §4): how long an instance may go without use before it
//! sleeps, and `run/sleeping`, the sites whose instances sleep, which the shims read.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use yard_proto::SettingSource;

/// The shortest `sleep_after`: a shorter one would stop an instance between two requests.
pub const MIN_SLEEP_SECS: u64 = 60;

/// How long an instance with no `sleep_after` of its own, and none in `yard.toml`, may go
/// unused before it sleeps (maintainer, 2026-11-03; sleeping was opt-in before). `"off"` on the
/// instance or in `yard.toml` keeps instances awake.
pub const DEFAULT_SLEEP_AFTER: SleepAfter = SleepAfter::After(15 * 60);

/// `service_sleep_after` in `yard.toml` or an instance's `sleep_after` in `services.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum SleepAfter {
    Off,
    /// Seconds without use.
    After(u64),
}

impl FromStr for SleepAfter {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        if text == "off" {
            return Ok(SleepAfter::Off);
        }
        let bad =
            || format!("{text:?} is not a duration: write \"off\" or a number with s, m or h, like \"15m\"");
        let unit = match text.as_bytes().last() {
            Some(b's') => 1,
            Some(b'm') => 60,
            Some(b'h') => 3600,
            _ => return Err(bad()),
        };
        let digits = &text[..text.len() - 1];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let secs = digits.parse::<u64>().ok().and_then(|n| n.checked_mul(unit)).ok_or_else(bad)?;
        if secs < MIN_SLEEP_SECS {
            return Err(format!("{text:?} is too short: an instance sleeps after at least 60s"));
        }
        Ok(SleepAfter::After(secs))
    }
}

impl TryFrom<String> for SleepAfter {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
    }
}

impl fmt::Display for SleepAfter {
    /// The largest whole unit: `900` is `15m`, `90` is `90s`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            SleepAfter::Off => f.write_str("off"),
            SleepAfter::After(secs) if secs % 3600 == 0 => write!(f, "{}h", secs / 3600),
            SleepAfter::After(secs) if secs % 60 == 0 => write!(f, "{}m", secs / 60),
            SleepAfter::After(secs) => write!(f, "{secs}s"),
        }
    }
}

/// The seconds an instance may idle, and where that comes from: its own key wins over
/// `yard.toml`'s, and either over `default` (yardd's [`DEFAULT_SLEEP_AFTER`]); `off` is `None`.
pub fn effective_sleep(
    instance: Option<SleepAfter>,
    machine: Option<SleepAfter>,
    default: Option<SleepAfter>,
) -> (Option<u64>, SettingSource) {
    let (setting, source) = match (instance, machine, default) {
        (Some(s), _, _) => (s, SettingSource::ServicesToml),
        (None, Some(s), _) => (s, SettingSource::YardToml),
        (None, None, Some(s)) => (s, SettingSource::Default),
        (None, None, None) => return (None, SettingSource::Default),
    };
    match setting {
        SleepAfter::Off => (None, source),
        SleepAfter::After(secs) => (Some(secs), source),
    }
}

/// `run/sleeping`: one site per line.
pub fn sleeping_text(sites: &BTreeSet<String>) -> String {
    sites.iter().map(|site| format!("{site}\n")).collect()
}

/// Whether `run/sleeping` lists `site`.
pub fn lists_site(text: &str, site: &str) -> bool {
    text.lines().any(|line| line.trim() == site)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use yard_proto::SettingSource;

    #[test]
    fn sleep_after_parses_units_and_off() {
        assert_eq!("90s".parse::<SleepAfter>(), Ok(SleepAfter::After(90)));
        assert_eq!("15m".parse::<SleepAfter>(), Ok(SleepAfter::After(900)));
        assert_eq!("2h".parse::<SleepAfter>(), Ok(SleepAfter::After(7200)));
        assert_eq!("off".parse::<SleepAfter>(), Ok(SleepAfter::Off));
        for text in ["15m", "1h", "90s", "off"] {
            assert_eq!(text.parse::<SleepAfter>().unwrap().to_string(), text);
        }
        assert_eq!(SleepAfter::After(900).to_string(), "15m");
        assert_eq!(SleepAfter::After(3600).to_string(), "1h");
    }

    #[test]
    fn sleep_after_refuses_short_and_bad() {
        for bad in ["59s", "0m", "15", "15 m", "1d", "-5m", ""] {
            assert!(bad.parse::<SleepAfter>().is_err(), "{bad:?}");
        }
        let e = "59s".parse::<SleepAfter>().unwrap_err();
        assert!(e.contains("at least 60s"), "{e}");
    }

    #[test]
    fn effective_sleep_instance_then_machine_then_the_default() {
        use SleepAfter::*;
        let default = Some(DEFAULT_SLEEP_AFTER);
        assert_eq!(
            effective_sleep(Some(After(3600)), Some(After(900)), default),
            (Some(3600), SettingSource::ServicesToml)
        );
        assert_eq!(effective_sleep(None, Some(After(1800)), default), (Some(1800), SettingSource::YardToml));
        assert_eq!(
            effective_sleep(Some(Off), Some(After(900)), default),
            (None, SettingSource::ServicesToml)
        );
        assert_eq!(effective_sleep(None, Some(Off), default), (None, SettingSource::YardToml));
        assert_eq!(effective_sleep(None, None, default), (Some(900), SettingSource::Default));
        assert_eq!(effective_sleep(None, None, None), (None, SettingSource::Default));
        assert_eq!(DEFAULT_SLEEP_AFTER.to_string(), "15m");
    }

    #[test]
    fn sleeping_text_lists_sites() {
        let sites: BTreeSet<String> = ["shop", "blog"].map(String::from).into();
        assert_eq!(sleeping_text(&sites), "blog\nshop\n");
        assert!(lists_site("blog\nshop\n", "shop"));
        assert!(!lists_site("blog\nshop\n", "sho"));
        assert!(!lists_site("", "shop"));
    }
}
