//! `yard db` (db spec): which databases and instances a command means, the SQL Yard runs,
//! what a GUI client needs to connect, and which PostgreSQL keg's tools to run. Pure.

pub mod command;
pub mod connect;
pub mod flags;
pub mod resolve;
pub mod sql;

use crate::services::Engine;
use std::path::PathBuf;
use yard_proto::{ErrorCode, YardError};

/// The longest database name Yard takes: PostgreSQL's limit, one under MySQL's.
pub const MAX_NAME: usize = 63;

/// `[A-Za-z0-9_-]`, 1 to 63 characters (db spec §5).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn check_name(name: &str) -> Result<(), YardError> {
    if valid_name(name) {
        return Ok(());
    }
    Err(YardError::new(
        ErrorCode::InvalidArgument,
        format!(
            "{name:?} is not a database name Yard takes: use letters, digits, _ and -, at most {MAX_NAME} characters"
        ),
    ))
}

/// The engines `yard db` lists, creates, opens, dumps and imports.
pub fn supports_db(engine: Engine) -> bool {
    matches!(engine, Engine::Mysql | Engine::Postgres)
}

/// The engines `yard db connect` describes.
pub fn supports_connect(engine: Engine) -> bool {
    supports_db(engine) || engine == Engine::Redis
}

/// The newest installed PostgreSQL keg at or above the instance's major (db spec §7.1): a
/// newer client reads an older server, while an older `pg_restore` refuses a newer archive.
pub fn pick_postgres_keg(instance_major: u32, installed: &[(u32, PathBuf)]) -> Option<(u32, PathBuf)> {
    installed.iter().filter(|(major, _)| *major >= instance_major).max_by_key(|(major, _)| *major).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::Engine;
    use std::path::PathBuf;
    use yard_proto::ErrorCode;

    #[test]
    fn names() {
        for ok in ["shop", "my_app", "my-app", "A1", &"a".repeat(63)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", &"a".repeat(64), "my app", "shop;drop", "sh`op", "\"shop\"", "ünï", "a.b"] {
            assert!(!valid_name(bad), "{bad}");
        }
        let e = check_name("my app").unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
        assert!(e.message.contains("63"), "{}", e.message);
    }

    #[test]
    fn engines_yard_db_works_on() {
        assert!(supports_db(Engine::Mysql) && supports_db(Engine::Postgres));
        assert!(!supports_db(Engine::Redis) && !supports_db(Engine::Minio));
        assert!(supports_connect(Engine::Redis) && !supports_connect(Engine::Minio));
    }

    #[test]
    fn postgres_keg_newest_at_or_above() {
        let (a, b, c) = (PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/c"));
        let all = [(16, a.clone()), (17, b.clone()), (18, c.clone())];
        assert_eq!(pick_postgres_keg(17, &all), Some((18, c)));
        assert_eq!(pick_postgres_keg(17, &[(16, a)]), None);
        assert_eq!(pick_postgres_keg(17, &[(17, b.clone())]), Some((17, b)));
    }
}
