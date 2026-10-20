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
    /// The TLD; `DEFAULT_TLD` when unset. `yard takeover` (M3) sets it to `test`.
    pub tld: Option<String>,
}

/// The TLD before `yard takeover`, whatever the port profile (spec §5.3).
pub const DEFAULT_TLD: &str = "yard";

impl YardConfig {
    pub fn effective_tld(&self) -> &str {
        self.tld.as_deref().unwrap_or(DEFAULT_TLD)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    parked: Option<Vec<String>>,
    default_php: Option<PhpVersion>,
    tld: Option<toml::Spanned<String>>,
}

/// `^[a-z][a-z0-9-]{0,62}$`: a DNS label, and a file name that cannot leave `/etc/resolver`.
pub fn valid_tld(tld: &str) -> bool {
    let mut bytes = tld.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && tld.len() <= 63
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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
    let tld = match raw.tld {
        Some(t) if !valid_tld(t.get_ref()) => {
            let message = format!(
                "tld {:?} must be lowercase letters, digits and dashes, starting with a letter",
                t.get_ref()
            );
            return Err(config_error(file, text.unwrap_or_default(), Some(t.span()), &message));
        }
        t => t.map(toml::Spanned::into_inner),
    };
    Ok(YardConfig { parked, default_php: raw.default_php.unwrap_or(PhpVersion::DEFAULT), tld })
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
    fn tld_defaults_to_yard_whatever_the_profile() {
        let c = parse_yard(Some("parked = []\n"), Path::new(FILE), Path::new("/h")).unwrap();
        assert_eq!(c.effective_tld(), "yard");
        let c = parse_yard(Some("tld = \"test\"\n"), Path::new(FILE), Path::new("/h")).unwrap();
        assert_eq!(c.effective_tld(), "test");
    }

    #[test]
    fn tld_pattern() {
        for ok in ["yard", "test", "a", "dev-1", &"a".repeat(63)] {
            assert!(valid_tld(ok), "{ok}");
        }
        for bad in ["", "Yard", "1yard", "-yard", "ya.rd", "../etc", "ya/rd", &"a".repeat(64)] {
            assert!(!valid_tld(bad), "{bad}");
        }
    }

    #[test]
    fn invalid_tld_is_rejected_with_location() {
        for bad in [".yard", "Yard", "ya;rd", ""] {
            let text = format!("parked = []\ntld = {bad:?}\n");
            let e = parse_yard(Some(&text), Path::new(FILE), Path::new("/h")).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfigInvalid, "{bad}");
            assert!(e.message.contains("tld"), "{}", e.message);
            assert_eq!(e.data.unwrap()["line"], 2, "{bad}");
        }
        let ok = parse_yard(Some("tld = \"dev-1\"\n"), Path::new(FILE), Path::new("/h")).unwrap();
        assert_eq!(ok.tld.as_deref(), Some("dev-1"));
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
