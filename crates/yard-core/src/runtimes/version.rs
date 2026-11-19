//! Runtime versions and the specs that pick them (runtimes spec §6.1).

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

pub use crate::node::spec_line;

/// An installed or published version, `2.2.32`: one to four dotted numbers. Compared
/// numerically, a missing part counting as 0 (`2.2` < `2.2.1`, `2.2` = `2.2.0`).
#[derive(Debug, Clone)]
pub struct RuntimeVersion(pub Vec<u32>);

fn numbers(s: &str, max: usize) -> Option<Vec<u32>> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let parts: Option<Vec<u32>> = s
        .split('.')
        .map(|p| if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) { p.parse().ok() } else { None })
        .collect();
    parts.filter(|p| !p.is_empty() && p.len() <= max)
}

impl FromStr for RuntimeVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        numbers(s, 4).map(RuntimeVersion).ok_or_else(|| format!("\"{}\" is not a version", s.trim()))
    }
}

impl fmt::Display for RuntimeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&dotted(&self.0))
    }
}

fn dotted(parts: &[u32]) -> String {
    parts.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
}

impl Ord for RuntimeVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let n = self.0.len().max(other.0.len());
        (0..n)
            .map(|i| self.0.get(i).copied().unwrap_or(0).cmp(&other.0.get(i).copied().unwrap_or(0)))
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for RuntimeVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for RuntimeVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for RuntimeVersion {}

impl RuntimeVersion {
    /// Whether the version begins with every part the spec names (`2.2.32` meets `2`,
    /// `2.2` and `2.2.32`, not `2.11`).
    pub fn prefix_matches(&self, spec: &VersionSpec) -> bool {
        spec.parts.iter().enumerate().all(|(i, p)| self.0.get(i).copied().unwrap_or(0) == *p)
    }
}

/// What a pin or a command asks for: a major (`2`), a major.minor (`2.2`) or an exact
/// version (`2.2.32`), with a leading `v` accepted. Ranges, `latest` and `stable` are not
/// specs: a project pins what it was tested on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[serde(try_from = "String")]
pub struct VersionSpec {
    pub parts: Vec<u32>,
}

impl TryFrom<String> for VersionSpec {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        VersionSpec::parse(&s)
    }
}

impl VersionSpec {
    pub fn parse(s: &str) -> Result<VersionSpec, String> {
        numbers(s, 3).map(|parts| VersionSpec { parts }).ok_or_else(|| {
            format!("\"{}\" is not a version spec; write a version such as 2.2 or 2.2.32", s.trim())
        })
    }

    /// An exact version (three parts).
    pub fn is_exact(&self) -> bool {
        self.parts.len() == 3
    }
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&dotted(&self.parts))
    }
}

/// The newest of `installed` that meets `spec`.
pub fn pick(spec: &VersionSpec, installed: &[RuntimeVersion]) -> Option<RuntimeVersion> {
    installed.iter().filter(|v| v.prefix_matches(spec)).max().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> RuntimeVersion {
        s.parse().unwrap()
    }

    fn spec(s: &str) -> VersionSpec {
        VersionSpec::parse(s).unwrap()
    }

    #[test]
    fn parses_majors_minors_and_exact_versions() {
        for ok in ["2", "2.2", "2.2.32", "v2.11.2", " 2.2 "] {
            assert!(VersionSpec::parse(ok).is_ok(), "{ok}");
        }
        assert_eq!(spec("v2.11.2").to_string(), "2.11.2");
        assert_eq!(spec(" 2.2 ").to_string(), "2.2");
        for bad in ["^2", ">=1", "latest", "stable", "2.x", "", "2.2.3.4"] {
            assert!(VersionSpec::parse(bad).is_err(), "{bad}");
        }
        assert!(VersionSpec::parse("^2").unwrap_err().contains("a version such as 2.2"));
    }

    #[test]
    fn orders_numerically() {
        assert!(v("2.11.2") > v("2.2.32"));
        assert!(v("2.2") < v("2.2.1"));
        assert_eq!(v("v2.2.32").to_string(), "2.2.32");
    }

    #[test]
    fn pick_takes_the_newest_match() {
        let installed = [v("2.2.32"), v("2.11.2"), v("2.10.0")];
        assert_eq!(pick(&spec("2"), &installed), Some(v("2.11.2")));
        let lts = [v("2.2.32")];
        assert_eq!(pick(&spec("2"), &lts), Some(v("2.2.32")));
        assert_eq!(pick(&spec("2.2.32"), &lts), Some(v("2.2.32")));
    }
}
