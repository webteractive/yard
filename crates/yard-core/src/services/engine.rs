//! The four engines Yard runs, and the facts about each that never change.

use serde::{Deserialize, Serialize};
use yard_proto::{Account, PortRef};

/// A service engine. Serializes as its key: `engine = "postgres"` in `services.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mysql,
    Postgres,
    Redis,
    Minio,
}

impl Engine {
    pub const ALL: [Engine; 4] = [Engine::Mysql, Engine::Postgres, Engine::Redis, Engine::Minio];

    /// The `.yard` `[services]` key and the `engine` value in `services.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Engine::Mysql => "mysql",
            Engine::Postgres => "postgres",
            Engine::Redis => "redis",
            Engine::Minio => "minio",
        }
    }

    pub fn from_key(key: &str) -> Option<Engine> {
        Engine::ALL.into_iter().find(|e| e.key() == key)
    }

    /// `MySQL`, `PostgreSQL`, `Redis`, `MinIO`: the one table lives in `yard-proto`.
    pub fn display(self) -> &'static str {
        yard_proto::engine_display(self.key())
    }

    /// The brew formula without a line: `mysql@8.4` is `mysql`'s 8.4 line.
    pub fn base_formula(self) -> &'static str {
        match self {
            Engine::Mysql => "mysql",
            Engine::Postgres => "postgresql",
            Engine::Redis => "redis",
            Engine::Minio => "minio",
        }
    }

    /// Whether brew ships several release lines of it (MySQL and PostgreSQL).
    pub fn versioned(self) -> bool {
        matches!(self, Engine::Mysql | Engine::Postgres)
    }

    /// The line `true` and a bare `yard service add` mean.
    pub fn default_line(self) -> Option<&'static str> {
        match self {
            Engine::Mysql => Some("8.4"),
            Engine::Postgres => Some("17"),
            Engine::Redis | Engine::Minio => None,
        }
    }

    /// The ports the engine is offered first, as `(suffix, port)`. An empty suffix names
    /// the registry entry `<instance>`, any other `<instance>-<suffix>`.
    pub fn usual_ports(self) -> &'static [(&'static str, u16)] {
        match self {
            Engine::Mysql => &[("", 3306)],
            Engine::Postgres => &[("", 5432)],
            Engine::Redis => &[("", 6379)],
            Engine::Minio => &[("", 9000), ("console", 9001)],
        }
    }

    /// The login shown for an instance (spec 7.5). Only PostgreSQL's depends on the
    /// machine: it takes the macOS user, which the caller reads.
    pub fn account(self, macos_user: &str) -> Account {
        let account = |user: Option<&str>, password: Option<&str>| Account {
            user: user.map(str::to_string),
            password: password.map(str::to_string),
        };
        match self {
            Engine::Mysql => account(Some("root"), Some("")),
            Engine::Postgres => account(Some(macos_user), Some("")),
            Engine::Redis => account(None, None),
            Engine::Minio => account(Some("minioadmin"), Some("minioadmin")),
        }
    }

    /// Whether `line` is a release line of this engine: `8.4` for MySQL, `17` for
    /// PostgreSQL; the others have none.
    pub fn valid_line(self, line: &str) -> bool {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        match self {
            Engine::Mysql => matches!(line.split_once('.'), Some((a, b)) if digits(a) && digits(b)),
            Engine::Postgres => digits(line),
            Engine::Redis | Engine::Minio => false,
        }
    }
}

/// The instance name: `mysql-8.4`, `postgres-17`, `redis`, `minio`. A versioned engine
/// with no line gets its default line.
pub fn instance_name(engine: Engine, line: Option<&str>) -> String {
    if !engine.versioned() {
        return engine.key().to_string();
    }
    match line.or(engine.default_line()) {
        Some(line) => format!("{}-{line}", engine.key()),
        None => engine.key().to_string(),
    }
}

/// The inverse of [`instance_name`]; `None` for a name that is not one (`mysql` alone,
/// `redis-8`).
pub fn parse_instance(name: &str) -> Option<(Engine, Option<String>)> {
    if let Some(engine) = Engine::from_key(name) {
        return (!engine.versioned()).then_some((engine, None));
    }
    let (key, line) = name.split_once('-')?;
    let engine = Engine::from_key(key)?;
    (engine.versioned() && engine.valid_line(line)).then(|| (engine, Some(line.to_string())))
}

/// The `.env` lines for one instance (spec 7.3). `ports` are the instance's registry
/// entries; MinIO's API port is the one that is not `-console`.
pub fn env_lines(engine: Engine, ports: &[PortRef], account: &Account) -> Vec<String> {
    let port = match engine {
        Engine::Minio => ports.iter().find(|p| !p.name.ends_with("-console")),
        _ => ports.first(),
    }
    .or(ports.first())
    .map(|p| p.port)
    .unwrap_or_else(|| engine.usual_ports()[0].1);
    let user = account.user.clone().unwrap_or_default();
    let password = account.password.clone().unwrap_or_default();
    match engine {
        Engine::Mysql | Engine::Postgres => {
            let connection = if engine == Engine::Mysql { "mysql" } else { "pgsql" };
            vec![
                format!("DB_CONNECTION={connection}"),
                "DB_HOST=127.0.0.1".to_string(),
                format!("DB_PORT={port}"),
                format!("DB_USERNAME={user}"),
                format!("DB_PASSWORD={password}"),
            ]
        }
        Engine::Redis => vec![
            "REDIS_HOST=127.0.0.1".to_string(),
            format!("REDIS_PORT={port}"),
            "REDIS_PASSWORD=null".to_string(),
        ],
        Engine::Minio => vec![
            format!("AWS_ACCESS_KEY_ID={user}"),
            format!("AWS_SECRET_ACCESS_KEY={password}"),
            "AWS_DEFAULT_REGION=us-east-1".to_string(),
            format!("AWS_ENDPOINT=http://127.0.0.1:{port}"),
            "AWS_USE_PATH_STYLE_ENDPOINT=true".to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, port: u16) -> PortRef {
        PortRef { name: name.into(), port }
    }

    #[test]
    fn instance_names() {
        assert_eq!(instance_name(Engine::Mysql, Some("8.4")), "mysql-8.4");
        assert_eq!(instance_name(Engine::Postgres, Some("17")), "postgres-17");
        assert_eq!(instance_name(Engine::Redis, None), "redis");
        assert_eq!(instance_name(Engine::Minio, None), "minio");
        for name in ["mysql-8.4", "postgres-17", "redis", "minio"] {
            let (engine, line) = parse_instance(name).unwrap();
            assert_eq!(instance_name(engine, line.as_deref()), name);
        }
        assert_eq!(parse_instance("mysql-8.4"), Some((Engine::Mysql, Some("8.4".into()))));
        assert_eq!(parse_instance("redis"), Some((Engine::Redis, None)));
        for bad in ["mysql", "postgres", "redis-8", "minio-1", "mysql-8", "postgres-17.2", "mariadb-11", ""] {
            assert_eq!(parse_instance(bad), None, "{bad}");
        }
    }

    #[test]
    fn keys_round_trip_and_serialize_as_the_key() {
        for engine in [Engine::Mysql, Engine::Postgres, Engine::Redis, Engine::Minio] {
            assert_eq!(Engine::from_key(engine.key()), Some(engine));
            assert_eq!(serde_json::to_string(&engine).unwrap(), format!("\"{}\"", engine.key()));
            assert_eq!(serde_json::from_str::<Engine>(&format!("\"{}\"", engine.key())).unwrap(), engine);
            assert_eq!(engine.display(), yard_proto::engine_display(engine.key()));
        }
        assert_eq!(Engine::from_key("mariadb"), None);
    }

    #[test]
    fn the_table_matches_the_spec() {
        assert_eq!(Engine::Mysql.base_formula(), "mysql");
        assert_eq!(Engine::Postgres.base_formula(), "postgresql");
        assert_eq!(Engine::Redis.base_formula(), "redis");
        assert_eq!(Engine::Minio.base_formula(), "minio");
        assert_eq!(Engine::Mysql.default_line(), Some("8.4"));
        assert_eq!(Engine::Postgres.default_line(), Some("17"));
        assert_eq!(Engine::Redis.default_line(), None);
        assert!(Engine::Mysql.versioned() && Engine::Postgres.versioned());
        assert!(!Engine::Redis.versioned() && !Engine::Minio.versioned());
        assert_eq!(Engine::Mysql.usual_ports(), [("", 3306)]);
        assert_eq!(Engine::Postgres.usual_ports(), [("", 5432)]);
        assert_eq!(Engine::Redis.usual_ports(), [("", 6379)]);
        assert_eq!(Engine::Minio.usual_ports(), [("", 9000), ("console", 9001)]);
        assert!(Engine::Mysql.valid_line("8.4") && !Engine::Mysql.valid_line("8"));
        assert!(!Engine::Mysql.valid_line("8.4.1") && !Engine::Mysql.valid_line("8."));
        assert!(!Engine::Mysql.valid_line("") && !Engine::Postgres.valid_line(""));
        assert!(Engine::Postgres.valid_line("17") && !Engine::Postgres.valid_line("17.2"));
        assert!(!Engine::Redis.valid_line("8") && !Engine::Minio.valid_line("1"));
    }

    #[test]
    fn accounts_follow_the_spec() {
        let acct = |u: Option<&str>, p: Option<&str>| Account {
            user: u.map(Into::into),
            password: p.map(Into::into),
        };
        assert_eq!(Engine::Mysql.account("me"), acct(Some("root"), Some("")));
        assert_eq!(Engine::Postgres.account("me"), acct(Some("me"), Some("")));
        assert_eq!(Engine::Redis.account("me"), acct(None, None));
        assert_eq!(Engine::Minio.account("me"), acct(Some("minioadmin"), Some("minioadmin")));
    }

    #[test]
    fn env_lines_match_spec() {
        let mysql = Engine::Mysql.account("me");
        assert_eq!(
            env_lines(Engine::Mysql, &[port("mysql-8.4", 3307)], &mysql),
            ["DB_CONNECTION=mysql", "DB_HOST=127.0.0.1", "DB_PORT=3307", "DB_USERNAME=root", "DB_PASSWORD=",]
        );
        let pg = Engine::Postgres.account("me");
        assert_eq!(
            env_lines(Engine::Postgres, &[port("postgres-17", 5433)], &pg),
            ["DB_CONNECTION=pgsql", "DB_HOST=127.0.0.1", "DB_PORT=5433", "DB_USERNAME=me", "DB_PASSWORD=",]
        );
        let redis = Engine::Redis.account("me");
        assert_eq!(
            env_lines(Engine::Redis, &[port("redis", 6380)], &redis),
            ["REDIS_HOST=127.0.0.1", "REDIS_PORT=6380", "REDIS_PASSWORD=null"]
        );
        let minio = Engine::Minio.account("me");
        // The console port comes first here: the API is found by name, not by position.
        let ports = [port("minio-console", 9101), port("minio", 9000)];
        assert_eq!(
            env_lines(Engine::Minio, &ports, &minio),
            [
                "AWS_ACCESS_KEY_ID=minioadmin",
                "AWS_SECRET_ACCESS_KEY=minioadmin",
                "AWS_DEFAULT_REGION=us-east-1",
                "AWS_ENDPOINT=http://127.0.0.1:9000",
                "AWS_USE_PATH_STYLE_ENDPOINT=true",
            ]
        );
    }
}
