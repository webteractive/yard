//! `yard.toml`: default PHP and an optional TLD override.

use super::{config_error, read_optional};
use crate::{Paths, PhpVersion};
use serde::Deserialize;
use std::path::Path;
use yard_proto::YardError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YardConfig {
    pub default_php: PhpVersion,
    pub tld: Option<String>,
}

pub const DEFAULT_TLD: &str = "yard";

impl YardConfig {
    pub fn effective_tld(&self) -> &str {
        self.tld.as_deref().unwrap_or(DEFAULT_TLD)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    /// M1's parked paths. Sites are explicit links since M2; the key is read only to
    /// say so, so an old `yard.toml` keeps serving.
    parked: Option<toml::Value>,
    default_php: Option<PhpVersion>,
    tld: Option<toml::Spanned<String>>,
}

pub fn valid_tld(tld: &str) -> bool {
    let mut bytes = tld.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && tld.len() <= 63
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The config and the warnings reading it produced.
pub fn load_yard(paths: &Paths) -> Result<(YardConfig, Vec<String>), YardError> {
    let file = paths.yard_toml();
    parse_yard(read_optional(&file)?.as_deref(), &file)
}

pub fn parse_yard(text: Option<&str>, file: &Path) -> Result<(YardConfig, Vec<String>), YardError> {
    let raw: Raw = match text {
        Some(text) => toml::from_str(text).map_err(|e| config_error(file, text, e.span(), e.message()))?,
        None => Raw { parked: None, default_php: None, tld: None },
    };
    let mut warnings = vec![];
    if raw.parked.is_some() {
        warnings.push(format!(
            "{}: parked is no longer used (sites are explicit links); remove it",
            file.display()
        ));
    }
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
    Ok((YardConfig { default_php: raw.default_php.unwrap_or(PhpVersion::DEFAULT), tld }, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use yard_proto::ErrorCode;

    const FILE: &str = "/r/yard.toml";

    fn parse(text: Option<&str>) -> Result<YardConfig, YardError> {
        parse_yard(text, Path::new(FILE)).map(|(c, _)| c)
    }

    #[test]
    fn missing_yard_toml_uses_defaults() {
        let (c, warnings) = parse_yard(None, Path::new(FILE)).unwrap();
        assert_eq!(c.default_php, crate::PhpVersion::DEFAULT);
        assert_eq!(c.tld, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn parked_key_is_ignored_with_a_warning() {
        let (c, warnings) =
            parse_yard(Some("parked = [\"~/Herd\"]\ntld = \"dev\"\n"), Path::new(FILE)).unwrap();
        assert_eq!(c.tld.as_deref(), Some("dev"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("parked is no longer used"), "{}", warnings[0]);
        assert!(warnings[0].contains(FILE), "{}", warnings[0]);
    }

    #[test]
    fn config_error_has_location() {
        let text = "tld = \"dev\"\ndefault_php = 1\n";
        let e = parse(Some(text)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        let data = e.data.unwrap();
        assert_eq!(data["file"], FILE);
        assert_eq!(data["line"], 2);
        assert!(data["column"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn tld_defaults_to_yard_whatever_the_profile() {
        assert_eq!(parse(Some("")).unwrap().effective_tld(), "yard");
        assert_eq!(parse(Some("tld = \"test\"\n")).unwrap().effective_tld(), "test");
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
            let text = format!("default_php = \"8.4\"\ntld = {bad:?}\n");
            let e = parse(Some(&text)).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfigInvalid, "{bad}");
            assert!(e.message.contains("tld"), "{}", e.message);
            assert_eq!(e.data.unwrap()["line"], 2, "{bad}");
        }
        assert_eq!(parse(Some("tld = \"dev-1\"\n")).unwrap().tld.as_deref(), Some("dev-1"));
    }

    #[test]
    fn unknown_key_is_rejected_by_name() {
        let e = parse(Some("parkd = []\n")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.message.contains("parkd"), "{}", e.message);
    }

    #[test]
    fn unsupported_php_is_config_invalid() {
        let e = parse(Some("default_php = \"5.6\"\n")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }
}
