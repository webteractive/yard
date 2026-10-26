//! `yard.toml`: default PHP, an optional TLD override and each version's extensions.

use super::{config_error, read_optional};
use crate::node::NodeSpec;
use crate::{Paths, PhpVersion};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_proto::YardError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YardConfig {
    pub default_php: PhpVersion,
    pub tld: Option<String>,
    /// Sites without their own `secure` are served over HTTPS (spec §5.4).
    pub secure_by_default: bool,
    /// `[php."<v>".extensions]`: the extensions Yard loads for each version (spec §4.3).
    pub extensions: BTreeMap<PhpVersion, BTreeMap<String, ExtEntry>>,
    /// `yard node default`: what runs where nothing pins a Node version (M4c spec §5).
    pub default_node: Option<NodeSpec>,
}

/// Where an extension comes from. `bundled` is one PHP ships with (opcache), listed only
/// to switch it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtSource {
    Brew,
    Pecl,
    Path,
    Bundled,
}

impl ExtSource {
    /// The name `yard.toml` and the API use.
    pub fn name(self) -> &'static str {
        match self {
            ExtSource::Brew => "brew",
            ExtSource::Pecl => "pecl",
            ExtSource::Path => "path",
            ExtSource::Bundled => "bundled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtEntry {
    pub source: ExtSource,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// The absolute path of the `.so`; required for `source = "path"`.
    pub so: Option<PathBuf>,
}

fn enabled_default() -> bool {
    true
}

/// An extension name, which is also part of its ini file's name.
pub fn valid_ext_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// An extension as `yard ext add` and `.yard` name it: `redis`, or `xdebug-2.9.8`, a pinned
/// pecl release, which legacy PHP often needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtName {
    /// As given.
    pub written: String,
    /// The module PHP loads: `xdebug`.
    pub module: String,
    /// The pecl package to install when a release is pinned: `xdebug-2.9.8`.
    pub release: Option<String>,
}

/// A name in `yard ext add`'s forms; not a `.so` path.
pub fn parse_ext_name(name: &str) -> Result<ExtName, YardError> {
    let (module, release) = match name.split_once('-') {
        Some((m, version))
            if version.starts_with(|c: char| c.is_ascii_digit())
                && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') =>
        {
            (m, Some(name.to_string()))
        }
        _ => (name, None),
    };
    if !valid_ext_name(module) {
        return Err(YardError::new(
            yard_proto::ErrorCode::InvalidArgument,
            format!("{module:?} is not an extension name: use letters, digits and _"),
        )
        .with_hint("yard ext add --help"));
    }
    Ok(ExtName { written: name.to_string(), module: module.to_string(), release })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPhp {
    #[serde(default)]
    extensions: BTreeMap<toml::Spanned<String>, toml::Spanned<ExtEntry>>,
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
    default_node: Option<NodeSpec>,
    tld: Option<toml::Spanned<String>>,
    #[serde(default)]
    secure_by_default: bool,
    #[serde(default)]
    php: BTreeMap<toml::Spanned<String>, RawPhp>,
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
        None => Raw {
            parked: None,
            default_php: None,
            default_node: None,
            tld: None,
            secure_by_default: false,
            php: BTreeMap::new(),
        },
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
    let text = text.unwrap_or_default();
    let mut extensions = BTreeMap::new();
    for (key, section) in raw.php {
        let v: PhpVersion = key
            .get_ref()
            .parse()
            .map_err(|e: YardError| config_error(file, text, Some(key.span()), &e.message))?;
        if *key.get_ref() != v.to_string() {
            let message = format!("write PHP {v} as \"{v}\", not {:?}", key.get_ref());
            return Err(config_error(file, text, Some(key.span()), &message));
        }
        let mut exts = BTreeMap::new();
        for (name, entry) in section.extensions {
            if !valid_ext_name(name.get_ref()) {
                let message =
                    format!("extension name {:?} must be letters, digits and underscores", name.get_ref());
                return Err(config_error(file, text, Some(name.span()), &message));
            }
            let span = entry.span();
            let entry = entry.into_inner();
            let message = match (&entry.source, &entry.so) {
                (ExtSource::Path, None) => {
                    Some("source = \"path\" needs so = \"<absolute path to the .so>\"")
                }
                (_, Some(so)) if !so.is_absolute() => Some("so must be an absolute path"),
                (_, Some(so)) if so.to_string_lossy().contains(['"', '\n']) => {
                    Some("so cannot contain a double quote or a newline")
                }
                _ => None,
            };
            if let Some(message) = message {
                return Err(config_error(file, text, Some(span), &format!("{}: {message}", name.get_ref())));
            }
            exts.insert(name.into_inner(), entry);
        }
        extensions.insert(v, exts);
    }
    Ok((
        YardConfig {
            default_php: raw.default_php.unwrap_or(PhpVersion::DEFAULT),
            tld,
            secure_by_default: raw.secure_by_default,
            extensions,
            default_node: raw.default_node,
        },
        warnings,
    ))
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
    fn secure_by_default_is_parsed() {
        let (c, _) = parse_yard(Some("secure_by_default = true\n"), Path::new("/r/yard.toml")).unwrap();
        assert!(c.secure_by_default);
        assert!(!parse_yard(None, Path::new("/r/yard.toml")).unwrap().0.secure_by_default);
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
    fn default_node_parses_and_a_bad_one_is_config_invalid() {
        assert_eq!(
            parse(Some("default_node = \"lts/iron\"\n"))
                .unwrap()
                .default_node
                .map(|s| s.to_string())
                .as_deref(),
            Some("lts/iron")
        );
        assert_eq!(parse(Some("")).unwrap().default_node, None);
        let e = parse(Some("default_node = \"14\"\n")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        let data = e.data.unwrap();
        assert_eq!(data["file"], FILE);
        assert_eq!(data["line"], 1);
        assert!(e.message.contains("older than 16"), "{}", e.message);
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
    fn extensions_parse_and_validate() {
        let text = r#"
[php."8.4".extensions]
redis = { source = "brew" }
opcache = { source = "bundled", enabled = false }
custom = { source = "path", so = "/opt/ext/custom.so" }
"#;
        let c = parse(Some(text)).unwrap();
        let exts = &c.extensions[&"8.4".parse::<PhpVersion>().unwrap()];
        assert_eq!(exts["redis"], ExtEntry { source: ExtSource::Brew, enabled: true, so: None });
        assert_eq!(exts["opcache"], ExtEntry { source: ExtSource::Bundled, enabled: false, so: None });
        assert_eq!(exts["custom"].so.as_deref(), Some(Path::new("/opt/ext/custom.so")));
        assert!(parse(Some("")).unwrap().extensions.is_empty());

        for (bad, line) in [
            ("[php.\"8.4\".extensions]\ncustom = { source = \"path\" }\n", 2),
            ("[php.\"8.4\".extensions]\ncustom = { source = \"path\", so = \"ext/c.so\" }\n", 2),
            ("[php.\"8.4\".extensions]\nredis = { source = \"apt\" }\n", 2),
            ("[php.\"5.6\".extensions]\nredis = { source = \"brew\" }\n", 1),
            // Two spellings of one version would be two tables that edits cannot find.
            ("[php.\"08.4\".extensions]\nredis = { source = \"brew\" }\n", 1),
            ("[php.\"8.4\".extensions]\n\"../x\" = { source = \"brew\" }\n", 2),
        ] {
            let e = parse(Some(bad)).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfigInvalid, "{bad}");
            assert_eq!(e.data.unwrap()["line"], line, "{bad}: {}", e.message);
        }
    }

    #[test]
    fn parse_ext_name_forms() {
        let x = parse_ext_name("xdebug-2.9.8").unwrap();
        assert_eq!(
            (x.written.as_str(), x.module.as_str(), x.release.as_deref()),
            ("xdebug-2.9.8", "xdebug", Some("xdebug-2.9.8"))
        );
        let r = parse_ext_name("redis").unwrap();
        assert_eq!((r.module.as_str(), r.release), ("redis", None));
        // A suffix that is not a version stays part of the name, which is then invalid.
        for bad in ["../x", "re dis", "pdo-mysql", ""] {
            assert_eq!(parse_ext_name(bad).unwrap_err().code, ErrorCode::InvalidArgument, "{bad}");
        }
    }

    #[test]
    fn unsupported_php_is_config_invalid() {
        let e = parse(Some("default_php = \"5.6\"\n")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }
}
