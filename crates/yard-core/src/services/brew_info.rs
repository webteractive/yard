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
    /// brew already refuses to install it (`disabled`): it is not offered as a line.
    pub disabled: bool,
    /// When brew disables it: its own `disable_date`, else a year after `deprecation_date`, as
    /// brew works it out for `brew info` ("It will be disabled on …").
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
    disabled: bool,
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
            disable_date: raw.disable_date.or_else(|| raw.deprecation_date.as_deref().and_then(a_year_after)),
            deprecation_date: raw.deprecation_date,
            disabled: raw.disabled,
            aliases: raw.aliases,
        })
        .collect())
}

/// `YYYY-MM-DD` twelve months on, as Ruby's `Date >> 12` (brew's
/// `DeprecateDisable::REMOVE_DISABLED_TIME_WINDOW`): 29 February becomes 28 February.
fn a_year_after(date: &str) -> Option<String> {
    let mut parts = date.split('-').map(|p| p.parse::<u32>().ok());
    let (year, month, day) = match (parts.next()??, parts.next()??, parts.next()??, parts.next()) {
        (y, m @ 1..=12, d @ 1..=31, None) => (y, m, d),
        _ => return None,
    };
    let leap = |y: u32| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let day = if month == 2 && day == 29 && !leap(year + 1) { 28 } else { day };
    Some(format!("{:04}-{month:02}-{day:02}", year + 1))
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
        assert!(!f.disabled);
        // brew prints "It will be disabled on 2027-04-30.": a year after the deprecation.
        assert_eq!(f.disable_date.as_deref(), Some("2027-04-30"));
        assert!(f.aliases.is_empty());
    }

    #[test]
    fn disable_date_is_a_year_after_deprecation_when_brew_sets_none() {
        let one = |deprecated: &str, disable: &str| {
            let json = format!(
                r#"{{"formulae":[{{"name":"x","deprecated":true,"deprecation_date":{deprecated},"disable_date":{disable}}}]}}"#
            );
            parse(&json).unwrap()[0].disable_date.clone()
        };
        assert_eq!(one(r#""2026-04-30""#, "null").as_deref(), Some("2027-04-30"));
        // Ruby's `Date >> 12` keeps the last day of February.
        assert_eq!(one(r#""2028-02-29""#, "null").as_deref(), Some("2029-02-28"));
        // brew's own date wins, and no deprecation date means none.
        assert_eq!(one(r#""2026-04-30""#, r#""2026-12-01""#).as_deref(), Some("2026-12-01"));
        assert_eq!(one("null", "null"), None);
        assert_eq!(one(r#""soon""#, "null"), None);
    }

    #[test]
    fn brew_info_reads_disabled() {
        let f =
            parse(r#"{"formulae":[{"name":"postgresql@13","disabled":true,"disable_date":"2026-11-13"}]}"#)
                .unwrap();
        assert!(f[0].disabled);
        assert_eq!(f[0].disable_date.as_deref(), Some("2026-11-13"));
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
