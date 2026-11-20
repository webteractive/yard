//! What the Services screen's database controls decide (db spec §8, §10): which instances have
//! them, the `yard <engine> dump` and `import` argument vectors the app runs, the Connect… sheet's
//! fields and state line, and byte sizes in words.

use crate::shell_words::split_words;
pub use yard_core::present::bytes_text;
use yard_core::services::Engines;
use yard_core::services::engines::{DbFlavor, db_flavor};
use yard_proto::{ConnectInfo, DbDumped, DbImported, ErrorCode, ServiceState, YardError};

/// Whether an instance of `engine` has Databases, Create database…, Import… and Dump….
pub fn has_databases(engine: &str) -> bool {
    db_flavor(engine).is_some()
}

/// Whether an instance of `engine` has Connect… (MySQL, PostgreSQL, Redis and any engine whose
/// recipe says how to connect): what yardd's `engines.list` says, else what this build carries.
pub fn has_connect(engine: &str, list: Option<&yard_proto::EnginesListResult>) -> bool {
    match list {
        Some(list) => list.engines.iter().any(|e| e.key == engine && e.connect),
        None => Engines::builtin_shared().get(engine).is_some_and(|e| e.has_connect()),
    }
}

/// Whether Import… offers Keep owners: only a PostgreSQL dump names roles (db spec §6.6).
pub fn has_owners(engine: &str) -> bool {
    db_flavor(engine) == Some(DbFlavor::Postgres)
}

/// The words typed into Extra options, split as a shell would (quotes and backslashes, nothing
/// expanded); an unclosed quote is an error to show in the sheet, never a run.
pub fn extra_words(text: &str) -> Result<Vec<String>, YardError> {
    split_words(text).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, "Extra options: a quote is not closed")
            .with_hint("close the quote, or put a backslash before it")
    })
}

/// A database name typed into a sheet, as yardd takes it.
pub fn database_name(text: &str) -> Result<String, YardError> {
    let name = text.trim();
    if name.is_empty() {
        return Err(YardError::new(ErrorCode::InvalidArgument, "give the database a name"));
    }
    yard_core::db::check_name(name)?;
    Ok(name.to_string())
}

/// What the Dump… sheet asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpRequest {
    pub instance: String,
    pub database: String,
    pub file: String,
    pub compress: bool,
    /// The save panel already asked whether to replace a file that is there.
    pub overwrite: bool,
    pub extra: String,
}

/// `yard <engine> dump <name> <file> [--compress] [--overwrite] --progress-json --service <i> [-- <words>]`.
/// The engine command an instance's databases go through: `postgres` for `postgres-17`.
fn engine_of(instance: &str) -> String {
    yard_core::services::parse_instance(instance)
        .map_or_else(|| "mysql".to_string(), |(key, _)| key.to_string())
}

pub fn dump_args(r: &DumpRequest) -> Result<Vec<String>, YardError> {
    let words = extra_words(&r.extra)?;
    // Compressed output is gzip, so its name says so; the save panel asked about replacing the
    // name it returned, so a name changed here replaces nothing.
    let renamed = r.compress && !r.file.ends_with(".gz");
    let file = if renamed { format!("{}.gz", r.file) } else { r.file.clone() };
    let mut args = vec![engine_of(&r.instance), "dump".into(), r.database.clone(), file];
    if r.compress {
        args.push("--compress".into());
    }
    if r.overwrite && !renamed {
        args.push("--overwrite".into());
    }
    args.extend(["--progress-json".to_string(), "--service".into(), r.instance.clone()]);
    if !words.is_empty() {
        args.push("--".into());
        args.extend(words);
    }
    Ok(args)
}

/// What the Import… sheet asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub instance: String,
    pub database: String,
    pub file: String,
    pub into_existing: bool,
    pub keep_owners: bool,
    pub extra: String,
}

/// `yard <engine> import <name> <file> [--into-existing] [--keep-owners] --progress-json --service <i>
/// [-- <words>]`, after the name is checked.
pub fn import_args(r: &ImportRequest) -> Result<Vec<String>, YardError> {
    let database = database_name(&r.database)?;
    let words = extra_words(&r.extra)?;
    let mut args = vec![engine_of(&r.instance), "import".into(), database, r.file.clone()];
    if r.into_existing {
        args.push("--into-existing".into());
    }
    if r.keep_owners {
        args.push("--keep-owners".into());
    }
    args.extend(["--progress-json".to_string(), "--service".into(), r.instance.clone()]);
    if !words.is_empty() {
        args.push("--".into());
        args.extend(words);
    }
    Ok(args)
}

/// The database name an import of `file` suggests: its name up to the first dot, with anything
/// Yard does not take in a name turned into `_` (`shop-2026-11-15.sql.gz` → `shop-2026-11-15`).
pub fn suggested_name(file: &str) -> String {
    let base = file.rsplit('/').next().unwrap_or(file);
    let stem = base.split('.').next().unwrap_or(base);
    let name: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .take(yard_core::db::MAX_NAME)
        .collect();
    name
}

/// The state line `yard <engine> connect` prints after the instance's name: whether it is up, and how
/// to wake or start it.
pub fn connect_state(i: &ConnectInfo) -> String {
    let n = &i.instance;
    match &i.state {
        ServiceState::Running => "running".to_string(),
        ServiceState::Stopped if i.sleeping => {
            format!(
                "sleeping — Open, yard {0} open or yard {0} shell wakes it; or yard service start {n}",
                i.engine
            )
        }
        ServiceState::Stopped => format!("stopped — yard service start {n}"),
        ServiceState::Starting => "starting".to_string(),
        ServiceState::Failed { .. } => "failed — the Services screen says why".to_string(),
        ServiceState::Other => "unknown".to_string(),
    }
}

/// One line of the Connect… sheet: a field a GUI client asks for, with its Copy button's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectField {
    /// `host`, `port`, `user`, `password`, `socket`, `url`: the controls' ids end in it.
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
    /// What the field's Copy button copies: the value, without a shown note.
    pub copy: String,
    /// Read after Copy's label, which alone does not say what it copies.
    pub description: String,
}

/// The fields `yard <engine> connect` prints, in its order (db spec §8): Host, Port, User when there is
/// one, Password (`(none)` without one), Socket when there is one, URL.
pub fn connect_fields(i: &ConnectInfo) -> Vec<ConnectField> {
    let field = |key, label: &'static str, value: String, copy: String| ConnectField {
        key,
        label,
        description: format!("Copy the {} of {}", label.to_lowercase(), i.instance),
        value,
        copy,
    };
    let mut out = vec![
        field("host", "Host", i.host.clone(), i.host.clone()),
        field("port", "Port", i.port.to_string(), i.port.to_string()),
    ];
    if let Some(user) = &i.user {
        out.push(field("user", "User", user.clone(), user.clone()));
    }
    out.push(field(
        "password",
        "Password",
        i.password.clone().unwrap_or_else(|| "(none)".into()),
        i.password.clone().unwrap_or_default(),
    ));
    if let Some(socket) = &i.socket {
        let shown = if db_flavor(&i.engine) == Some(DbFlavor::Postgres) {
            format!("{socket}  (the directory; a client takes it as the host)")
        } else {
            socket.clone()
        };
        out.push(field("socket", "Socket", shown, socket.clone()));
    }
    out.push(field("url", "URL", i.url.clone(), i.url.clone()));
    out
}

/// Which clients take the fields: what yardd says (protocol 2.13), else the app's own table for an
/// older yardd.
pub fn connect_clients(info: &ConnectInfo) -> Vec<String> {
    if !info.clients.is_empty() {
        return info.clients.clone();
    }
    let lines: [&str; 2] = match info.engine.as_str() {
        "postgres" => [
            "TablePlus: Open, or paste the URL",
            "DBeaver, DataGrip, Postico: a new PostgreSQL connection with the fields above",
        ],
        "redis" => {
            ["TablePlus: paste the URL", "Another Redis client: a connection to the host and port above"]
        }
        _ => [
            "TablePlus, Sequel Ace: Open, or paste the URL",
            "DBeaver, DataGrip: a new MySQL connection with the fields above",
        ],
    };
    lines.iter().map(|l| l.to_string()).collect()
}

/// The URL Open on a database row opens: the instance's URL with the database as its path.
pub fn database_url(instance_url: &str, database: &str) -> String {
    format!("{}/{database}", instance_url.trim_end_matches('/'))
}

/// Why Open did nothing: macOS has no app for the URL's scheme.
pub fn no_handler(url: &str) -> YardError {
    let scheme = url.split("://").next().unwrap_or("mysql");
    YardError::new(ErrorCode::DbNoHandler, format!("no app opens {scheme}:// URLs"))
        .with_hint("install TablePlus or Sequel Ace, or use Connect… for the details")
}

/// What a finished dump or import says in Activity, from the CLI's `--json` answer
/// (`DbDumped` or `DbImported`), in `yard <engine> dump`'s and `import`'s words.
pub fn outcome_text(answer: &serde_json::Value) -> String {
    if let Ok(i) = serde_json::from_value::<DbImported>(answer.clone()) {
        let created = if i.created { " (created it)" } else { "" };
        let mut text = format!("Imported {} into {} on {}{created}.", i.file, i.name, i.instance);
        if i.skipped > 0 {
            text.push_str(&format!(
                " Skipped {} ownership and grant statements (Keep owners keeps them).",
                i.skipped
            ));
        }
        return text;
    }
    match serde_json::from_value::<DbDumped>(answer.clone()) {
        Ok(d) => format!("Dumped {} from {} to {} ({}).", d.name, d.instance, d.file, bytes_text(d.bytes)),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mysql() -> ConnectInfo {
        ConnectInfo {
            instance: "mysql-8.4".into(),
            engine: "mysql".into(),
            state: ServiceState::Stopped,
            sleeping: true,
            host: "127.0.0.1".into(),
            port: 3307,
            user: Some("root".into()),
            password: None,
            socket: Some("/Users/me/.yard/run/mysql-8.4.sock".into()),
            url: "mysql://root@127.0.0.1:3307".into(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn which_engines_have_what() {
        assert!(has_databases("mysql") && has_databases("postgres"));
        assert!(!has_databases("redis") && !has_databases("minio") && !has_databases("emailulator"));
        assert!(has_connect("redis", None) && has_connect("mysql", None) && !has_connect("minio", None));
        let list: yard_proto::EnginesListResult = serde_json::from_value(serde_json::json!({"engines": [
            {"key": "echo", "name": "Echo", "source": "path", "origin": {"kind": "built_in"}, "installed": true,
             "ports": [], "console": false, "databases": false, "connect": true}]}))
        .unwrap();
        assert!(has_connect("echo", Some(&list)) && !has_connect("redis", Some(&list)));
        assert!(has_owners("postgres") && !has_owners("mysql"));
    }

    #[test]
    fn compress_names_the_file_gz() {
        let r = DumpRequest {
            instance: "mysql-8.4".into(),
            database: "shop".into(),
            file: "/tmp/x.sql".into(),
            compress: true,
            overwrite: true,
            extra: String::new(),
        };
        let args = dump_args(&r).unwrap();
        assert_eq!(args[3], "/tmp/x.sql.gz");
        // The save panel asked about x.sql, not x.sql.gz, so nothing is replaced unasked.
        assert!(!args.contains(&"--overwrite".to_string()), "{args:?}");
        let gz = DumpRequest { file: "/tmp/x.sql.gz".into(), ..r };
        let args = dump_args(&gz).unwrap();
        assert_eq!(args[3], "/tmp/x.sql.gz");
        assert!(args.contains(&"--overwrite".to_string()));
    }

    #[test]
    fn dump_args_in_the_clis_order() {
        let mut r = DumpRequest {
            instance: "mysql-8.4".into(),
            database: "shop".into(),
            file: "/tmp/x.sql.gz".into(),
            compress: true,
            overwrite: false,
            extra: "--routines".into(),
        };
        assert_eq!(
            dump_args(&r).unwrap(),
            [
                "mysql",
                "dump",
                "shop",
                "/tmp/x.sql.gz",
                "--compress",
                "--progress-json",
                "--service",
                "mysql-8.4",
                "--",
                "--routines"
            ]
        );
        r.compress = false;
        r.overwrite = true;
        r.extra = "  ".into();
        assert_eq!(
            dump_args(&r).unwrap(),
            [
                "mysql",
                "dump",
                "shop",
                "/tmp/x.sql.gz",
                "--overwrite",
                "--progress-json",
                "--service",
                "mysql-8.4"
            ],
            "no -- without extra words"
        );
        r.extra = "--where='x".into();
        assert_eq!(dump_args(&r).unwrap_err().message, "Extra options: a quote is not closed");
    }

    #[test]
    fn import_args_check_the_name_and_split_the_words() {
        let r = ImportRequest {
            instance: "postgres-17".into(),
            database: " shop ".into(),
            file: "/Users/me/shop.dump".into(),
            into_existing: true,
            keep_owners: true,
            extra: "--jobs 4 \"--table=a b\"".into(),
        };
        assert_eq!(
            import_args(&r).unwrap(),
            [
                "postgres",
                "import",
                "shop",
                "/Users/me/shop.dump",
                "--into-existing",
                "--keep-owners",
                "--progress-json",
                "--service",
                "postgres-17",
                "--",
                "--jobs",
                "4",
                "--table=a b"
            ]
        );
        let bad = ImportRequest { database: "my shop".into(), ..r.clone() };
        assert!(import_args(&bad).unwrap_err().message.contains("is not a database name"));
        let empty = ImportRequest { database: "".into(), ..r };
        assert_eq!(import_args(&empty).unwrap_err().message, "give the database a name");
    }

    #[test]
    fn an_import_suggests_the_files_name() {
        assert_eq!(suggested_name("/Users/me/shop-2026-11-15.sql.gz"), "shop-2026-11-15");
        assert_eq!(suggested_name("/Users/me/My Shop.dump"), "My_Shop");
        assert_eq!(suggested_name("/Users/me/dumpdir"), "dumpdir");
    }

    #[test]
    fn connect_fields_follow_yard_db_connect() {
        let fields = connect_fields(&mysql());
        let shown: Vec<(&str, &str)> = fields.iter().map(|f| (f.label, f.value.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("Host", "127.0.0.1"),
                ("Port", "3307"),
                ("User", "root"),
                ("Password", "(none)"),
                ("Socket", "/Users/me/.yard/run/mysql-8.4.sock"),
                ("URL", "mysql://root@127.0.0.1:3307")
            ]
        );
        let password = &fields[3];
        assert_eq!(
            (password.copy.as_str(), password.description.as_str()),
            ("", "Copy the password of mysql-8.4")
        );
        assert!(connect_state(&mysql()).starts_with("sleeping — "));

        let redis = ConnectInfo {
            instance: "redis".into(),
            engine: "redis".into(),
            state: ServiceState::Running,
            sleeping: false,
            user: None,
            socket: None,
            url: "redis://127.0.0.1:6380".into(),
            port: 6380,
            ..mysql()
        };
        let keys: Vec<&str> = connect_fields(&redis).iter().map(|f| f.key).collect();
        assert_eq!(keys, ["host", "port", "password", "url"]);
        assert_eq!(connect_state(&redis), "running");

        let pg = ConnectInfo {
            instance: "postgres-17".into(),
            engine: "postgres".into(),
            socket: Some("/Users/me/.yard/run".into()),
            ..mysql()
        };
        let socket = connect_fields(&pg).into_iter().find(|f| f.key == "socket").unwrap();
        assert_eq!(socket.copy, "/Users/me/.yard/run", "the note is shown, not copied");
        assert!(socket.value.ends_with("(the directory; a client takes it as the host)"));
    }

    #[test]
    fn open_names_the_database_and_says_when_nothing_opens_it() {
        assert_eq!(database_url("mysql://root@127.0.0.1:3307", "shop"), "mysql://root@127.0.0.1:3307/shop");
        let e = no_handler("postgresql://me@127.0.0.1:5433/app");
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::DbNoHandler, "no app opens postgresql:// URLs"));
    }

    #[test]
    fn a_finished_transfer_says_what_it_did() {
        let dumped = serde_json::json!({"instance": "mysql-8.4", "name": "shop", "file": "/tmp/x.sql.gz", "bytes": 3145728});
        assert_eq!(outcome_text(&dumped), "Dumped shop from mysql-8.4 to /tmp/x.sql.gz (3.0 MB).");
        let imported = serde_json::json!({"instance": "postgres-17", "name": "app", "file": "/tmp/a.dump", "created": true, "bytes": 10, "skipped": 2});
        assert_eq!(
            outcome_text(&imported),
            "Imported /tmp/a.dump into app on postgres-17 (created it). Skipped 2 ownership and grant statements (Keep owners keeps them)."
        );
        assert_eq!(outcome_text(&serde_json::Value::Null), "");
    }

    #[test]
    fn sizes_in_words() {
        assert_eq!(bytes_text(512), "512 bytes");
        assert_eq!(bytes_text(80 * 1024), "80.0 KB");
        assert_eq!(bytes_text(3 << 20), "3.0 MB");
        assert_eq!(bytes_text(5 << 30), "5.0 GB");
    }

    #[test]
    fn connect_clients_from_answer() {
        let mut info = mysql();
        info.clients = vec!["Any client: the URL".into()];
        assert_eq!(connect_clients(&info), ["Any client: the URL"]);
        // An older yardd sends none: the app's own table for the engine.
        info.clients = vec![];
        assert_eq!(connect_clients(&info).len(), 2);
    }

    #[test]
    fn dump_and_import_run_the_engine_command() {
        let pg = DumpRequest {
            instance: "postgres-17".into(),
            database: "shop".into(),
            file: "/tmp/x.sql".into(),
            compress: false,
            overwrite: false,
            extra: String::new(),
        };
        assert_eq!(dump_args(&pg).unwrap()[..2], ["postgres", "dump"]);
    }
}
