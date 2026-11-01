//! Each engine's unit command, init command and generated config file (spec 5.1 to 5.4).
//! Pure: it builds argument lists and text, and the daemon spawns and writes them.

use super::Engine;
use crate::Paths;
use crate::render::header;
use crate::render::quote::nginx_str;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use yard_proto::PortRef;

/// SIGINT and SIGTERM have the same numbers on every platform Yard supports. They are
/// named here so `yard-core` needs no `libc`; the daemon passes the number to `kill`.
pub const SIGINT: i32 = 2;
pub const SIGTERM: i32 = 15;

/// A program to run for an instance: absolute, with no shell and no PATH lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    /// The signal that stops it cleanly.
    pub stop_signal: i32,
}

/// The registry entry that is the instance's main port: the one named for the unit, else the
/// first that is not a console, else the engine's usual port.
fn main_port(engine: Engine, instance: &str, ports: &[PortRef]) -> u16 {
    ports
        .iter()
        .find(|p| p.name == instance)
        .or_else(|| ports.iter().find(|p| !p.name.ends_with("-console")))
        .map_or_else(|| engine.usual_ports()[0].1, |p| p.port)
}

/// MinIO's console port: `<instance>-console`, else the engine's usual console port.
fn console_port(engine: Engine, instance: &str, ports: &[PortRef]) -> u16 {
    let name = format!("{instance}-console");
    ports.iter().find(|p| p.name == name).map_or_else(
        || engine.usual_ports().iter().find(|(suffix, _)| *suffix == "console").map_or(0, |(_, port)| *port),
        |p| p.port,
    )
}

fn arg(text: impl Into<OsString>) -> OsString {
    text.into()
}

/// What a MySQL line does with `mysql_native_password`, the only login PHP 7.0 to 7.3's mysqlnd
/// knows (`caching_sha2_password` arrived in 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePassword {
    /// 8.0 to 8.3 load it, and only need it made the default.
    Loaded,
    /// 8.4 has it but leaves it off.
    Off,
    /// 9.0 removed it: old PHP cannot log in at all.
    Gone,
}

/// The line comes from the instance name (`mysql-8.4`), which is always made from it.
fn native_password(instance: &str) -> NativePassword {
    let line = super::parse_instance(instance).and_then(|(_, line)| line).unwrap_or_default();
    let number = |s: Option<&str>| s.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let mut parts = line.split('.');
    match (number(parts.next()), number(parts.next())) {
        (8, minor) if minor < 4 => NativePassword::Loaded,
        (8, _) => NativePassword::Off,
        _ => NativePassword::Gone,
    }
}

/// Whether PHP 7.0 to 7.3 can log in to the MySQL `instance` at all: MySQL 9 removed the
/// only login their mysqlnd knows.
pub fn old_php_can_log_in(instance: &str) -> bool {
    native_password(instance) != NativePassword::Gone
}

/// The flags that turn `mysql_native_password` on where the line has it and make it the
/// default, so the root `--initialize-insecure` creates and every new user log in the old way.
fn native_password_flags(instance: &str) -> Vec<OsString> {
    let policy = arg("--authentication-policy=mysql_native_password,,");
    match native_password(instance) {
        NativePassword::Loaded => vec![policy],
        NativePassword::Off => vec![arg("--mysql-native-password=ON"), policy],
        NativePassword::Gone => vec![],
    }
}

/// The locale `postgres` and `initdb` run with. yardd runs from launchd with none, and
/// postgres on macOS then exits at startup ("postmaster became multithreaded during startup",
/// hint: set LC_ALL), as Postgres.app knows. Every macOS install has this one.
pub const POSTGRES_LOCALE: &str = "en_US.UTF-8";

fn postgres_env() -> Vec<(String, String)> {
    vec![("LC_ALL".to_string(), POSTGRES_LOCALE.to_string())]
}

/// The command that runs `instance` in the foreground (spec 5.1 to 5.4). Every program is a
/// binary in `keg`, which is brew's `opt/<formula>`; nothing is looked up on PATH.
pub fn command(engine: Engine, instance: &str, keg: &Path, paths: &Paths, ports: &[PortRef]) -> UnitCommand {
    let bin = |name: &str| keg.join("bin").join(name);
    let port = main_port(engine, instance, ports);
    match engine {
        Engine::Mysql => UnitCommand {
            program: bin("mysqld"),
            args: vec![arg(format!("--defaults-file={}", paths.service_conf(instance, "my.cnf").display()))],
            env: Vec::new(),
            stop_signal: SIGTERM,
        },
        Engine::Postgres => UnitCommand {
            program: bin("postgres"),
            args: vec![
                arg("-D"),
                paths.service_data(instance).into_os_string(),
                arg("-c"),
                arg("listen_addresses=127.0.0.1"),
                arg("-c"),
                arg(format!("port={port}")),
                arg("-c"),
                arg(format!("unix_socket_directories={}", paths.run_dir().display())),
            ],
            env: postgres_env(),
            // SIGINT is PostgreSQL's fast shutdown; SIGTERM is the smart one and waits for clients.
            stop_signal: SIGINT,
        },
        Engine::Redis => UnitCommand {
            program: bin("redis-server"),
            args: vec![paths.service_conf(instance, "redis.conf").into_os_string()],
            env: Vec::new(),
            stop_signal: SIGTERM,
        },
        Engine::Minio => UnitCommand {
            program: bin("minio"),
            args: vec![
                arg("server"),
                paths.service_data(instance).into_os_string(),
                arg("--address"),
                arg(format!("127.0.0.1:{port}")),
                arg("--console-address"),
                arg(format!("127.0.0.1:{}", console_port(engine, instance, ports))),
            ],
            env: vec![
                ("MINIO_ROOT_USER".to_string(), "minioadmin".to_string()),
                ("MINIO_ROOT_PASSWORD".to_string(), "minioadmin".to_string()),
            ],
            stop_signal: SIGTERM,
        },
    }
}

/// The one-time command that creates the data directory, for the engines that need one.
/// `macos_user` is PostgreSQL's superuser.
pub fn init_command(
    engine: Engine,
    instance: &str,
    keg: &Path,
    paths: &Paths,
    macos_user: &str,
) -> Option<UnitCommand> {
    let bin = |name: &str| keg.join("bin").join(name);
    let data = paths.service_data(instance);
    match engine {
        Engine::Mysql => Some(UnitCommand {
            program: bin("mysqld"),
            // `--no-defaults` first: mysqld reads `/etc/my.cnf` and `~/.my.cnf` unless told
            // not to, and a user's `datadir` or `port` there would shape what Yard creates.
            args: [
                vec![
                    arg("--no-defaults"),
                    arg("--initialize-insecure"),
                    arg(format!("--datadir={}", data.display())),
                    arg(format!("--basedir={}", keg.display())),
                ],
                native_password_flags(instance),
            ]
            .concat(),
            env: Vec::new(),
            stop_signal: SIGTERM,
        }),
        Engine::Postgres => Some(UnitCommand {
            program: bin("initdb"),
            args: vec![
                arg("-D"),
                data.into_os_string(),
                arg("-U"),
                arg(macos_user),
                arg("--auth=trust"),
                arg("-E"),
                arg("UTF8"),
            ],
            env: postgres_env(),
            stop_signal: SIGTERM,
        }),
        Engine::Redis | Engine::Minio => None,
    }
}

/// What `services/<i>/overrides.conf` starts as, for the engines that include it; Yard creates
/// it once with this and never writes it again. `None` for PostgreSQL, whose user file is
/// `postgresql.conf` in its data directory (spec 5.2). MySQL's needs a group header before any
/// setting, or mysqld exits at startup, so it starts with `[mysqld]`. Redis's include takes bare
/// lines. MinIO's is not MinIO's at all: nginx includes it in `minio.<tld>`'s block, so it says so.
pub fn user_overrides(engine: Engine) -> Option<&'static str> {
    match engine {
        Engine::Mysql => Some(
            "# USER-OWNED — Yard never rewrites this file. my.cnf includes it last, so a setting here wins.\n\
             # mysqld needs a group before any setting: put yours under [mysqld].\n\
             [mysqld]\n",
        ),
        Engine::Redis => Some(""),
        // The TLD can change and this file is never rewritten, so it names no host.
        Engine::Minio => Some(
            "# USER-OWNED — Yard never rewrites this file. nginx includes it in the server block for\n\
             # MinIO's console (minio.<tld>), so nginx directives go here. Yard tests them with\n\
             # nginx -t when you save, and keeps the last good config if they fail.\n",
        ),
        Engine::Postgres => None,
    }
}

/// The generated config file for `instance` and its text, for the engines that read one
/// (MySQL's `my.cnf`, Redis's `redis.conf`). The header names `overrides.conf`, which the
/// file includes last so the user's lines win.
pub fn config_file(
    engine: Engine,
    instance: &str,
    paths: &Paths,
    ports: &[PortRef],
) -> Option<(PathBuf, String)> {
    let overrides = paths.service_overrides(instance);
    let data = nginx_str(&paths.service_data(instance).display().to_string());
    let port = main_port(engine, instance, ports);
    match engine {
        Engine::Mysql => {
            let socket = nginx_str(&paths.run_dir().join(format!("{instance}.sock")).display().to_string());
            let text = format!(
                "{header}[mysqld]
port = {port}
bind-address = 127.0.0.1
socket = {socket}
datadir = {data}
mysqlx = OFF
{native}# PHP 7.0 knows no utf8mb4_0900 collation. Old apps' loose GROUP BY and zero dates work;
# Laravel sets its own sql_mode on each connection.
character-set-server = utf8mb4
collation-server = utf8mb4_unicode_ci
sql_mode = NO_ENGINE_SUBSTITUTION

!include {overrides}
",
                header = header(&overrides),
                native = match native_password(instance) {
                    NativePassword::Loaded => {
                        "# PHP 7.0 to 7.3 log in only with mysql_native_password, and 7.0 only when\n\
                         # the server offers it first.\n\
                         default_authentication_plugin = mysql_native_password\n\
                         authentication_policy = mysql_native_password,,\n"
                    }
                    NativePassword::Off => {
                        "# PHP 7.0 to 7.3 log in only with mysql_native_password.\n\
                         mysql_native_password = ON\n\
                         authentication_policy = mysql_native_password,,\n"
                    }
                    NativePassword::Gone => "",
                },
                overrides = overrides.display(),
            );
            Some((paths.service_conf(instance, "my.cnf"), text))
        }
        Engine::Redis => {
            let text = format!(
                "{header}bind 127.0.0.1
port {port}
dir {data}
daemonize no
include {overrides}
",
                header = header(&overrides),
                overrides = nginx_str(&overrides.display().to_string()),
            );
            Some((paths.service_conf(instance, "redis.conf"), text))
        }
        Engine::Postgres | Engine::Minio => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    const ROOT: &str = "/Users/me/Library/Application Support/Yard";
    const KEG: &str = "/opt/homebrew/opt/mysql@8.4";

    fn port(name: &str, port: u16) -> PortRef {
        PortRef { name: name.into(), port }
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn mysql_cnf() {
        let paths = Paths::new(ROOT);
        let (file, text) =
            config_file(Engine::Mysql, "mysql-8.4", &paths, &[port("mysql-8.4", 3306)]).unwrap();
        assert_eq!(file, paths.service_conf("mysql-8.4", "my.cnf"));
        assert_snapshot!(text);
    }

    /// The `[mysqld]` settings of `instance`'s `my.cnf`, before the user's `!include`.
    fn mysqld_settings(instance: &str) -> Vec<String> {
        let (_, text) = config_file(Engine::Mysql, instance, &Paths::new(ROOT), &[]).unwrap();
        text.lines().take_while(|l| !l.starts_with("!include")).map(str::to_string).collect()
    }

    #[test]
    fn mysql_84_lets_old_php_log_in() {
        let settings = mysqld_settings("mysql-8.4");
        assert!(settings.iter().any(|l| l == "mysql_native_password = ON"), "{settings:?}");
        assert!(
            settings.iter().any(|l| l == "authentication_policy = mysql_native_password,,"),
            "{settings:?}"
        );
    }

    #[test]
    fn mysql_80_has_native_password_without_turning_it_on() {
        let settings = mysqld_settings("mysql-8.0");
        assert!(!settings.iter().any(|l| l.starts_with("mysql_native_password")), "{settings:?}");
        assert!(
            settings.iter().any(|l| l == "authentication_policy = mysql_native_password,,"),
            "{settings:?}"
        );
    }

    /// 8.0 to 8.3 offer `default_authentication_plugin` first, and PHP 7.0 cannot switch away
    /// from caching_sha2 (seen 2026-11-01). 8.4 removed the variable, and mysqld refuses one it
    /// does not know.
    #[test]
    fn only_mysql_80_to_83_offer_native_password_first() {
        let line = "default_authentication_plugin = mysql_native_password";
        for instance in ["mysql-8.0", "mysql-8.3"] {
            assert!(mysqld_settings(instance).iter().any(|l| l == line), "{instance} lacks {line}");
        }
        for instance in ["mysql-8.4", "mysql-9.7"] {
            assert!(
                !mysqld_settings(instance).iter().any(|l| l.starts_with("default_authentication")),
                "{instance}"
            );
        }
    }

    #[test]
    fn mysql_9_has_no_native_password_to_turn_on() {
        let settings = mysqld_settings("mysql-9.7");
        assert!(!settings.iter().any(|l| l.contains("native_password")), "{settings:?}");
    }

    #[test]
    fn every_mysql_line_uses_a_charset_old_php_knows_and_a_lenient_sql_mode() {
        for instance in ["mysql-8.0", "mysql-8.4", "mysql-9.7"] {
            let settings = mysqld_settings(instance);
            for line in [
                "character-set-server = utf8mb4",
                "collation-server = utf8mb4_unicode_ci",
                "sql_mode = NO_ENGINE_SUBSTITUTION",
            ] {
                assert!(settings.iter().any(|l| l == line), "{instance} lacks {line}: {settings:?}");
            }
        }
    }

    #[test]
    fn mysql_init_creates_root_with_native_password_where_the_line_has_it() {
        let paths = Paths::new(ROOT);
        let keg = Path::new(KEG);
        let args = |instance: &str| init_command(Engine::Mysql, instance, keg, &paths, "me").unwrap().args;
        let policy = OsString::from("--authentication-policy=mysql_native_password,,");
        let on = OsString::from("--mysql-native-password=ON");
        let a84 = args("mysql-8.4");
        assert!(a84.contains(&policy) && a84.contains(&on), "{a84:?}");
        let a80 = args("mysql-8.0");
        assert!(a80.contains(&policy) && !a80.contains(&on), "{a80:?}");
        let a97 = args("mysql-9.7");
        assert!(!a97.iter().any(|a| a.to_string_lossy().contains("native")), "{a97:?}");
        // `--no-defaults` must stay first, or mysqld reads the user's own my.cnf.
        assert_eq!(a84[0], "--no-defaults");
    }

    #[test]
    fn redis_conf() {
        let paths = Paths::new(ROOT);
        let (file, text) = config_file(Engine::Redis, "redis", &paths, &[port("redis", 6380)]).unwrap();
        assert_eq!(file, paths.service_conf("redis", "redis.conf"));
        assert_snapshot!(text);
    }

    #[test]
    fn postgres_and_minio_have_no_config_file() {
        let paths = Paths::new(ROOT);
        assert_eq!(config_file(Engine::Postgres, "postgres-17", &paths, &[]), None);
        assert_eq!(config_file(Engine::Minio, "minio", &paths, &[]), None);
    }

    #[test]
    fn the_config_file_follows_the_registry_port() {
        let paths = Paths::new(ROOT);
        let (_, text) = config_file(Engine::Mysql, "mysql-8.4", &paths, &[port("mysql-8.4", 3399)]).unwrap();
        assert!(text.lines().any(|l| l == "port = 3399"), "{text}");
    }

    #[test]
    fn commands_per_engine() {
        let paths = Paths::new(ROOT);
        let keg = Path::new(KEG);
        let conf = paths.service_conf("mysql-8.4", "my.cnf");

        let mysql = command(Engine::Mysql, "mysql-8.4", keg, &paths, &[port("mysql-8.4", 3306)]);
        assert_eq!(mysql.program, keg.join("bin/mysqld"));
        assert_eq!(mysql.args, [OsString::from(format!("--defaults-file={}", conf.display()))]);
        assert!(mysql.env.is_empty());
        assert_eq!(mysql.stop_signal, SIGTERM);

        let keg = Path::new("/opt/homebrew/opt/postgresql@17");
        let pg = command(Engine::Postgres, "postgres-17", keg, &paths, &[port("postgres-17", 5433)]);
        assert_eq!(pg.program, keg.join("bin/postgres"));
        assert_eq!(
            pg.args,
            os(&[
                "-D",
                &paths.service_data("postgres-17").display().to_string(),
                "-c",
                "listen_addresses=127.0.0.1",
                "-c",
                "port=5433",
                "-c",
                &format!("unix_socket_directories={}", paths.run_dir().display()),
            ])
        );
        assert_eq!(pg.stop_signal, SIGINT);
        assert_eq!(SIGINT, 2);

        let keg = Path::new("/opt/homebrew/opt/redis");
        let redis = command(Engine::Redis, "redis", keg, &paths, &[port("redis", 6379)]);
        assert_eq!(redis.program, keg.join("bin/redis-server"));
        assert_eq!(redis.args, [paths.service_conf("redis", "redis.conf").into_os_string()]);
        assert_eq!(redis.stop_signal, SIGTERM);
        assert_eq!(SIGTERM, 15);

        let keg = Path::new("/opt/homebrew/opt/minio");
        // The console entry comes first: each port is found by name, not by position.
        let ports = [port("minio-console", 9101), port("minio", 9100)];
        let minio = command(Engine::Minio, "minio", keg, &paths, &ports);
        assert_eq!(minio.program, keg.join("bin/minio"));
        assert_eq!(
            minio.args,
            os(&[
                "server",
                &paths.service_data("minio").display().to_string(),
                "--address",
                "127.0.0.1:9100",
                "--console-address",
                "127.0.0.1:9101",
            ])
        );
        assert!(minio.env.contains(&("MINIO_ROOT_USER".into(), "minioadmin".into())));
        assert!(minio.env.contains(&("MINIO_ROOT_PASSWORD".into(), "minioadmin".into())));
        assert_eq!(minio.stop_signal, SIGTERM);
    }

    #[test]
    fn init_commands() {
        let paths = Paths::new(ROOT);
        let keg = Path::new(KEG);
        let data = paths.service_data("mysql-8.4");
        let mysql = init_command(Engine::Mysql, "mysql-8.4", keg, &paths, "me").unwrap();
        assert_eq!(mysql.program, keg.join("bin/mysqld"));
        assert_eq!(
            mysql.args,
            os(&[
                "--no-defaults",
                "--initialize-insecure",
                &format!("--datadir={}", data.display()),
                &format!("--basedir={}", keg.display()),
                "--mysql-native-password=ON",
                "--authentication-policy=mysql_native_password,,",
            ])
        );

        let keg = Path::new("/opt/homebrew/opt/postgresql@17");
        let data = paths.service_data("postgres-17");
        let pg = init_command(Engine::Postgres, "postgres-17", keg, &paths, "me").unwrap();
        assert_eq!(pg.program, keg.join("bin/initdb"));
        assert_eq!(
            pg.args,
            os(&["-D", &data.display().to_string(), "-U", "me", "--auth=trust", "-E", "UTF8"])
        );

        assert_eq!(init_command(Engine::Redis, "redis", keg, &paths, "me"), None);
        assert_eq!(init_command(Engine::Minio, "minio", keg, &paths, "me"), None);
    }

    #[test]
    fn postgres_gets_a_locale_from_yard() {
        // yardd runs from launchd with no locale, and postgres on macOS then exits with
        // "postmaster became multithreaded during startup" (seen on 2026-10-24).
        let paths = Paths::new(ROOT);
        let keg = Path::new("/opt/homebrew/opt/postgresql@17");
        let locale = ("LC_ALL".to_string(), POSTGRES_LOCALE.to_string());
        let run = command(Engine::Postgres, "postgres-17", keg, &paths, &[port("postgres-17", 5433)]);
        let init = init_command(Engine::Postgres, "postgres-17", keg, &paths, "me").unwrap();
        assert_eq!(run.env, vec![locale.clone()]);
        assert_eq!(init.env, vec![locale]);
        assert_eq!(POSTGRES_LOCALE, "en_US.UTF-8");
    }

    #[test]
    fn each_engine_starts_its_overrides_as_its_config_reads_it() {
        let mysql = user_overrides(Engine::Mysql).unwrap();
        assert!(mysql.starts_with("# USER-OWNED"), "{mysql}");
        // A setting before any group makes mysqld exit ("Found option without preceding group").
        assert_eq!(mysql.lines().find(|l| !l.starts_with('#')), Some("[mysqld]"));
        assert_eq!(user_overrides(Engine::Redis), Some(""));
        assert_eq!(user_overrides(Engine::Postgres), None, "PostgreSQL's file is postgresql.conf");
        // nginx includes MinIO's: comments only, and it names the host.
        let minio = user_overrides(Engine::Minio).unwrap();
        assert!(minio.lines().all(|l| l.starts_with('#')) && minio.contains("nginx -t"), "{minio}");
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let paths = Paths::new("/tmp/a b");
        let cases = [
            (Engine::Mysql, "mysql-8.4", port("mysql-8.4", 3306)),
            (Engine::Redis, "redis", port("redis", 6379)),
        ];
        for (engine, instance, p) in cases {
            let (_, text) = config_file(engine, instance, &paths, &[p]).unwrap();
            assert!(text.starts_with("# Generated by Yard — edit /tmp/a b/services/"), "{text}");
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                if let Some(at) = line.find("a b") {
                    // mysqld's `!include` takes the rest of the line and keeps a quote as
                    // part of the path (checked against mysqld 8.0), so it cannot be quoted.
                    if line.starts_with("!include ") {
                        assert_eq!(line, "!include /tmp/a b/services/mysql-8.4/overrides.conf");
                    } else {
                        assert!(line[..at].matches('"').count() % 2 == 1, "unquoted path: {line}");
                    }
                }
            }
        }
    }
}
