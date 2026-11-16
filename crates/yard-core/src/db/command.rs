//! The command `db.command` answers: the tool, Yard's arguments, then the user's (db spec §7, §11).

use super::flags::Tool;
use crate::services::engines::DbFlavor;
use std::path::Path;
use yard_proto::{DbAction, DbCommand, ImportKind, StreamFilter};

/// The PostgreSQL variables that could point a tool at another server or login; cleared for
/// Yard's dumps and imports (db spec §7).
/// The MySQL client's variables that could point it at another server; cleared for every run.
pub const MYSQL_ENV: [&str; 3] = ["MYSQL_HOST", "MYSQL_TCP_PORT", "MYSQL_UNIX_PORT"];

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
    pub engine: DbFlavor,
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
        DbFlavor::Postgres => {
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
                    // ON_ERROR_STOP=1 goes after the user's flags (below), so it always wins:
                    // an import that carries on past an error could be read wrongly.
                    let a = [
                        vec!["--no-psqlrc".to_string()],
                        conn,
                        db(name.unwrap_or_default()),
                        vec!["-q".into()],
                    ]
                    .concat();
                    (Tool::Psql, a, StreamFilter::Postgres { keep_owners })
                }
            }
        }
        _ => {
            // --protocol=socket wins over a host in MYSQL_HOST or a ~/.my.cnf [client] group.
            let conn = vec![
                "--protocol=socket".to_string(),
                format!("--socket={socket}"),
                format!("--user={}", t.user),
            ];
            env_remove = MYSQL_ENV.iter().map(|v| v.to_string()).collect();
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
                    // --binary-mode turns off the client's own commands (`\!`, `\u`, `source`)
                    // in piped input, all but `delimiter` and `charset`.
                    let db =
                        vec![format!("--database={}", name.unwrap_or_default()), "--binary-mode".to_string()];
                    (Tool::Mysql, [no_defaults, conn, db].concat(), StreamFilter::Mysql)
                }
            }
        }
    };
    argv.extend(args.iter().cloned());
    if matches!(filter, StreamFilter::Postgres { .. }) {
        argv.extend(["-v".to_string(), "ON_ERROR_STOP=1".to_string()]);
    }
    // pg_restore reads an archive by path (so `-j` works) or a directory; the client sends the
    // path when the archive is not gzipped.
    if let Some((ImportKind::Directory | ImportKind::Archive, Some(path))) = import {
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
    use std::path::Path;
    use yard_proto::{DbAction, ImportKind, StreamFilter};

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn my() -> Target<'static> {
        Target {
            engine: DbFlavor::Mysql,
            instance: "mysql-8.4",
            bin_dir: Path::new("/k/bin"),
            socket: Path::new("/r/run/mysql-8.4.sock"),
            port: 3307,
            user: "root",
        }
    }

    fn pg() -> Target<'static> {
        Target {
            engine: DbFlavor::Postgres,
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
            s(&[
                "--protocol=socket",
                "--socket=/r/run/mysql-8.4.sock",
                "--user=root",
                "--database=shop",
                "-e",
                "show tables"
            ])
        );
        assert_eq!((c.filter, c.env_remove.len()), (StreamFilter::None, 3));
        let c = build(&my(), DbAction::Shell, None, &[], None, false);
        assert_eq!(c.args, s(&["--protocol=socket", "--socket=/r/run/mysql-8.4.sock", "--user=root"]));

        let c = build(&my(), DbAction::Dump, Some("shop"), &s(&["--routines", "orders"]), None, false);
        assert_eq!(c.program, "/k/bin/mysqldump");
        assert_eq!(
            c.args,
            s(&[
                "--no-defaults",
                "--protocol=socket",
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
                "--protocol=socket",
                "--socket=/r/run/mysql-8.4.sock",
                "--user=root",
                "--database=shop",
                "--binary-mode",
                "--force"
            ])
        );
        assert_eq!(c.filter, StreamFilter::Mysql);
        assert_eq!((c.instance.as_str(), c.created, c.notes.len()), ("mysql-8.4", false, 0));
    }

    #[test]
    fn mysql_always_uses_the_socket() {
        for action in [DbAction::Shell, DbAction::Dump, DbAction::Import] {
            let c = build(&my(), action, Some("shop"), &[], Some((ImportKind::Plain, None)), false);
            assert!(c.args.contains(&"--protocol=socket".to_string()), "{action:?}: {:?}", c.args);
            for var in ["MYSQL_HOST", "MYSQL_TCP_PORT", "MYSQL_UNIX_PORT"] {
                assert!(c.env_remove.contains(&var.to_string()), "{action:?} keeps {var}");
            }
        }
    }

    #[test]
    fn psql_imports_always_stop_on_error() {
        let c = build(
            &pg(),
            DbAction::Import,
            Some("app"),
            &s(&["-v", "ON_ERROR_STOP=0"]),
            Some((ImportKind::Plain, None)),
            false,
        );
        assert_eq!(
            c.args[c.args.len() - 2..],
            s(&["-v", "ON_ERROR_STOP=1"]),
            "Yard's comes last, so it wins"
        );
    }

    #[test]
    fn an_archive_is_read_by_path() {
        let c = build(
            &pg(),
            DbAction::Import,
            Some("app"),
            &s(&["-j", "8"]),
            Some((ImportKind::Archive, Some("/d/app.dump"))),
            false,
        );
        assert_eq!(c.args.last().map(String::as_str), Some("/d/app.dump"));
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
                &["-d", "app", "-q", "-v", "ON_ERROR_STOP=0", "-v", "ON_ERROR_STOP=1"]
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
