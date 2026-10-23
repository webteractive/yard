//! The service engine table: which engines Yard runs, how their instances are named, which
//! brew formula serves a release line, and the `.env` lines that reach them. Pure: brew's
//! answers are passed in.

pub mod brew_info;
pub mod engine;
pub mod render;
pub mod reserved;

use brew_info::Formula;
use yard_proto::{ErrorCode, YardError};

pub use engine::{Engine, env_lines, instance_name, parse_instance};

/// A release line as numbers, so `9.7` sorts before `26.10`.
fn line_key(line: &str) -> Vec<u64> {
    line.split('.').filter_map(|part| part.parse().ok()).collect()
}

/// The line a formula's `version` is on: `8.4.13` is MySQL `8.4`, `18.1` is PostgreSQL `18`.
fn plain_line(engine: Engine, version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let line = match engine {
        Engine::Mysql => format!("{major}.{}", parts.next()?),
        Engine::Postgres => major.to_string(),
        Engine::Redis | Engine::Minio => return None,
    };
    engine.valid_line(&line).then_some(line)
}

/// The names in `names` (what `brew search` found) that can be a line of `engine`: the
/// plain formula and every `<base>@<line>` with a valid line. These are the ones whose
/// `brew info` `lines` and `resolve_line` read.
pub fn candidate_names(engine: Engine, names: &[String]) -> Vec<String> {
    if !engine.versioned() {
        return Vec::new();
    }
    let prefix = format!("{}@", engine.base_formula());
    let mut found: Vec<String> = names
        .iter()
        .filter(|n| {
            n.as_str() == engine.base_formula()
                || n.strip_prefix(&prefix).is_some_and(|line| engine.valid_line(line))
        })
        .cloned()
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The formula `resolve_line` would pick for `line` before it looks at the formula's
/// version: `<base>@<line>` when `names` has it, else the plain formula. The caller needs
/// brew's `info` for exactly this one; without it the line cannot be told apart from one
/// brew does not have.
pub fn line_formula(engine: Engine, line: &str, names: &[String]) -> Option<String> {
    let versioned = format!("{}@{line}", engine.base_formula());
    [versioned.as_str(), engine.base_formula()]
        .into_iter()
        .find(|name| engine.versioned() && names.iter().any(|n| n == name))
        .map(str::to_string)
}

/// Every line `resolve_line` accepts, as `(line, formula)`, sorted by version. A
/// `<base>@<line>` formula wins over the plain formula when both name the same line.
/// A formula brew has disabled is never offered.
fn candidates(
    engine: Engine,
    names: &[String],
    info: &dyn Fn(&str) -> Option<Formula>,
) -> Vec<(String, Formula)> {
    if !engine.versioned() {
        return Vec::new();
    }
    let prefix = format!("{}@", engine.base_formula());
    let mut found: Vec<(String, Formula)> = names
        .iter()
        .filter_map(|name| {
            let line = name.strip_prefix(&prefix)?;
            if !engine.valid_line(line) {
                return None;
            }
            Some((line.to_string(), info(name)?))
        })
        .filter(|(_, formula)| !formula.disabled)
        .collect();
    let plain = engine.base_formula();
    let plain_found = names
        .iter()
        .any(|n| n == plain)
        .then(|| info(plain))
        .flatten()
        .filter(|formula| !formula.disabled)
        .and_then(|formula| Some((plain_line(engine, &formula.version)?, formula)));
    // The plain formula goes last, so the stable sort and `dedup_by` keep the versioned
    // formula when both name the same line.
    found.extend(plain_found);
    found.sort_by_key(|(line, _)| line_key(line));
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

/// Every line of `engine` that brew offers, sorted by version (`8.0, 8.4, 9.7, 26.10`).
/// `names` are the formula names `brew search` found; `info` is `brew info` for one.
pub fn lines(engine: Engine, names: &[String], info: &dyn Fn(&str) -> Option<Formula>) -> Vec<String> {
    candidates(engine, names, info).into_iter().map(|(line, _)| line).collect()
}

/// The formula that serves `line`: `<base>@<line>` when brew has it, else the plain
/// formula when its version is on that line. Anything else is
/// `SERVICE_VERSION_UNSUPPORTED`, naming the lines brew does offer.
pub fn resolve_line(
    engine: Engine,
    line: &str,
    names: &[String],
    info: &dyn Fn(&str) -> Option<Formula>,
) -> Result<Formula, YardError> {
    let found = candidates(engine, names, info);
    if let Some((_, formula)) = found.iter().find(|(l, _)| l == line) {
        return Ok(formula.clone());
    }
    let key = engine.key();
    let name = engine.display();
    let (message, hint) = match found.first() {
        None if !engine.versioned() => (format!("{name} has no versions to choose between"), None),
        None => {
            (format!("brew offers no {name} release lines"), Some("brew update, then try again".to_string()))
        }
        Some((first, _)) => {
            let offered: Vec<&str> = found.iter().map(|(l, _)| l.as_str()).collect();
            (
                format!("{name} {line} is not available from brew; it offers {}", offered.join(", ")),
                Some(format!("yard service add {key} --version {first}")),
            )
        }
    };
    let mut error = YardError::new(ErrorCode::ServiceVersionUnsupported, message);
    if let Some(hint) = hint {
        error = error.with_hint(hint);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::brew_info::Formula;
    use super::*;
    use yard_proto::ErrorCode;

    fn formula(name: &str, version: &str) -> Formula {
        Formula {
            name: name.into(),
            full_name: name.into(),
            tap: "homebrew/core".into(),
            version: version.into(),
            deprecated: false,
            deprecation_date: None,
            disabled: false,
            disable_date: None,
            aliases: vec![],
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn resolve_prefers_versioned_formula() {
        let names = names(&["mysql", "mysql@8.0", "mysql@8.4", "mysql@9.7"]);
        let info = |n: &str| Some(formula(n, "8.4.13"));
        let f = resolve_line(Engine::Mysql, "8.4", &names, &info).unwrap();
        assert_eq!(f.name, "mysql@8.4");
    }

    #[test]
    fn resolve_falls_back_to_plain_formula() {
        let names = names(&["mysql", "mysql@8.0", "mysql@8.4", "mysql@9.7"]);
        let info = |n: &str| Some(formula(n, if n == "mysql" { "26.10.0" } else { "9.7.0" }));
        let f = resolve_line(Engine::Mysql, "26.10", &names, &info).unwrap();
        assert_eq!(f.name, "mysql");

        let err = resolve_line(Engine::Mysql, "26.11", &names, &info).unwrap_err();
        assert_eq!(err.code, ErrorCode::ServiceVersionUnsupported);
        assert!(err.message.contains("8.0, 8.4, 9.7, 26.10"), "{}", err.message);
        assert_eq!(err.hint.as_deref(), Some("yard service add mysql --version 8.0"));
    }

    #[test]
    fn lines_are_sorted_by_version_and_skip_a_plain_duplicate() {
        let names = names(&["postgresql", "postgresql@17", "postgresql@9", "postgresql@18", "postgresql@14"]);
        let info = |n: &str| Some(formula(n, if n == "postgresql" { "18.1" } else { "1" }));
        assert_eq!(lines(Engine::Postgres, &names, &info), ["9", "14", "17", "18"]);
    }

    #[test]
    fn a_plain_formula_is_not_a_line_for_an_unversioned_engine() {
        let names = names(&["redis"]);
        let info = |n: &str| Some(formula(n, "8.0.1"));
        assert!(lines(Engine::Redis, &names, &info).is_empty());
        let err = resolve_line(Engine::Redis, "8.0", &names, &info).unwrap_err();
        assert_eq!(err.code, ErrorCode::ServiceVersionUnsupported);
    }

    #[test]
    fn no_formulae_at_all_is_unsupported_with_a_hint() {
        let info = |_: &str| None;
        let err = resolve_line(Engine::Mysql, "8.4", &[], &info).unwrap_err();
        assert_eq!(err.code, ErrorCode::ServiceVersionUnsupported);
        assert!(err.hint.is_some());
    }

    #[test]
    fn a_disabled_formula_is_not_a_line() {
        let names = names(&["postgresql@13", "postgresql@17", "mysql", "mysql@8.4"]);
        let info = |n: &str| {
            let mut f = formula(n, if n == "mysql" { "9.9.1" } else { "1" });
            f.disabled = n == "postgresql@13" || n == "mysql";
            f.disable_date = f.disabled.then(|| "2026-01-01".to_string());
            Some(f)
        };
        assert_eq!(lines(Engine::Postgres, &names, &info), ["17"]);
        let err = resolve_line(Engine::Postgres, "13", &names, &info).unwrap_err();
        assert_eq!(err.code, ErrorCode::ServiceVersionUnsupported);
        assert!(err.message.contains("it offers 17"), "{}", err.message);
        // A disabled plain formula is no line either, whatever version it is on.
        assert_eq!(lines(Engine::Mysql, &names, &info), ["8.4"]);
        assert!(resolve_line(Engine::Mysql, "9.9", &names, &info).is_err());
    }

    #[test]
    fn a_deprecated_formula_is_still_a_line() {
        let names = names(&["mysql@8.0"]);
        let info = |n: &str| {
            let mut f = formula(n, "8.0.46");
            f.deprecated = true;
            f.disable_date = Some("2027-04-30".into());
            Some(f)
        };
        assert_eq!(lines(Engine::Mysql, &names, &info), ["8.0"]);
    }

    #[test]
    fn candidate_names_are_the_plain_formula_and_valid_lines() {
        let found = names(&["mysql", "mysql@8.4", "mysql@8.0", "mysql@x", "mysql-client", "mysql@8.4"]);
        assert_eq!(candidate_names(Engine::Mysql, &found), ["mysql", "mysql@8.0", "mysql@8.4"]);
        assert!(candidate_names(Engine::Redis, &names(&["redis"])).is_empty());
    }

    #[test]
    fn the_line_formula_is_the_versioned_one_when_brew_lists_it() {
        let found = names(&["mysql", "mysql@8.4"]);
        assert_eq!(line_formula(Engine::Mysql, "8.4", &found).as_deref(), Some("mysql@8.4"));
        assert_eq!(line_formula(Engine::Mysql, "26.10", &found).as_deref(), Some("mysql"));
        assert_eq!(line_formula(Engine::Mysql, "8.4", &names(&["mysql@8.0"])), None);
        assert_eq!(line_formula(Engine::Redis, "8.4", &names(&["redis"])), None);
    }

    #[test]
    fn postgres_lines_are_majors() {
        let names = names(&["postgresql@16", "postgresql@17"]);
        let info = |n: &str| Some(formula(n, "17.2"));
        let f = resolve_line(Engine::Postgres, "17", &names, &info).unwrap();
        assert_eq!(f.name, "postgresql@17");
        assert!(!Engine::Postgres.valid_line("17.2"));
        assert!(resolve_line(Engine::Postgres, "17.2", &names, &info).is_err());
    }
}
