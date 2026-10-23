//! What brew trusts (spec M4b §8). Yard keeps no trust list of its own: it reads
//! `brew trust --json=v1` and shows what brew says. Pure parsing and assembly; running
//! brew lives in `yard-daemon`.

use serde::Deserialize;
use std::collections::BTreeSet;
use yard_proto::{SourceFormula, SourceInfo};

/// The taps Yard installs from, in the order `yard sources` lists them.
pub const YARD_TAPS: [&str; 3] = ["shivammathur/php", "shivammathur/extensions", "minio/stable"];

/// The first Homebrew major with `brew trust` and a trust requirement for third-party taps.
pub const TRUST_SINCE: u32 = 6;

/// `brew trust --json=v1`: the taps and formulae the user trusted. Casks and commands are
/// not something Yard installs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustStore {
    pub taps: BTreeSet<String>,
    pub formulae: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    taps: BTreeSet<String>,
    #[serde(default)]
    formulae: BTreeSet<String>,
}

/// Parse `brew trust --json=v1`'s output.
pub fn parse_trust(json: &str) -> Result<TrustStore, String> {
    let doc: Document = serde_json::from_str(json).map_err(|e| format!("brew trust --json=v1: {e}"))?;
    Ok(TrustStore { taps: doc.taps, formulae: doc.formulae })
}

/// The tap of a fully qualified formula (`shivammathur/php/php@8.3` is `shivammathur/php`).
/// `None` for a bare name and for an official tap's formula (`homebrew/core/redis`), which
/// brew always trusts.
pub fn tap_of(formula: &str) -> Option<&str> {
    let mut parts = formula.splitn(3, '/');
    let (owner, repo, name) = (parts.next()?, parts.next()?, parts.next()?);
    if name.is_empty() || owner.eq_ignore_ascii_case("homebrew") {
        return None;
    }
    Some(&formula[..owner.len() + 1 + repo.len()])
}

impl TrustStore {
    /// Whether brew would load `formula`: it is official, or it or its tap is trusted.
    pub fn trusts(&self, formula: &str) -> bool {
        match tap_of(formula) {
            None => true,
            Some(tap) => self.taps.contains(tap) || self.formulae.contains(formula),
        }
    }
}

/// The major of `brew --version`'s output (`Homebrew 7.0.10` is 7), from its first line
/// that starts `Homebrew `.
pub fn brew_major(version_output: &str) -> Option<u32> {
    let rest = version_output.lines().find_map(|l| l.trim().strip_prefix("Homebrew "))?;
    rest.split(['.', '-']).next()?.trim().parse().ok()
}

/// Where a tap lives under `<prefix>/Library/Taps`: `owner/homebrew-repo` for the
/// formula's `owner/repo/name`. `None` for a bare name or one in `homebrew/*`.
pub fn tap_dir(formula: &str) -> Option<String> {
    let (owner, repo) = tap_of(formula)?.split_once('/')?;
    Some(format!("{owner}/homebrew-{repo}"))
}

/// The bare name of `formula`: `redis@8.4` for `shivammathur/extensions/redis@8.4`.
pub fn short_name(formula: &str) -> &str {
    formula.rsplit('/').next().unwrap_or(formula)
}

/// Where, relative to the tap's directory, a tap that lists `name` has a file for it: the
/// formula in `Formula/` or at the tap's root, or an alias. (A formula in a sharded
/// `Formula/<x>/` directory is the caller's to look for.)
pub fn listing_paths(name: &str) -> [String; 3] {
    [format!("Formula/{name}.rb"), format!("{name}.rb"), format!("Aliases/{name}")]
}

/// Whether the tap's `formula_renames.json` renames `name`, which brew then resolves to
/// the new formula as it does a formula of that name. Text that is not a JSON object
/// counts as renaming it: a tap Yard cannot read is not one to let past the gate.
pub fn renames_name(renames_json: &str, name: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(renames_json) {
        Ok(serde_json::Value::Object(map)) => map.contains_key(name),
        _ => true,
    }
}

/// The rows of `sources.list`: each Yard tap, with whether it is tapped and trusted as a
/// whole, and the formulae in it that are trusted one by one or installed, sorted by name.
/// `trust` is `None` for a brew that predates trust, which trusts everything.
pub fn rows(
    trust: Option<&TrustStore>,
    tapped: &dyn Fn(&str) -> bool,
    installed: &BTreeSet<String>,
) -> Vec<SourceInfo> {
    YARD_TAPS
        .iter()
        .map(|&tap| {
            let mut names: BTreeSet<&String> = installed.iter().filter(|f| tap_of(f) == Some(tap)).collect();
            if let Some(trust) = trust {
                names.extend(trust.formulae.iter().filter(|f| tap_of(f) == Some(tap)));
            }
            SourceInfo {
                tap: tap.to_string(),
                tapped: tapped(tap),
                trusted: trust.is_none_or(|t| t.taps.contains(tap)),
                formulae: names
                    .into_iter()
                    .map(|name| SourceFormula {
                        name: name.clone(),
                        trusted: trust.is_none_or(|t| t.trusts(name)),
                        installed: installed.contains(name),
                    })
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(taps: &[&str], formulae: &[&str]) -> TrustStore {
        parse_trust(
            &serde_json::json!({"taps": taps, "formulae": formulae, "casks": [], "commands": []}).to_string(),
        )
        .unwrap()
    }

    #[test]
    fn trust_store_rules() {
        let empty = store(&[], &[]);
        // brew always trusts its own: a bare name and homebrew/core need no entry.
        assert!(empty.trusts("mysql@8.4"));
        assert!(empty.trusts("homebrew/core/redis"));
        assert!(!empty.trusts("shivammathur/php/php@8.3"));
        let by_formula = store(&[], &["shivammathur/php/php@8.3"]);
        assert!(by_formula.trusts("shivammathur/php/php@8.3"));
        assert!(!by_formula.trusts("shivammathur/php/php@8.2"));
        let by_tap = store(&["shivammathur/php"], &[]);
        assert!(by_tap.trusts("shivammathur/php/php@8.3"));
        assert!(!by_tap.trusts("shivammathur/extensions/redis@8.3"));
        assert_eq!(tap_of("shivammathur/php/php@8.3"), Some("shivammathur/php"));
        assert_eq!(tap_of("minio/stable/minio"), Some("minio/stable"));
        assert_eq!((tap_of("mysql@8.4"), tap_of("homebrew/core/redis")), (None, None));
    }

    #[test]
    fn tap_dir_and_short_name() {
        let formula = "shivammathur/extensions/redis@8.4";
        assert_eq!(tap_dir(formula).as_deref(), Some("shivammathur/homebrew-extensions"));
        assert_eq!(short_name(formula), "redis@8.4");
        assert_eq!((tap_dir("mysql@8.4"), tap_dir("homebrew/core/redis")), (None, None));
        assert_eq!(short_name("mysql@8.4"), "mysql@8.4");
    }

    #[test]
    fn listing_paths_name_the_formula_its_root_copy_and_its_alias() {
        assert_eq!(listing_paths("redis@8.4"), ["Formula/redis@8.4.rb", "redis@8.4.rb", "Aliases/redis@8.4"]);
    }

    #[test]
    fn renames_name_reads_the_keys_and_fails_closed() {
        let json = r#"{"redis@8.4": "phpredis@8.4", "apcu@8.4": "phpapcu@8.4"}"#;
        assert!(renames_name(json, "redis@8.4"));
        assert!(!renames_name(json, "phpredis@8.4"), "a value is a formula, not a rename");
        assert!(!renames_name(json, "mongodb@8.4"));
        assert!(!renames_name("{}", "redis@8.4"));
        for unreadable in ["", "{not json", "[]", "\"redis@8.4\""] {
            assert!(renames_name(unreadable, "mongodb@8.4"), "{unreadable:?}");
        }
    }

    #[test]
    fn parse_trust_reads_brews_document() {
        let json = r#"{"taps":["a/b"],"formulae":["a/b/c"],"casks":[],"commands":[]}"#;
        let s = parse_trust(json).unwrap();
        assert!(s.taps.contains("a/b") && s.formulae.contains("a/b/c"));
        // A key brew leaves out is empty; garbage is an error.
        assert!(parse_trust(r#"{"taps":["a/b"]}"#).unwrap().formulae.is_empty());
        assert!(parse_trust("Error: nope").is_err());
    }

    #[test]
    fn brew_major_parses() {
        assert_eq!(brew_major("Homebrew 7.0.10"), Some(7));
        assert_eq!(brew_major("Homebrew 5.1.0\nHomebrew/homebrew-core (git revision abc)"), Some(5));
        assert_eq!(brew_major("Homebrew 6.0.0-12-gabc"), Some(6));
        assert_eq!(brew_major("==> fake\nHomebrew 7.0.10\n"), Some(7));
        assert_eq!(brew_major("not brew"), None);
    }

    #[test]
    fn rows_list_the_yard_taps_with_their_formulae() {
        let trust =
            store(&["shivammathur/php"], &["minio/stable/minio", "shivammathur/extensions/redis@8.4"]);
        let installed: BTreeSet<String> =
            ["shivammathur/php/php@7.1", "shivammathur/extensions/xdebug@8.4"].map(String::from).into();
        let tapped = |tap: &str| tap != "minio/stable";
        let rows = rows(Some(&trust), &tapped, &installed);
        let taps: Vec<&str> = rows.iter().map(|r| r.tap.as_str()).collect();
        assert_eq!(taps, ["shivammathur/php", "shivammathur/extensions", "minio/stable"]);
        assert_eq!((rows[0].tapped, rows[0].trusted), (true, true));
        assert_eq!(rows[0].formulae.len(), 1);
        assert_eq!(rows[0].formulae[0].name, "shivammathur/php/php@7.1");
        assert!(rows[0].formulae[0].trusted && rows[0].formulae[0].installed);
        // Not trusted as a tap, so each formula carries its own state, sorted by name.
        assert_eq!((rows[1].tapped, rows[1].trusted), (true, false));
        let names: Vec<(&str, bool, bool)> =
            rows[1].formulae.iter().map(|f| (f.name.as_str(), f.trusted, f.installed)).collect();
        assert_eq!(
            names,
            [
                ("shivammathur/extensions/redis@8.4", true, false),
                ("shivammathur/extensions/xdebug@8.4", false, true)
            ]
        );
        assert_eq!((rows[2].tapped, rows[2].trusted), (false, false));
        assert_eq!(rows[2].formulae.len(), 1);
        // A brew with no trust store trusts everything.
        let old = super::rows(None, &tapped, &installed);
        assert!(old.iter().all(|r| r.trusted && r.formulae.iter().all(|f| f.trusted)));
    }
}
