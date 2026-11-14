//! The command `db.command` answers: the tool, Yard's arguments, then the user's (db spec §7, §11).

use super::flags::Tool;
use crate::services::Engine;
use std::path::Path;
use yard_proto::{DbAction, DbCommand, ImportKind, StreamFilter};

/// The PostgreSQL variables that could point a tool at another server or login; cleared for
/// Yard's dumps and imports (db spec §7).
pub const PG_ENV: [&str; 9] = [
    "PGHOST",
    "PGHOSTADDR",
    "PGPORT",
    "PGUSER",
    "PGDATABASE",
    "PGPASSWORD",
    "PGPASSFILE",
    "PGSERVICE",
    "PGSERVICEFILE",
];

/// The instance a command runs against.
pub struct Target<'a> {
    pub engine: Engine,
    pub instance: &'a str,
    /// The keg's `bin` directory the tools come from (db spec §7.1).
    pub bin_dir: &'a Path,
    /// MySQL's socket file, or PostgreSQL's socket directory.
    pub socket: &'a Path,
    pub port: u16,
    pub user: &'a str,
}

/// The command for `action`. Yard's arguments come first and the user's `args` after, so the
/// user's wins where a tool takes the last of a flag. MySQL's tools read options after the
/// database name; PostgreSQL's (macOS getopt) do not, so they take the database as `-d` and
/// only a directory-format path follows the user's flags.
pub fn build(
    t: &Target,
    action: DbAction,
    name: Option<&str>,
    args: &[String],
    import: Option<(ImportKind, Option<&str>)>,
    keep_owners: bool,
) -> DbCommand {
    let socket = t.socket.display().to_string();
    let mut env_remove = vec![];
    let (tool, mut argv, filter): (Tool, Vec<String>, StreamFilter) = match t.engine {
        Engine::Postgres => {
            let conn = vec!["-h".into(), socket, "-p".into(), t.port.to_string(), "-U".into(), t.user.into()];
            if action != DbAction::Shell {
                env_remove = PG_ENV.iter().map(|v| v.to_string()).collect();
            }
            let db = |n: &str| vec!["-d".to_string(), n.to_string()];
            match (action, import) {
                (DbAction::Shell, _) => {
                    (Tool::Psql, [conn, db(name.unwrap_or("postgres"))].concat(), StreamFilter::None)
                }
                (DbAction::Dump, _) => {
                    (Tool::PgDump, [conn, db(name.unwrap_or_default())].concat(), StreamFilter::None)
                }
                (DbAction::Import, Some((ImportKind::Archive | ImportKind::Directory, _))) => {
                    let mut a = [conn, db(name.unwrap_or_default())].concat();
                    if !keep_owners {
                        a.extend(["--no-owner".to_string(), "--no-privileges".to_string()]);
                    }
                    (Tool::PgRestore, a, StreamFilter::None)
                }
                (DbAction::Import, _) => {
                    let a = [
                        vec!["--no-psqlrc".to_string()],
                        conn,
                        db(name.unwrap_or_default()),
                        vec!["-v".into(), "ON_ERROR_STOP=1".into(), "-q".into()],
                    ]
                    .concat();
                    (Tool::Psql, a, StreamFilter::Postgres { keep_owners })
                }
            }
        }
        _ => {
            let conn = vec![format!("--socket={socket}"), format!("--user={}", t.user)];
            let no_defaults = vec!["--no-defaults".to_string()];
            match action {
                DbAction::Shell => {
                    let db = name.map(|n| vec![format!("--database={n}")]).unwrap_or_default();
                    (Tool::Mysql, [conn, db].concat(), StreamFilter::None)
                }
                DbAction::Dump => (
                    Tool::Mysqldump,
                    [no_defaults, conn, vec![name.unwrap_or_default().to_string()]].concat(),
                    StreamFilter::None,
                ),
                DbAction::Import => {
                    let db = vec![format!("--database={}", name.unwrap_or_default())];
                    (Tool::Mysql, [no_defaults, conn, db].concat(), StreamFilter::Mysql)
                }
            }
        }
    };
    argv.extend(args.iter().cloned());
    if let Some((ImportKind::Directory, Some(path))) = import {
        argv.push(path.to_string());
    }
    DbCommand {
        instance: t.instance.to_string(),
        program: t.bin_dir.join(tool.program()).display().to_string(),
        args: argv,
        env: vec![],
        env_remove,
        filter,
        created: false,
        notes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::Engine;
    use std::path::Path;
    use yard_proto::{DbAction, ImportKind, StreamFilter};

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn my() -> Target<'static> {
        Target {
            engine: Engine::Mysql,
            instance: "mysql-8.4",
            bin_dir: Path::new("/k/bin"),
            socket: Path::new("/r/run/mysql-8.4.sock"),
            port: 3307,
            user: "root",
        }
    }

    fn pg() -> Target<'static> {
        Target {
            engine: Engine::Postgres,
            instance: "postgres-17",
            bin_dir: Path::new("/k/bin"),
            socket: Path::new("/r/run"),
            port: 5433,
            user: "me",
        }
    }

    #[test]
    fn mysql_commands() {
        let c = build(&my(), DbAction::Shell, Some("shop"), &s(&["-e", "show tables"]), None, false);
        assert_eq!(c.program, "/k/bin/mysql");
        assert_eq!(
            c.args,
            s(&["--socket=/r/run/mysql-8.4.sock", "--user=root", "--database=shop", "-e", "show tables"])
        );
        assert_eq!((c.filter, c.env_remove.len()), (StreamFilter::None, 0));
        let c = build(&my(), DbAction::Shell, None, &[], None, false);
        assert_eq!(c.args, s(&["--socket=/r/run/mysql-8.4.sock", "--user=root"]));

        let c = build(&my(), DbAction::Dump, Some("shop"), &s(&["--routines", "orders"]), None, false);
        assert_eq!(c.program, "/k/bin/mysqldump");
        assert_eq!(
            c.args,
            s(&[
                "--no-defaults",
                "--socket=/r/run/mysql-8.4.sock",
                "--user=root",
                "shop",
                "--routines",
                "orders"
            ])
        );

        let c = build(
            &my(),
            DbAction::Import,
            Some("shop"),
            &s(&["--force"]),
            Some((ImportKind::Plain, None)),
            false,
        );
        assert_eq!(c.program, "/k/bin/mysql");
        assert_eq!(
            c.args,
            s(&[
                "--no-defaults",
                "--socket=/r/run/mysql-8.4.sock",
                "--user=root",
                "--database=shop",
                "--force"
            ])
        );
        assert_eq!(c.filter, StreamFilter::Mysql);
        assert_eq!((c.instance.as_str(), c.created, c.notes.len()), ("mysql-8.4", false, 0));
    }

    #[test]
    fn postgres_commands() {
        let conn = ["-h", "/r/run", "-p", "5433", "-U", "me"];
        let c = build(&pg(), DbAction::Shell, None, &[], None, false);
        assert_eq!(c.program, "/k/bin/psql");
        assert_eq!(c.args, [&conn[..], &["-d", "postgres"]].concat());
        assert!(c.env_remove.is_empty());

        let c = build(&pg(), DbAction::Dump, Some("app"), &s(&["-Fc"]), None, false);
        assert_eq!(c.program, "/k/bin/pg_dump");
        assert_eq!(c.args, s(&[&conn[..], &["-d", "app", "-Fc"]].concat()));
        assert!(
            c.env_remove.contains(&"PGHOST".to_string()) && c.env_remove.contains(&"PGPASSFILE".to_string())
        );

        let c = build(
            &pg(),
            DbAction::Import,
            Some("app"),
            &s(&["-v", "ON_ERROR_STOP=0"]),
            Some((ImportKind::Plain, None)),
            false,
        );
        assert_eq!(c.program, "/k/bin/psql");
        assert_eq!(
            c.args,
            s(&[
                &["--no-psqlrc"][..],
                &conn,
                &["-d", "app", "-v", "ON_ERROR_STOP=1", "-q", "-v", "ON_ERROR_STOP=0"]
            ]
            .concat())
        );
        assert_eq!(c.filter, StreamFilter::Postgres { keep_owners: false });

        let c = build(
            &pg(),
            DbAction::Import,
            Some("app"),
            &s(&["-j", "4"]),
            Some((ImportKind::Archive, None)),
            false,
        );
        assert_eq!(c.program, "/k/bin/pg_restore");
        assert_eq!(
            c.args,
            s(&[&conn[..], &["-d", "app", "--no-owner", "--no-privileges", "-j", "4"]].concat())
        );
        assert_eq!(c.filter, StreamFilter::None);

        let c = build(&pg(), DbAction::Import, Some("app"), &[], Some((ImportKind::Archive, None)), true);
        assert!(!c.args.contains(&"--no-owner".to_string()));
        let c = build(&pg(), DbAction::Import, Some("app"), &[], Some((ImportKind::Plain, None)), true);
        assert_eq!(c.filter, StreamFilter::Postgres { keep_owners: true });

        let c = build(
            &pg(),
            DbAction::Import,
            Some("app"),
            &s(&["-j", "4"]),
            Some((ImportKind::Directory, Some("/d/app.dir"))),
            false,
        );
        assert_eq!(c.args.last().map(String::as_str), Some("/d/app.dir"));
        assert_eq!(c.args[c.args.len() - 3..], s(&["-j", "4", "/d/app.dir"]));
    }
}
