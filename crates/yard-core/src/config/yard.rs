//! `yard.toml`: parked paths, default PHP, optional TLD override.

use super::{config_error, read_optional};
use crate::{Paths, PhpVersion};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YardConfig {
    pub parked: Vec<PathBuf>,
    pub default_php: PhpVersion,
    /// Overrides the active port profile's TLD when set.
    pub tld: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    parked: Option<Vec<String>>,
    default_php: Option<PhpVersion>,
    tld: Option<String>,
}

pub fn load_yard(paths: &Paths) -> Result<YardConfig, YardError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| YardError::new(ErrorCode::Internal, "HOME is not set"))?;
    let file = paths.yard_toml();
    parse_yard(read_optional(&file)?.as_deref(), &file, &home)
}

/// A missing file (`text = None`) means all defaults.
pub fn parse_yard(text: Option<&str>, file: &Path, home: &Path) -> Result<YardConfig, YardError> {
    let raw: Raw = match text {
        Some(text) => toml::from_str(text).map_err(|e| config_error(file, text, e.span(), e.message()))?,
        None => Raw { parked: None, default_php: None, tld: None },
    };
    let parked = match raw.parked {
        Some(list) => list.iter().map(|p| expand_tilde(p, home)).collect(),
        None => vec![home.join("Herd")],
    };
    Ok(YardConfig { parked, default_php: raw.default_php.unwrap_or(PhpVersion::DEFAULT), tld: raw.tld })
}

fn expand_tilde(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if p == "~" => home.to_path_buf(),
        None => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use yard_proto::ErrorCode;

    const FILE: &str = "/r/yard.toml";

    #[test]
    fn missing_yard_toml_uses_defaults() {
        let c = parse_yard(None, Path::new(FILE), Path::new("/Users/me")).unwrap();
        assert_eq!(c.parked, vec![PathBuf::from("/Users/me/Herd")]);
        assert_eq!(c.default_php, crate::PhpVersion::DEFAULT);
        assert_eq!(c.tld, None);
    }

    #[test]
    fn tilde_in_parked_is_expanded() {
        let c =
            parse_yard(Some("parked = [\"~/code\", \"/abs\"]\n"), Path::new(FILE), Path::new("/Users/me"))
                .unwrap();
        assert_eq!(c.parked, vec![PathBuf::from("/Users/me/code"), PathBuf::from("/abs")]);
    }

    #[test]
    fn config_error_has_location() {
        let text = "parked = [\"/a\"]\ndefault_php = 1\n";
        let e = parse_yard(Some(text), Path::new(FILE), Path::new("/h")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        let data = e.data.unwrap();
        assert_eq!(data["file"], FILE);
        assert_eq!(data["line"], 2);
        assert!(data["column"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn unknown_key_is_rejected_by_name() {
        let e = parse_yard(Some("parkd = []\n"), Path::new(FILE), Path::new("/h")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.message.contains("parkd"), "{}", e.message);
    }

    #[test]
    fn unsupported_php_is_config_invalid() {
        let e = parse_yard(Some("default_php = \"5.6\"\n"), Path::new(FILE), Path::new("/h")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }
}
