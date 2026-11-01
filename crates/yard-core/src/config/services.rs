//! `services.toml`: the service instances Yard runs, one `[services."<instance>"]` table
//! each (M4b spec §4). Edited only through `TomlFile`.

use super::{config_error, read_optional};
use crate::Paths;
use crate::services::sleep::SleepAfter;
use crate::services::{Engine, parse_instance};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use toml::Spanned;
use yard_proto::YardError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServicesConfig {
    pub services: BTreeMap<String, ServiceEntry>,
    /// `[emailulator]`: written by `yard service add emailulator`, which Yard does not run
    /// (M4d, option B).
    pub emailulator: Option<EmailulatorEntry>,
}

/// `[emailulator]` in `services.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmailulatorEntry {
    /// The binary the CLI found on the user's PATH; yardd, started by launchd, has another.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    pub engine: Engine,
    pub version: Option<String>,
    pub formula: String,
    pub stopped: bool,
    /// Overrides `yard.toml`'s `service_sleep_after` (M4e spec §4).
    pub sleep_after: Option<SleepAfter>,
}

pub fn load_services(paths: &Paths) -> Result<ServicesConfig, YardError> {
    let file = paths.services_toml();
    parse_services(read_optional(&file)?.as_deref(), &file)
}

/// One `[services."<instance>"]` table as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    engine: Spanned<Engine>,
    version: Option<Spanned<String>>,
    formula: String,
    #[serde(default)]
    stopped: bool,
    sleep_after: Option<SleepAfter>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    services: BTreeMap<Spanned<String>, RawEntry>,
    emailulator: Option<RawEmailulator>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEmailulator {
    path: Option<Spanned<PathBuf>>,
}

/// `CONFIG_INVALID` for an unknown key, a missing `engine` or `formula`, an instance name
/// `parse_instance` rejects, or an `engine` / `version` that disagree with the name.
pub fn parse_services(text: Option<&str>, file: &Path) -> Result<ServicesConfig, YardError> {
    let Some(text) = text else {
        return Ok(ServicesConfig::default());
    };
    let raw: Raw = toml::from_str(text).map_err(|e| config_error(file, text, e.span(), e.message()))?;
    let invalid = |span: Range<usize>, message: String| config_error(file, text, Some(span), &message);
    let mut services = BTreeMap::new();
    for (name, entry) in raw.services {
        let instance = name.get_ref();
        let Some((engine, line)) = parse_instance(instance) else {
            return Err(invalid(
                name.span(),
                format!(
                    "{instance:?} is not a service instance; names are mysql-<line>, postgres-<line>, redis and minio"
                ),
            ));
        };
        if *entry.engine.get_ref() != engine {
            return Err(invalid(
                entry.engine.span(),
                format!(
                    "{instance} is {} but engine is {:?}; the name and the engine must agree",
                    engine.key(),
                    entry.engine.get_ref().key()
                ),
            ));
        }
        let version = entry.version.as_ref().map(|v| v.get_ref().as_str());
        if version != line.as_deref() {
            let span = entry.version.as_ref().map_or_else(|| entry.engine.span(), |v| v.span());
            let said = |v: Option<&str>| v.map_or("no version".to_string(), |v| format!("version {v:?}"));
            return Err(invalid(
                span,
                format!(
                    "{instance} needs {} but has {}; the name and the version must agree",
                    said(line.as_deref()),
                    said(version)
                ),
            ));
        }
        services.insert(
            instance.clone(),
            ServiceEntry {
                engine,
                version: line,
                formula: entry.formula,
                stopped: entry.stopped,
                sleep_after: entry.sleep_after,
            },
        );
    }
    let emailulator = match raw.emailulator {
        Some(RawEmailulator { path: Some(p) }) if !p.get_ref().is_absolute() => {
            return Err(invalid(p.span(), format!("emailulator's path {:?} must be absolute", p.get_ref())));
        }
        Some(e) => Some(EmailulatorEntry { path: e.path.map(Spanned::into_inner) }),
        None => None,
    };
    Ok(ServicesConfig { services, emailulator })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use yard_proto::ErrorCode;

    const FILE: &str = "/r/services.toml";

    fn parse(text: &str) -> Result<ServicesConfig, YardError> {
        parse_services(Some(text), Path::new(FILE))
    }

    #[test]
    fn emailulator_table_records_the_path() {
        let file = Path::new("/r/services.toml");
        let c = parse_services(Some("[emailulator]\npath = \"/opt/bin/emailulator\"\n"), file).unwrap();
        assert_eq!(
            c.emailulator,
            Some(EmailulatorEntry { path: Some(PathBuf::from("/opt/bin/emailulator")) })
        );
        assert_eq!(
            parse_services(Some("[emailulator]\n"), file).unwrap().emailulator,
            Some(EmailulatorEntry::default())
        );
        assert_eq!(parse_services(Some(""), file).unwrap().emailulator, None);
        let e = parse_services(Some("[emailulator]\npath = \"emailulator\"\n"), file).unwrap_err();
        assert!(e.message.contains("must be absolute"), "{}", e.message);
        assert!(parse_services(Some("[emailulator]\nversion = \"0.1\"\n"), file).is_err());
    }

    #[test]
    fn parses_services_toml() {
        let c = parse(
            "# services.toml\n[services.\"mysql-8.4\"]\nengine = \"mysql\"\nversion = \"8.4\"\nformula = \"mysql@8.4\"\n\n[services.\"postgres-17\"]\nengine = \"postgres\"\nversion = \"17\"\nformula = \"postgresql@17\"\nstopped = true\n",
        )
        .unwrap();
        assert_eq!(c.services.len(), 2);
        let my = &c.services["mysql-8.4"];
        assert_eq!(
            (my.engine, my.version.as_deref(), my.formula.as_str(), my.stopped),
            (Engine::Mysql, Some("8.4"), "mysql@8.4", false)
        );
        let pg = &c.services["postgres-17"];
        assert_eq!((pg.engine, pg.version.as_deref(), pg.stopped), (Engine::Postgres, Some("17"), true));
    }

    #[test]
    fn unversioned_engines_have_no_version() {
        let c = parse("[services.redis]\nengine = \"redis\"\nformula = \"redis\"\n").unwrap();
        assert_eq!(c.services["redis"].version, None);
        assert!(!c.services["redis"].stopped);
        assert!(parse("[services.minio]\nengine = \"minio\"\nformula = \"minio/stable/minio\"\n").is_ok());
    }

    #[test]
    fn sleep_after_on_an_instance() {
        use crate::services::sleep::SleepAfter;
        let c = parse("[services.redis]\nengine = \"redis\"\nformula = \"redis\"\nsleep_after = \"off\"\n")
            .unwrap();
        assert_eq!(c.services["redis"].sleep_after, Some(SleepAfter::Off));
        let c = parse("[services.redis]\nengine = \"redis\"\nformula = \"redis\"\n").unwrap();
        assert_eq!(c.services["redis"].sleep_after, None);
        let e = parse("[services.redis]\nengine = \"redis\"\nformula = \"redis\"\nsleep_after = \"fast\"\n")
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert_eq!(e.data.as_ref().unwrap()["line"], 4);
    }

    #[test]
    fn missing_file_and_empty_file_are_empty() {
        assert_eq!(parse_services(None, Path::new(FILE)).unwrap(), ServicesConfig::default());
        assert_eq!(parse("").unwrap(), ServicesConfig::default());
    }

    #[test]
    fn unknown_service_key_names_line() {
        let text = "[services.\"mysql-8.4\"]\nengine = \"mysql\"\nversion = \"8.4\"\nformula = \"mysql@8.4\"\nport = 3306\n";
        let e = parse(text).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert_eq!(e.data.as_ref().unwrap()["line"], 5);
        assert!(e.message.contains("port") && e.message.contains(FILE), "{}", e.message);
    }

    #[test]
    fn instance_name_must_match_engine() {
        let text = "[services.\"mysql-8.4\"]\nengine = \"postgres\"\nversion = \"8.4\"\nformula = \"x\"\n";
        let e = parse(text).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.message.contains("mysql-8.4"), "{}", e.message);
        assert_eq!(e.data.as_ref().unwrap()["line"], 2);
    }

    #[test]
    fn instance_name_must_match_version() {
        let text =
            "[services.\"mysql-8.4\"]\nengine = \"mysql\"\nversion = \"8.0\"\nformula = \"mysql@8.0\"\n";
        let e = parse(text).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert_eq!(e.data.as_ref().unwrap()["line"], 3);
        let text = "[services.\"mysql-8.4\"]\nengine = \"mysql\"\nformula = \"mysql@8.4\"\n";
        assert_eq!(parse(text).unwrap_err().code, ErrorCode::ConfigInvalid);
        let text = "[services.redis]\nengine = \"redis\"\nversion = \"8\"\nformula = \"redis\"\n";
        assert_eq!(parse(text).unwrap_err().code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn a_name_that_is_not_an_instance_is_invalid() {
        for name in ["mysql", "mysql-8", "redis-8", "mongo-1", "mysql-latest"] {
            let text = format!("[services.\"{name}\"]\nengine = \"mysql\"\nformula = \"x\"\n");
            let e = parse(&text).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfigInvalid, "{name}");
            assert_eq!(e.data.as_ref().unwrap()["line"], 1, "{name}");
        }
    }

    #[test]
    fn unknown_engine_and_missing_formula_are_invalid() {
        let e = parse("[services.mongo]\nengine = \"mongo\"\nformula = \"mongodb\"\n").unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert_eq!(e.data.as_ref().unwrap()["line"], 2);
        assert_eq!(
            parse("[services.redis]\nengine = \"redis\"\n").unwrap_err().code,
            ErrorCode::ConfigInvalid
        );
    }

    #[test]
    fn unknown_top_level_key_is_invalid() {
        assert_eq!(parse("tld = \"x\"\n").unwrap_err().code, ErrorCode::ConfigInvalid);
    }
}
