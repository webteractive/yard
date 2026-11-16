//! Engine keys and release lines: the facts every crate checks without loading recipes.

use serde::{Deserialize, Serialize};
use std::fmt;

/// An engine's key: `^[a-z][a-z0-9-]{0,31}$`. Serializes as the string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EngineKey(String);

/// The keys of the engines in Yard's code, emailulator's, which Yard only points at, and the names
/// of Yard's own units and port entries, which an instance named for a key would take over.
const RESERVED: [&str; 7] = ["mysql", "postgres", "minio", "emailulator", "nginx", "dnsmasq", "mail"];

/// Prefixes of Yard's own unit and port-entry names (`tunnel-<site>`, `nginx-tunnel-<site>`,
/// `php-fpm-<v>`, `emailulator-smtp`), and of the versioned built-ins' instances.
const RESERVED_PREFIXES: [&str; 6] = ["mysql-", "postgres-", "tunnel-", "nginx-", "php-fpm", "emailulator-"];

impl EngineKey {
    /// Checks the syntax only; whether an engine has the key is the set's to say.
    pub fn new(key: &str) -> Result<EngineKey, String> {
        let mut bytes = key.bytes();
        let ok = key.len() <= 32
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(EngineKey(key.to_string()))
        } else {
            Err(format!(
                "{key:?} is not an engine key: a lowercase letter, then lowercase letters, digits and -, \
                 at most 32 characters"
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A built-in's key, emailulator's, or one a versioned instance name could be read as
    /// (`mysql-x`): a recipe may not take it.
    pub fn is_reserved(&self) -> bool {
        RESERVED.contains(&self.0.as_str()) || RESERVED_PREFIXES.iter().any(|p| self.0.starts_with(p))
    }
}

impl TryFrom<String> for EngineKey {
    type Error = String;
    fn try_from(key: String) -> Result<Self, String> {
        EngineKey::new(&key)
    }
}

impl From<EngineKey> for String {
    fn from(key: EngineKey) -> String {
        key.0
    }
}

impl fmt::Display for EngineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a release line is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineShape {
    /// `8.4`.
    MajorMinor,
    /// `17`.
    Major,
}

/// An engine brew ships several release lines of: `<base>@<line>` formulas, and the plain
/// formula's own line.
#[derive(Debug, PartialEq, Eq)]
pub struct LineSpec {
    pub base_formula: &'static str,
    pub default_line: &'static str,
    pub shape: LineShape,
}

pub static MYSQL_LINES: LineSpec =
    LineSpec { base_formula: "mysql", default_line: "8.4", shape: LineShape::MajorMinor };
pub static POSTGRES_LINES: LineSpec =
    LineSpec { base_formula: "postgresql", default_line: "17", shape: LineShape::Major };

impl LineSpec {
    pub fn valid(&self, line: &str) -> bool {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        match self.shape {
            LineShape::MajorMinor => matches!(line.split_once('.'), Some((a, b)) if digits(a) && digits(b)),
            LineShape::Major => digits(line),
        }
    }

    /// The line a formula's `version` is on: `8.4.13` is `8.4`, `18.1` is `18`.
    pub fn plain_line(&self, version: &str) -> Option<String> {
        let mut parts = version.split('.');
        let major = parts.next()?;
        let line = match self.shape {
            LineShape::MajorMinor => format!("{major}.{}", parts.next()?),
            LineShape::Major => major.to_string(),
        };
        self.valid(&line).then_some(line)
    }
}

/// The lines of the engine with `key`, when it is one of the two versioned built-ins. Fixed,
/// so `.yard` and `services.toml` are checked with no engine set (recipes are unversioned).
pub fn versioned_lines(key: &str) -> Option<&'static LineSpec> {
    match key {
        "mysql" => Some(&MYSQL_LINES),
        "postgres" => Some(&POSTGRES_LINES),
        _ => None,
    }
}

/// The instance name: `mysql-8.4`, `postgres-17`, `redis`, `memcached`. A versioned engine with
/// no line gets its default line; any other engine's instance is named for its key.
pub fn instance_name(key: &str, line: Option<&str>) -> String {
    match versioned_lines(key) {
        Some(spec) => format!("{key}-{}", line.unwrap_or(spec.default_line)),
        None => key.to_string(),
    }
}

/// The inverse of [`instance_name`], with no engine set: `mysql-8.4` is MySQL's 8.4 line, and
/// any other valid engine key is that engine's one instance. `None` for `mysql` alone,
/// `mysql-8` or a name that is not a key.
pub fn parse_instance(name: &str) -> Option<(EngineKey, Option<String>)> {
    for key in ["mysql", "postgres"] {
        if name == key {
            return None;
        }
        if let Some(line) = name.strip_prefix(key).and_then(|rest| rest.strip_prefix('-')) {
            let spec = versioned_lines(key)?;
            return spec.valid(line).then(|| (EngineKey(key.to_string()), Some(line.to_string())));
        }
    }
    EngineKey::new(name).ok().map(|k| (k, None))
}
