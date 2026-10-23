//! `brew info --json=v2` for a service formula: only the fields Yard reads.

use serde::Deserialize;

/// One formula from `brew info --json=v2`'s `formulae`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formula {
    pub name: String,
    /// `minio/stable/minio` for a tap formula, the plain name for homebrew/core.
    pub full_name: String,
    pub tap: String,
    /// The stable version brew would install (`8.4.13`); empty when brew gives none.
    pub version: String,
    pub deprecated: bool,
    pub deprecation_date: Option<String>,
    pub disable_date: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    formulae: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    name: String,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    tap: String,
    #[serde(default)]
    versions: Versions,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    deprecation_date: Option<String>,
    #[serde(default)]
    disable_date: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Deserialize, Default)]
struct Versions {
    #[serde(default)]
    stable: Option<String>,
}

/// Parse `brew info --json=v2`'s output into its formulae (casks are ignored).
pub fn parse(json: &str) -> Result<Vec<Formula>, String> {
    let doc: Document = serde_json::from_str(json).map_err(|e| format!("brew info: {e}"))?;
    Ok(doc
        .formulae
        .into_iter()
        .map(|raw| Formula {
            full_name: raw.full_name.unwrap_or_else(|| raw.name.clone()),
            name: raw.name,
            tap: raw.tap,
            version: raw.versions.stable.unwrap_or_default(),
            deprecated: raw.deprecated,
            deprecation_date: raw.deprecation_date,
            disable_date: raw.disable_date,
            aliases: raw.aliases,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `brew info --json=v2 mysql@8.0` on 2026-10-24 (Homebrew 6), trimmed to the fields
    /// Yard reads plus the shape around them.
    const MYSQL_80: &str = r#"{
  "formulae": [
    {
      "name": "mysql@8.0",
      "full_name": "mysql@8.0",
      "tap": "homebrew/core",
      "oldnames": [],
      "aliases": [],
      "desc": "Open source relational database management system",
      "homepage": "https://dev.mysql.com/doc/refman/8.0/en/",
      "versions": { "stable": "8.0.46", "head": null, "bottle": true },
      "keg_only": true,
      "deprecated": true,
      "deprecation_date": "2026-04-30",
      "deprecation_reason": "unsupported",
      "disabled": false,
      "disable_date": null
    }
  ],
  "casks": []
}"#;

    #[test]
    fn brew_info_parses_real_shape() {
        let formulae = parse(MYSQL_80).unwrap();
        assert_eq!(formulae.len(), 1);
        let f = &formulae[0];
        assert_eq!(f.name, "mysql@8.0");
        assert_eq!(f.full_name, "mysql@8.0");
        assert_eq!(f.tap, "homebrew/core");
        assert_eq!(f.version, "8.0.46");
        assert!(f.deprecated);
        assert_eq!(f.deprecation_date.as_deref(), Some("2026-04-30"));
        assert_eq!(f.disable_date, None);
        assert!(f.aliases.is_empty());
    }

    #[test]
    fn brew_info_tolerates_missing_fields_and_rejects_garbage() {
        let f = parse(r#"{"formulae":[{"name":"redis","versions":{"stable":"8.0.1"}}]}"#).unwrap();
        assert_eq!(f[0].full_name, "redis");
        assert_eq!(f[0].version, "8.0.1");
        assert!(!f[0].deprecated);
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"casks":[]}"#).unwrap().is_empty());
    }
}
