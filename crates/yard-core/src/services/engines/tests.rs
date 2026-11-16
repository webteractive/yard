//! The built-ins and the carried recipes, through the contract. Their commands and config text
//! are pinned by `services::render`'s tests and snapshots, which predate the contract.

use super::*;
use crate::Paths;
use crate::services::render::SIGTERM;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_proto::PortRef;

const ROOT: &str = "/Users/me/.yard";

fn port(name: &str, port: u16) -> PortRef {
    PortRef { name: name.into(), port }
}

fn get(key: &str) -> Arc<dyn ServiceEngine> {
    Engines::builtin().get(key).unwrap_or_else(|| panic!("{key} missing")).clone()
}

fn ctx<'a>(instance: &'a str, home: &'a str, paths: &'a Paths, ports: &'a [PortRef]) -> InstanceCtx<'a> {
    InstanceCtx { instance, home: Path::new(home), paths, ports, macos_user: "me", node: None }
}

#[test]
fn the_set_is_the_builtins_and_the_carried_recipes() {
    let engines = Engines::builtin();
    assert_eq!(engines.keys(), ["minio", "mysql", "postgres", "redis"]);
    for key in ["mysql", "postgres", "minio"] {
        assert_eq!(get(key).origin(), Origin::BuiltIn, "{key}");
    }
    assert_eq!(get("redis").origin(), Origin::Carried);
    assert_eq!(
        [get("mysql").display(), get("postgres").display(), get("redis").display(), get("minio").display()],
        ["MySQL", "PostgreSQL", "Redis", "MinIO"]
    );
}

#[test]
fn each_builtin_says_what_it_supports() {
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
    assert_eq!(get("mysql").lines(), Some(&MYSQL_LINES));
    assert_eq!(get("redis").lines(), None);
    let timeout = Some(crate::services::DATABASE_START_TIMEOUT);
    assert_eq!((get("mysql").start_timeout(), get("postgres").start_timeout()), (timeout, timeout));
    assert_eq!((get("redis").start_timeout(), get("minio").start_timeout()), (None, None));
    let ports = |k: &str| get(k).ports().into_iter().map(|p| (p.suffix, p.usual)).collect::<Vec<_>>();
    assert_eq!(ports("minio"), [(String::new(), 9000), ("console".into(), 9001)]);
    assert_eq!(ports("mysql"), [(String::new(), 3306)]);
    assert_eq!(ports("postgres"), [(String::new(), 5432)]);
    assert_eq!(ports("redis"), [(String::new(), 6379)]);
}

#[test]
fn accounts_and_env_lines() {
    let paths = Paths::new(ROOT);
    let acct = |u: Option<&str>, p: Option<&str>| yard_proto::Account {
        user: u.map(str::to_string),
        password: p.map(str::to_string),
    };
    assert_eq!(get("mysql").account("me"), acct(Some("root"), Some("")));
    assert_eq!(get("postgres").account("me"), acct(Some("me"), Some("")));
    assert_eq!(get("redis").account("me"), acct(None, None));
    assert_eq!(get("minio").account("me"), acct(Some("minioadmin"), Some("minioadmin")));
    let my = [port("mysql-8.4", 3307)];
    assert_eq!(
        get("mysql").env_lines(&ctx("mysql-8.4", "/k", &paths, &my)),
        ["DB_CONNECTION=mysql", "DB_HOST=127.0.0.1", "DB_PORT=3307", "DB_USERNAME=root", "DB_PASSWORD="]
    );
    let pg = [port("postgres-17", 5433)];
    assert_eq!(
        get("postgres").env_lines(&ctx("postgres-17", "/k", &paths, &pg)),
        ["DB_CONNECTION=pgsql", "DB_HOST=127.0.0.1", "DB_PORT=5433", "DB_USERNAME=me", "DB_PASSWORD="]
    );
    let redis = [port("redis", 6380)];
    assert_eq!(
        get("redis").env_lines(&ctx("redis", "/k", &paths, &redis)),
        ["REDIS_HOST=127.0.0.1", "REDIS_PORT=6380", "REDIS_PASSWORD=null"]
    );
    // The API port, not the console's, whichever comes first.
    let minio = [port("minio-console", 9101), port("minio", 9100)];
    assert_eq!(
        get("minio").env_lines(&ctx("minio", "/k", &paths, &minio)),
        [
            "AWS_ACCESS_KEY_ID=minioadmin",
            "AWS_SECRET_ACCESS_KEY=minioadmin",
            "AWS_DEFAULT_REGION=us-east-1",
            "AWS_ENDPOINT=http://127.0.0.1:9100",
            "AWS_USE_PATH_STYLE_ENDPOINT=true",
        ]
    );
}

/// Redis moved to a carried recipe (recipes spec §10); its conf is pinned byte for byte by
/// `services::render`'s `redis_conf` snapshot, taken from the code it replaced.
#[test]
fn redis_runs_as_it_did() {
    let paths = Paths::new("/Users/me/Library/Application Support/Ya\"rd");
    let ports = [port("redis", 26379)];
    let redis = get("redis");
    let c = ctx("redis", "/opt/homebrew/opt/redis", &paths, &ports);
    let cmd = redis.command(&c);
    assert_eq!(cmd.program, PathBuf::from("/opt/homebrew/opt/redis/bin/redis-server"));
    assert_eq!(cmd.args, [paths.service_conf("redis", "redis.conf").into_os_string()]);
    assert!(cmd.env.is_empty());
    assert_eq!(cmd.stop_signal, SIGTERM);
    assert_eq!(redis.init(&c), None);
    assert_eq!(redis.user_overrides().as_deref(), Some(""));
    let (file, text) = redis.config_file(&c).unwrap();
    assert_eq!(file, paths.service_conf("redis", "redis.conf"));
    assert!(
        text.contains("dir \"/Users/me/Library/Application Support/Ya\\\"rd/services/redis/data\"\n"),
        "{text}"
    );
}

#[test]
fn connect_details() {
    let paths = Paths::new(ROOT);
    let redis_ports = [port("redis", 26379)];
    let redis = get("redis").connect(&ctx("redis", "/opt/homebrew/opt/redis", &paths, &redis_ports)).unwrap();
    assert_eq!(redis.url, "redis://127.0.0.1:26379");
    assert_eq!(redis.socket, None);
    assert_eq!(
        redis.shell,
        Some(vec!["/opt/homebrew/opt/redis/bin/redis-cli".into(), "-p".into(), "26379".into()])
    );
    assert_eq!(
        redis.clients,
        ["TablePlus: paste the URL", "Another Redis client: a connection to the host and port above"]
    );
    let my_ports = [port("mysql-8.4", 23307)];
    let mysql = get("mysql").connect(&ctx("mysql-8.4", "/k", &paths, &my_ports)).unwrap();
    assert_eq!(mysql.url, "mysql://root@127.0.0.1:23307");
    assert_eq!(mysql.socket, Some(paths.mysql_socket("mysql-8.4")));
    assert_eq!(mysql.shell, None);
    assert_eq!(mysql.clients.len(), 2);
    let pg_ports = [port("postgres-17", 25432)];
    let pg = get("postgres").connect(&ctx("postgres-17", "/k", &paths, &pg_ports)).unwrap();
    assert_eq!(pg.url, "postgresql://me@127.0.0.1:25432");
    assert_eq!(pg.socket, Some(paths.run_dir()));
    let minio_ports = [port("minio", 29000)];
    assert_eq!(get("minio").connect(&ctx("minio", "/k", &paths, &minio_ports)), None);
}

#[test]
fn main_port_skips_the_console() {
    let minio = get("minio");
    let ports = [port("minio-console", 29001), port("minio", 29000)];
    assert_eq!(main_port(minio.as_ref(), "minio", &ports), 29000);
    assert_eq!(main_port(minio.as_ref(), "minio", &[]), 9000);
    assert_eq!(main_port(minio.as_ref(), "minio", &[port("minio-console", 29001)]), 9000);
}

#[test]
fn every_carried_recipe_loads() {
    let texts = carried::texts();
    assert!(!texts.is_empty());
    assert_eq!(carried::all().len(), texts.len(), "a carried recipe failed to load");
    for (name, text) in texts {
        let file = PathBuf::from(format!("{name}.toml"));
        if let Err(e) = recipe::parse_recipe(text, &file, Origin::Carried, &LoadOptions::default()) {
            panic!("{name}: {e:?}");
        }
    }
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

#[test]
fn instance_names() {
    assert_eq!(instance_name("mysql", None), "mysql-8.4");
    assert_eq!(instance_name("mysql", Some("8.0")), "mysql-8.0");
    assert_eq!(instance_name("postgres", None), "postgres-17");
    assert_eq!(instance_name("redis", None), "redis");
    assert_eq!(instance_name("memcached", None), "memcached");
    let parse = |n: &str| parse_instance(n).map(|(k, l)| (k.to_string(), l));
    assert_eq!(parse("mysql-8.4"), Some(("mysql".into(), Some("8.4".into()))));
    assert_eq!(parse("postgres-17"), Some(("postgres".into(), Some("17".into()))));
    assert_eq!(parse("redis"), Some(("redis".into(), None)));
    assert_eq!(parse("meili-search"), Some(("meili-search".into(), None)));
    for bad in ["mysql", "mysql-8", "postgres-17.2", "mysql-latest", "Mongo", ""] {
        assert_eq!(parse(bad), None, "{bad}");
    }
}

mod user_recipes {
    use super::*;

    fn recipe(key: &str, env: &str) -> String {
        format!(
            "schema = 1\nkey = \"{key}\"\nname = \"{key}\"\n\n[source]\nkind = \"brew\"\nformula = \"{key}\"\n\n\
             [ports]\nmain = 7000\n\n[run]\ncommand = [\"{{home}}/bin/{key}\"]\n\n[health]\ntcp = \"main\"\n\n\
             [env]\nlines = [\"{env}\"]\n"
        )
    }

    fn file(key: &str) -> PathBuf {
        PathBuf::from(format!("/u/.yard/engines/{key}.toml"))
    }

    fn parse(key: &str, text: &str) -> Result<RecipeEngine, RecipeError> {
        recipe::parse_recipe(text, &file(key), Origin::User(file(key)), &LoadOptions::default())
    }

    fn env_of(engines: &Engines, key: &str) -> Vec<String> {
        let paths = Paths::new("/u/.yard");
        engines.get(key).unwrap().env_lines(&ctx(key, "/k", &paths, &[]))
    }

    #[test]
    fn a_user_recipe_is_added() {
        let mut last_good = BTreeMap::new();
        let engines = Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("echo", &recipe("echo", "A=1"))],
            &mut last_good,
        );
        assert_eq!(engines.get("echo").unwrap().origin(), Origin::User(file("echo")));
        assert!(engines.errors().is_empty());
        assert!(last_good.contains_key("echo"));
        assert!(engines.get("mysql").is_some(), "the built-ins stay");
    }

    #[test]
    fn user_recipe_replaces_carried() {
        let mut last_good = BTreeMap::new();
        let engines = Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("redis", &recipe("redis", "MINE=1"))],
            &mut last_good,
        );
        assert_eq!(engines.get("redis").unwrap().origin(), Origin::User(file("redis")));
        assert_eq!(env_of(&engines, "redis"), ["MINE=1"]);
    }

    #[test]
    fn user_recipe_cannot_replace_builtin() {
        let mut last_good = BTreeMap::new();
        let engines = Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("mysql", &recipe("mysql", "X=1"))],
            &mut last_good,
        );
        assert_eq!(engines.get("mysql").unwrap().origin(), Origin::BuiltIn);
        assert_eq!(engines.errors().len(), 1);
        assert!(engines.errors()[0].message.contains("reserved"), "{:?}", engines.errors());
    }

    #[test]
    fn broken_edit_keeps_last_good() {
        let mut last_good = BTreeMap::new();
        Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("echo", &recipe("echo", "A=1"))],
            &mut last_good,
        );
        let engines = Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("echo", "schema = 1\nkey = ")],
            &mut last_good,
        );
        assert_eq!(env_of(&engines, "echo"), ["A=1"], "the last good version stays in force");
        assert!(engines.errors().is_empty(), "the engine exists, so the error is its own");
        let stale = engines.stale("echo").expect("the error rides on the engine");
        assert_eq!(stale.file, file("echo"));
        assert!(stale.line.is_some());
    }

    #[test]
    fn broken_with_no_last_good_is_an_error_and_no_engine() {
        let mut last_good = BTreeMap::new();
        let engines = Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("echo", "schema = 1\nkey = ")],
            &mut last_good,
        );
        assert!(engines.get("echo").is_none());
        assert_eq!(engines.errors().len(), 1);
    }

    #[test]
    fn broken_user_recipe_shadowing_carried_falls_back_to_it() {
        let mut last_good = BTreeMap::new();
        let engines = Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("redis", "schema = 1\nkey = ")],
            &mut last_good,
        );
        assert_eq!(engines.get("redis").unwrap().origin(), Origin::Carried);
        assert!(engines.stale("redis").is_some());
    }

    #[test]
    fn a_deleted_recipe_is_forgotten() {
        let mut last_good = BTreeMap::new();
        Engines::with_user(
            Engines::builtin_shared(),
            vec![parse("echo", &recipe("echo", "A=1"))],
            &mut last_good,
        );
        let engines = Engines::with_user(Engines::builtin_shared(), vec![], &mut last_good);
        assert!(engines.get("echo").is_none());
        assert!(last_good.is_empty());
    }
}

#[test]
fn verbs_follow_the_contract() {
    use Verb::*;
    assert_eq!(engine_verbs(get("mysql").as_ref()), [Shell, List, Create, Dump, Import, Connect, Open]);
    assert_eq!(engine_verbs(get("postgres").as_ref()), [Shell, List, Create, Dump, Import, Connect, Open]);
    assert_eq!(engine_verbs(get("redis").as_ref()), [Shell, Connect, Open], "redis-cli, its URL");
    assert_eq!(engine_verbs(get("minio").as_ref()), [Open], "its console only");
    assert_eq!(Verb::parse("dump"), Some(Dump));
    assert_eq!(Verb::parse("prompt"), None);
    assert_eq!(Dump.name(), "dump");
}
