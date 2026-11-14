//! The flags `yard db` passes through to a tool and the ones it refuses (db spec §7).

use yard_proto::{DbAction, ErrorCode, YardError};

/// A database tool `yard db` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Mysql,
    Mysqldump,
    Psql,
    PgDump,
    PgRestore,
}

impl Tool {
    pub fn program(self) -> &'static str {
        match self {
            Tool::Mysql => "mysql",
            Tool::Mysqldump => "mysqldump",
            Tool::Psql => "psql",
            Tool::PgDump => "pg_dump",
            Tool::PgRestore => "pg_restore",
        }
    }

    fn is_mysql(self) -> bool {
        matches!(self, Tool::Mysql | Tool::Mysqldump)
    }
}

/// `pg_dump`'s output format (`-F`); MySQL's dumps are always `Plain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpFormat {
    Plain,
    Custom,
    Tar,
    Directory,
}

const SERVER: &str = "yard db only talks to its own instance";
const DATABASE: &str = "the database is the one you name";
const OUTPUT: &str = "yard db writes the file";
const RUNS: &str = "the file is what runs";

/// A refused flag: its short letter (if any), its long name, and why.
struct Reserved(Option<char>, &'static str, &'static str);

fn reserved(tool: Tool, action: DbAction) -> Vec<Reserved> {
    let mut r = Vec::new();
    if tool.is_mysql() {
        r.extend([
            Reserved(Some('h'), "host", SERVER),
            Reserved(Some('P'), "port", SERVER),
            Reserved(Some('u'), "user", SERVER),
            Reserved(Some('p'), "password", SERVER),
            Reserved(Some('S'), "socket", SERVER),
            Reserved(None, "protocol", SERVER),
            Reserved(None, "defaults-file", SERVER),
            Reserved(None, "defaults-extra-file", SERVER),
            Reserved(None, "defaults-group-suffix", SERVER),
            Reserved(None, "login-path", SERVER),
            Reserved(None, "no-defaults", SERVER),
            Reserved(None, "print-defaults", SERVER),
            Reserved(Some('D'), "database", DATABASE),
        ]);
    } else {
        r.extend([
            Reserved(Some('h'), "host", SERVER),
            Reserved(Some('p'), "port", SERVER),
            Reserved(Some('U'), "username", SERVER),
            Reserved(Some('W'), "password", SERVER),
            Reserved(Some('d'), "dbname", DATABASE),
            Reserved(Some('l'), "list", SERVER),
            Reserved(Some('D'), "pgdata", SERVER),
        ]);
    }
    match (tool, action) {
        (Tool::Mysqldump, _) => r.extend([
            Reserved(Some('B'), "databases", DATABASE),
            Reserved(Some('A'), "all-databases", DATABASE),
            Reserved(Some('r'), "result-file", OUTPUT),
        ]),
        (Tool::Mysql, DbAction::Import) => {
            r.extend([Reserved(Some('e'), "execute", RUNS), Reserved(None, "init-command", RUNS)])
        }
        (Tool::PgDump, _) => r.push(Reserved(Some('f'), "file", OUTPUT)),
        (Tool::PgRestore, _) => {
            r.extend([Reserved(Some('C'), "create", DATABASE), Reserved(Some('f'), "file", OUTPUT)])
        }
        (Tool::Psql, DbAction::Import) => {
            r.extend([Reserved(Some('c'), "command", RUNS), Reserved(Some('f'), "file", RUNS)])
        }
        _ => {}
    }
    r
}

/// A long option's name as the tool reads it: before `=`; MySQL's also with `_` read as `-`
/// and a `loose-` prefix dropped, as its option parser does.
fn long_name(tool: Tool, arg: &str) -> Option<String> {
    let name = arg.strip_prefix("--")?.split('=').next()?;
    if !tool.is_mysql() {
        return Some(name.to_string());
    }
    let name = name.replace('_', "-");
    Some(name.strip_prefix("loose-").map(str::to_string).unwrap_or(name))
}

/// Refuses each passed-through flag that would point the tool at another server, database or
/// file (db spec §7). Both MySQL's option parser and getopt_long take a unique prefix of a
/// long option, so any prefix of three or more characters of a refused name is refused. A
/// short flag is read by its first letter only: `-Fd` is `-F` with `d`, not `-d`.
pub fn check(tool: Tool, action: DbAction, args: &[String]) -> Result<(), YardError> {
    let reserved = reserved(tool, action);
    for arg in args {
        let hit = if let Some(name) = long_name(tool, arg) {
            reserved.iter().find(|r| r.1 == name || (name.len() >= 3 && r.1.starts_with(name.as_str())))
        } else if let Some(rest) = arg.strip_prefix('-') {
            let letter = rest.chars().next();
            reserved.iter().find(|r| r.0.is_some() && r.0 == letter)
        } else {
            None
        };
        if let Some(Reserved(_, _, why)) = hit {
            let flag = arg.split('=').next().unwrap_or(arg);
            let flag = if flag.starts_with("--") { flag } else { &flag[..flag.len().min(2)] };
            return Err(YardError::new(ErrorCode::DbFlagReserved, format!("{flag}: {why}"))
                .with_hint(format!("leave {flag} out; yard db sets the server, the database and the file")));
        }
    }
    Ok(())
}

/// `pg_dump`'s format from `-F`/`--format` among the passed-through flags.
pub fn dump_format(args: &[String]) -> DumpFormat {
    let mut format = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = if let Some(v) = arg.strip_prefix("--format=") {
            Some(v.to_string())
        } else if arg == "--format" || arg == "-F" {
            iter.next().cloned()
        } else {
            arg.strip_prefix("-F").map(str::to_string)
        };
        if let Some(v) = value {
            format = Some(match v.as_str() {
                "c" | "custom" => DumpFormat::Custom,
                "t" | "tar" => DumpFormat::Tar,
                "d" | "directory" => DumpFormat::Directory,
                _ => DumpFormat::Plain,
            });
        }
    }
    format.unwrap_or(DumpFormat::Plain)
}

/// Plain SQL is gzipped one way only (db spec §6.5): Yard's `--compress`, never `pg_dump -Z`;
/// and an archive format, which compresses itself, takes `-Z` and never Yard's.
pub fn check_compress(tool: Tool, compress: bool, args: &[String]) -> Result<(), YardError> {
    if tool != Tool::PgDump {
        return Ok(());
    }
    let plain = dump_format(args) == DumpFormat::Plain;
    let z = args.iter().find(|a| {
        a.starts_with("-Z")
            || long_name(tool, a).is_some_and(|n| n.len() >= 3 && "compress".starts_with(n.as_str()))
    });
    if let (true, Some(flag)) = (plain, z) {
        let flag = flag.split('=').next().unwrap_or(flag);
        return Err(YardError::new(
            ErrorCode::DbFlagReserved,
            format!("{flag}: plain SQL is gzipped by yard db dump --compress"),
        )
        .with_hint("--compress"));
    }
    if compress && !plain {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            "this pg_dump format compresses itself, so --compress does not apply",
        )
        .with_hint("-- -Z <level>"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::{DbAction, ErrorCode};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn refused(tool: Tool, action: DbAction, a: &[&str]) -> bool {
        match check(tool, action, &args(a)) {
            Err(e) => {
                assert_eq!(e.code, ErrorCode::DbFlagReserved);
                let flag = a[0].split('=').next().unwrap();
                let flag = if flag.starts_with("--") { flag } else { &flag[..2] };
                assert!(e.message.starts_with(&format!("{flag}: ")), "{}", e.message);
                true
            }
            Ok(()) => false,
        }
    }

    #[test]
    fn every_refused_flag_in_every_form() {
        use DbAction::*;
        use Tool::*;
        let mysql_all = [
            "host",
            "port",
            "user",
            "password",
            "socket",
            "protocol",
            "defaults-file",
            "defaults-extra-file",
            "defaults-group-suffix",
            "login-path",
            "no-defaults",
            "print-defaults",
            "database",
        ];
        for tool in [Mysql, Mysqldump] {
            for long in mysql_all {
                assert!(refused(tool, Dump, &[&format!("--{long}")]), "{long}");
                assert!(refused(tool, Dump, &[&format!("--{long}=x")]), "{long}=");
            }
            for short in ["-h", "-P", "-u", "-p", "-S", "-D"] {
                assert!(refused(tool, Import, &[short, "x"]), "{short}");
                assert!(refused(tool, Import, &[&format!("{short}x")]), "{short}x");
            }
        }
        for flag in ["--databases", "-B", "--all-databases", "-A", "--result-file=x", "-r", "-rx"] {
            assert!(refused(Mysqldump, Dump, &[flag]), "{flag}");
        }
        for flag in ["-e", "--execute=x", "--init-command=x"] {
            assert!(refused(Mysql, Import, &[flag]), "{flag}");
        }
        for tool in [Psql, PgDump, PgRestore] {
            for flag in [
                "-h",
                "--host=x",
                "-p5432",
                "--port",
                "-U",
                "--username=x",
                "-W",
                "--password",
                "-d",
                "--dbname=x",
                "-l",
                "--list",
                "-D",
                "--pgdata=x",
            ] {
                assert!(refused(tool, Import, &[flag]), "{tool:?} {flag}");
            }
        }
        for flag in ["-f", "-fx", "--file=x"] {
            assert!(refused(PgDump, Dump, &[flag]), "{flag}");
            assert!(refused(PgRestore, Import, &[flag]), "{flag}");
            assert!(refused(Psql, Import, &[flag]), "{flag}");
        }
        for flag in ["-C", "--create"] {
            assert!(refused(PgRestore, Import, &[flag]), "{flag}");
        }
        for flag in ["-c", "--command=x"] {
            assert!(refused(Psql, Import, &[flag]), "{flag}");
        }
    }

    #[test]
    fn mysql_prefixes_and_spellings_are_refused() {
        for flag in ["--hos=x", "--sock=/t", "--databa=x", "--defaults_file=x", "--loose-host=x", "--pass"] {
            assert!(refused(Tool::Mysqldump, DbAction::Dump, &[flag]), "{flag}");
        }
        for flag in [
            "--hostname-file",
            "--compress",
            "--routines",
            "--single-transaction",
            "--where=id>1",
            "--no-data",
        ] {
            assert!(!refused(Tool::Mysqldump, DbAction::Dump, &[flag]), "{flag}");
        }
    }

    #[test]
    fn postgres_prefixes_are_refused_too() {
        assert!(refused(Tool::PgDump, DbAction::Dump, &["--hos=x"]));
        assert!(!refused(Tool::PgDump, DbAction::Dump, &["--filter=f.txt"]));
        assert!(!refused(Tool::PgDump, DbAction::Dump, &["--data-only"]));
    }

    #[test]
    fn shell_may_execute() {
        assert!(!refused(Tool::Mysql, DbAction::Shell, &["-e", "select 1"]));
        assert!(refused(Tool::Mysql, DbAction::Import, &["-e", "select 1"]));
        assert!(!refused(Tool::Psql, DbAction::Shell, &["-c", "select 1"]));
        assert!(refused(Tool::Psql, DbAction::Import, &["-c", "select 1"]));
    }

    #[test]
    fn pg_restore_create_refused_and_w_allowed() {
        assert!(refused(Tool::PgRestore, DbAction::Import, &["--create"]));
        for flag in ["-w", "--no-password", "-O", "--no-owner", "-j", "4", "-v", "ON_ERROR_STOP=0"] {
            assert!(!refused(Tool::PgRestore, DbAction::Import, &[flag]), "{flag}");
        }
    }

    #[test]
    fn values_are_not_read_as_flags() {
        // A value that looks like a flag belongs to the flag before it.
        assert!(check(Tool::Mysqldump, DbAction::Dump, &args(&["--where", "x"])).is_ok());
    }

    #[test]
    fn dump_format_reads_every_spelling() {
        for (a, f) in [
            (&["-Fc"][..], DumpFormat::Custom),
            (&["-F", "c"], DumpFormat::Custom),
            (&["--format=custom"], DumpFormat::Custom),
            (&["--format", "tar"], DumpFormat::Tar),
            (&["-Fd"], DumpFormat::Directory),
            (&["-Fp"], DumpFormat::Plain),
            (&["--schema-only"], DumpFormat::Plain),
            (&[], DumpFormat::Plain),
        ] {
            assert_eq!(dump_format(&args(a)), f, "{a:?}");
        }
    }

    #[test]
    fn compress_conflicts() {
        let e = check_compress(Tool::PgDump, false, &args(&["-Z", "6"])).unwrap_err();
        assert_eq!((e.code, e.hint.as_deref()), (ErrorCode::DbFlagReserved, Some("--compress")));
        assert!(check_compress(Tool::PgDump, false, &args(&["--compress=gzip:6"])).is_err());
        let e = check_compress(Tool::PgDump, true, &args(&["-Fc"])).unwrap_err();
        assert_eq!((e.code, e.hint.as_deref()), (ErrorCode::InvalidArgument, Some("-- -Z <level>")));
        assert!(check_compress(Tool::PgDump, false, &args(&["-Fc", "-Z", "9"])).is_ok());
        assert!(check_compress(Tool::Mysqldump, true, &args(&["--compress"])).is_ok());
        assert!(check_compress(Tool::PgDump, true, &args(&[])).is_ok());
    }
}
