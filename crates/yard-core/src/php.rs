//! PHP versions Yard supports (spec §4.1).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use yard_proto::{ErrorCode, YardError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhpVersion {
    pub major: u8,
    pub minor: u8,
}

const fn v(major: u8, minor: u8) -> PhpVersion {
    PhpVersion { major, minor }
}

pub const SUPPORTED: [PhpVersion; 11] =
    [v(7, 0), v(7, 1), v(7, 2), v(7, 3), v(7, 4), v(8, 0), v(8, 1), v(8, 2), v(8, 3), v(8, 4), v(8, 5)];

impl PhpVersion {
    pub const DEFAULT: PhpVersion = v(8, 4);
}

impl fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for PhpVersion {
    type Err = YardError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let unsupported = || {
            YardError::new(
                ErrorCode::InvalidArgument,
                format!("PHP {s:?} is not supported; Yard supports 7.0–8.5"),
            )
        };
        let (major, minor) = s.split_once('.').ok_or_else(unsupported)?;
        let parsed = v(major.parse().map_err(|_| unsupported())?, minor.parse().map_err(|_| unsupported())?);
        if SUPPORTED.contains(&parsed) { Ok(parsed) } else { Err(unsupported()) }
    }
}

impl Serialize for PhpVersion {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PhpVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(|e: YardError| serde::de::Error::custom(e.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::ErrorCode;

    #[test]
    fn supported_php_range_is_7_0_to_8_5() {
        for bad in ["6.9", "8.6", "5.6", "8", "8.x", ""] {
            let e = bad.parse::<PhpVersion>().unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{bad}");
        }
        assert!("8.6".parse::<PhpVersion>().unwrap_err().message.contains("7.0–8.5"));
        assert_eq!("7.0".parse::<PhpVersion>().unwrap().to_string(), "7.0");
        assert_eq!("8.5".parse::<PhpVersion>().unwrap().to_string(), "8.5");
        assert_eq!(SUPPORTED.len(), 11);
        assert_eq!(PhpVersion::DEFAULT.to_string(), "8.4");
        assert!("7.4".parse::<PhpVersion>().unwrap() < "8.0".parse().unwrap());
    }

    #[test]
    fn serde_as_string() {
        let v: PhpVersion = serde_json::from_str("\"7.1\"").unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"7.1\"");
    }
}
