//! The contract's built-ins must say exactly what the enum's match arms say (recipes plan
//! Task 2); Task 5 deletes the arms and keeps these as the built-ins' own tests.

use super::*;
use crate::Paths;
use crate::services::render;
use crate::services::{DATABASE_START_TIMEOUT, Engine};
use std::path::Path;
use yard_proto::PortRef;

const ROOT: &str = "/Users/me/.yard";

fn port(name: &str, port: u16) -> PortRef {
    PortRef { name: name.into(), port }
}

/// Each instance with the ports a test daemon would give it, and its enum engine.
fn cases() -> Vec<(&'static str, Engine, Vec<PortRef>, &'static str)> {
    vec![
        ("mysql-8.0", Engine::Mysql, vec![port("mysql-8.0", 23306)], "/opt/homebrew/opt/mysql@8.0"),
        ("mysql-8.4", Engine::Mysql, vec![port("mysql-8.4", 23307)], "/opt/homebrew/opt/mysql@8.4"),
        ("mysql-9.7", Engine::Mysql, vec![port("mysql-9.7", 23308)], "/opt/homebrew/opt/mysql@9.7"),
        (
            "postgres-17",
            Engine::Postgres,
            vec![port("postgres-17", 25432)],
            "/opt/homebrew/opt/postgresql@17",
        ),
        ("redis", Engine::Redis, vec![port("redis", 26379)], "/opt/homebrew/opt/redis"),
        (
            "minio",
            Engine::Minio,
            vec![port("minio", 29000), port("minio-console", 29001)],
            "/opt/homebrew/opt/minio",
        ),
    ]
}

#[test]
fn parity_with_the_enum() {
    let paths = Paths::new(ROOT);
    let engines = Engines::builtin();
    for (instance, engine, ports, keg) in cases() {
        let e = engines.get(engine.key()).unwrap_or_else(|| panic!("{} missing", engine.key()));
        let ctx = InstanceCtx {
            instance,
            home: Path::new(keg),
            paths: &paths,
            ports: &ports,
            macos_user: "me",
            node: None,
        };
        assert_eq!(e.key(), engine.key());
        assert_eq!(e.display(), engine.display(), "{instance}");
        assert_eq!(
            e.command(&ctx),
            render::command(engine, instance, Path::new(keg), &paths, &ports),
            "{instance}"
        );
        assert_eq!(
            e.init(&ctx),
            render::init_command(engine, instance, Path::new(keg), &paths, "me"),
            "{instance}"
        );
        assert_eq!(e.config_file(&ctx), render::config_file(engine, instance, &paths, &ports), "{instance}");
        assert_eq!(e.user_overrides().as_deref(), render::user_overrides(engine), "{instance}");
        let account = engine.account("me");
        assert_eq!(e.account("me"), account, "{instance}");
        assert_eq!(e.env_lines(&ctx), crate::services::env_lines(engine, &ports, &account), "{instance}");
        let usual: Vec<(String, u16)> =
            engine.usual_ports().iter().map(|(s, p)| (s.to_string(), *p)).collect();
        let got: Vec<(String, u16)> = e.ports().into_iter().map(|p| (p.suffix, p.usual)).collect();
        assert_eq!(got, usual, "{instance}");
        assert_eq!(e.lines().is_some(), engine.versioned(), "{instance}");
        if let Some(lines) = e.lines() {
            assert_eq!(Some(lines.default_line), engine.default_line());
            assert_eq!(lines.base_formula, engine.base_formula());
            for line in ["8.4", "17", "8", "x", "8.4.1", ""] {
                assert_eq!(lines.valid(line), engine.valid_line(line), "{instance} {line}");
            }
        }
        let timeout = matches!(engine, Engine::Mysql | Engine::Postgres).then_some(DATABASE_START_TIMEOUT);
        assert_eq!(e.start_timeout(), timeout, "{instance}");
        assert!(e.may_sleep());
        assert_eq!(e.origin(), Origin::BuiltIn);
    }
}

#[test]
fn each_builtin_says_what_it_supports() {
    let engines = Engines::builtin();
    let get = |k: &str| engines.get(k).unwrap().clone();
    assert_eq!(get("mysql").databases(), Some(DbFlavor::Mysql));
    assert_eq!(get("postgres").databases(), Some(DbFlavor::Postgres));
    assert_eq!(get("redis").databases(), None);
    assert_eq!(get("minio").databases(), None);
    assert_eq!(
        get("minio").console(),
        Some(Console { port: "console".into(), host_header: HostHeader::Upstream, nginx_overrides: true })
    );
    assert_eq!(get("redis").console(), None);
    assert_eq!(get("redis").dirs(), vec!["data".to_string()]);
    assert!(get("mysql").dirs().is_empty());
    assert_eq!(get("minio").source(), SourceSpec::Brew { formula: "minio/stable/minio".into() });
    assert_eq!(get("redis").source(), SourceSpec::Brew { formula: "redis".into() });
    assert_eq!(get("mysql").source(), SourceSpec::BrewLines(&MYSQL_LINES));
    assert_eq!(engines.keys(), ["minio", "mysql", "postgres", "redis"]);
}

#[test]
fn connect_details() {
    let paths = Paths::new(ROOT);
    let engines = Engines::builtin();
    let ctx = |instance: &'static str, ports: &'static [PortRef], home: &'static str| InstanceCtx {
        instance,
        home: Path::new(home),
        paths: Box::leak(Box::new(paths.clone())),
        ports,
        macos_user: "me",
        node: None,
    };
    let redis_ports: &'static [PortRef] = Box::leak(Box::new([port("redis", 26379)]));
    let redis =
        engines.get("redis").unwrap().connect(&ctx("redis", redis_ports, "/opt/homebrew/opt/redis")).unwrap();
    assert_eq!(redis.url, "redis://127.0.0.1:26379");
    assert_eq!(redis.socket, None);
    assert_eq!(
        redis.shell,
        Some(vec!["/opt/homebrew/opt/redis/bin/redis-cli".into(), "-p".into(), "26379".into()])
    );
    assert_eq!(redis.clients.len(), 2);
    let my_ports: &'static [PortRef] = Box::leak(Box::new([port("mysql-8.4", 23307)]));
    let mysql = engines.get("mysql").unwrap().connect(&ctx("mysql-8.4", my_ports, "/k")).unwrap();
    assert_eq!(mysql.url, "mysql://root@127.0.0.1:23307");
    assert_eq!(mysql.socket, Some(paths.mysql_socket("mysql-8.4")));
    assert_eq!(mysql.shell, None);
    let pg_ports: &'static [PortRef] = Box::leak(Box::new([port("postgres-17", 25432)]));
    let pg = engines.get("postgres").unwrap().connect(&ctx("postgres-17", pg_ports, "/k")).unwrap();
    assert_eq!(pg.url, "postgresql://me@127.0.0.1:25432");
    assert_eq!(pg.socket, Some(paths.run_dir()));
    let minio_ports: &'static [PortRef] = Box::leak(Box::new([port("minio", 29000)]));
    assert_eq!(engines.get("minio").unwrap().connect(&ctx("minio", minio_ports, "/k")), None);
}

#[test]
fn main_port_skips_the_console() {
    let engines = Engines::builtin();
    let minio = engines.get("minio").unwrap();
    let ports = [port("minio-console", 29001), port("minio", 29000)];
    assert_eq!(main_port(minio.as_ref(), "minio", &ports), 29000);
    assert_eq!(main_port(minio.as_ref(), "minio", &[]), 9000);
    assert_eq!(main_port(minio.as_ref(), "minio", &[port("minio-console", 29001)]), 9000);
}

#[test]
fn engine_key_rules() {
    for ok in ["a", "redis", "meili-search", "x9", "a23456789012345678901234567890bc"] {
        assert!(EngineKey::new(ok).is_ok(), "{ok}");
    }
    for bad in ["", "A", "1a", "a_b", "-a", "a b", "a23456789012345678901234567890bcd"] {
        assert!(EngineKey::new(bad).is_err(), "{bad}");
    }
    for reserved in ["mysql", "postgres", "minio", "emailulator", "mysql-x", "postgres-17"] {
        assert!(EngineKey::new(reserved).unwrap().is_reserved(), "{reserved}");
    }
    assert!(!EngineKey::new("redis").unwrap().is_reserved());
    let key = EngineKey::new("valkey").unwrap();
    assert_eq!(serde_json::to_value(&key).unwrap(), serde_json::json!("valkey"));
    assert_eq!(key.to_string(), "valkey");
}

#[test]
fn versioned_lines_without_a_set() {
    assert_eq!(versioned_lines("mysql"), Some(&MYSQL_LINES));
    assert_eq!(versioned_lines("postgres"), Some(&POSTGRES_LINES));
    assert_eq!(versioned_lines("redis"), None);
    assert_eq!(MYSQL_LINES.plain_line("8.4.13"), Some("8.4".into()));
    assert_eq!(POSTGRES_LINES.plain_line("18.1"), Some("18".into()));
}
