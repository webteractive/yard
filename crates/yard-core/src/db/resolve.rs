//! Which instance a database verb (`yard mysql list`) means (db spec §5).

use crate::services::ServiceEngine;
use yard_proto::{ErrorCode, YardError};

/// An instance in `services.toml`, with what its engine supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub engine: String,
    /// `MySQL`, a recipe's name.
    pub display: String,
    pub databases: bool,
    pub connect: bool,
    /// Whether it has a prompt: a database's client, or a recipe's `connect.shell`.
    pub shell: bool,
}

impl Candidate {
    /// The instance `name` of `engine`.
    pub fn of(name: &str, engine: &dyn ServiceEngine) -> Candidate {
        Candidate {
            name: name.to_string(),
            engine: engine.key().to_string(),
            display: engine.display().to_string(),
            databases: engine.databases().is_some(),
            connect: engine.has_connect(),
            shell: crate::services::engines::engine_verbs(engine)
                .contains(&crate::services::engines::Verb::Shell),
        }
    }
}

/// The instance a command means; `fallback` when the machine rule chose it, so the CLI
/// says `using <instance>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub instance: String,
    pub fallback: bool,
}

/// db spec §5: `explicit` (`--service`) names it; else what the project's `.yard` asks for,
/// among the engines `allow` takes; else the machine's only such instance.
pub fn pick(
    explicit: Option<&str>,
    project_wants: Option<&[String]>,
    all: &[Candidate],
    allow: fn(&Candidate) -> bool,
) -> Result<Picked, YardError> {
    if let Some(name) = explicit {
        let found = all.iter().find(|c| c.name == name).ok_or_else(|| {
            YardError::new(ErrorCode::ServiceNotFound, format!("no service instance named {name:?}"))
                .with_hint("yard services lists the instances")
        })?;
        if !allow(found) {
            return Err(YardError::new(
                ErrorCode::DbUnsupportedEngine,
                format!("{name} is {}; its databases are not MySQL or PostgreSQL", found.display),
            )
            .with_hint("yard services lists the instances"));
        }
        return Ok(Picked { instance: name.to_string(), fallback: false });
    }
    let allowed = |name: &str| all.iter().any(|c| c.name == name && allow(c));
    if let Some(wants) = project_wants {
        let wanted: Vec<&String> = wants.iter().filter(|w| allowed(w)).collect();
        match wanted.as_slice() {
            [one] => return Ok(Picked { instance: (*one).clone(), fallback: false }),
            [] => {}
            many => {
                return Err(ambiguous(
                    many.iter().map(|s| s.as_str()).collect(),
                    "the project's .yard asks for",
                ));
            }
        }
    }
    let on_machine: Vec<&str> = all.iter().filter(|c| allow(c)).map(|c| c.name.as_str()).collect();
    match on_machine.as_slice() {
        [one] => Ok(Picked { instance: (*one).to_string(), fallback: true }),
        [] => Err(YardError::new(ErrorCode::DbNoService, "there is no MySQL or PostgreSQL instance yet")
            .with_hint("yard service add mysql")),
        many => Err(ambiguous(many.to_vec(), "this Mac has")),
    }
}

fn ambiguous(names: Vec<&str>, whose: &str) -> YardError {
    YardError::new(
        ErrorCode::DbServiceAmbiguous,
        format!("{whose} {}; say which with --service", names.join(" and ")),
    )
    .with_hint(format!("--service {}", names[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{supports_connect, supports_db};
    use yard_proto::ErrorCode;

    fn c(name: &str, engine: &str) -> Candidate {
        Candidate::of(name, crate::services::Engines::builtin().get(engine).unwrap().as_ref())
    }

    fn machine() -> Vec<Candidate> {
        vec![c("mysql-8.4", "mysql"), c("postgres-17", "postgres"), c("redis", "redis")]
    }

    #[test]
    fn explicit_must_exist_and_be_allowed() {
        let e = pick(Some("mysql-9.7"), None, &machine(), supports_db).unwrap_err();
        assert_eq!(e.code, ErrorCode::ServiceNotFound);
        let e = pick(Some("redis"), None, &machine(), supports_db).unwrap_err();
        assert_eq!(e.code, ErrorCode::DbUnsupportedEngine);
        let p = pick(Some("redis"), None, &machine(), supports_connect).unwrap();
        assert_eq!((p.instance.as_str(), p.fallback), ("redis", false));
    }

    #[test]
    fn a_project_wanting_one_gets_it() {
        let wants = vec!["mysql-8.4".to_string()];
        let p = pick(None, Some(&wants), &machine(), supports_db).unwrap();
        assert_eq!((p.instance.as_str(), p.fallback), ("mysql-8.4", false));
    }

    #[test]
    fn a_project_wanting_two_is_ambiguous() {
        let wants = vec!["mysql-8.4".to_string(), "postgres-17".to_string()];
        let e = pick(None, Some(&wants), &machine(), supports_db).unwrap_err();
        assert_eq!(e.code, ErrorCode::DbServiceAmbiguous);
        assert!(e.message.contains("mysql-8.4") && e.message.contains("postgres-17"), "{}", e.message);
    }

    #[test]
    fn a_project_wanting_no_database_falls_back_to_the_machine() {
        let wants = vec!["redis".to_string()];
        let one = vec![c("mysql-8.4", "mysql"), c("redis", "redis")];
        let p = pick(None, Some(&wants), &one, supports_db).unwrap();
        assert_eq!((p.instance.as_str(), p.fallback), ("mysql-8.4", true));
    }

    #[test]
    fn the_machine_rule() {
        let two = vec![c("mysql-8.0", "mysql"), c("mysql-8.4", "mysql")];
        let e = pick(None, None, &two, supports_db).unwrap_err();
        assert_eq!(e.code, ErrorCode::DbServiceAmbiguous);
        let e = pick(None, None, &[c("redis", "redis")], supports_db).unwrap_err();
        assert_eq!(e.code, ErrorCode::DbNoService);
        assert_eq!(e.hint.as_deref(), Some("yard service add mysql"));
    }
}
