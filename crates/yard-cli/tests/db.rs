//! `yard db` against `MockDaemon` (db spec §6, §8).

mod support;

use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::{MockDaemon, stderr, stdout};

fn lists() -> Value {
    json!([
        {"instance": "mysql-8.4", "engine": "mysql", "databases": [
            {"name": "blog", "size_bytes": 3_145_728, "tables": 12},
            {"name": "shop", "size_bytes": 81_920, "tables": 3}
        ]},
        {"instance": "postgres-17", "engine": "postgres", "databases": []}
    ])
}

fn mysql_connect(state: &str, sleeping: bool) -> Value {
    json!({"instance": "mysql-8.4", "engine": "mysql", "state": state, "sleeping": sleeping,
        "host": "127.0.0.1", "port": 3307, "user": "root", "password": null,
        "socket": "/Users/me/.yard/run/mysql-8.4.sock", "url": "mysql://root@127.0.0.1:3307"})
}

fn pg_connect() -> Value {
    json!({"instance": "postgres-17", "engine": "postgres", "state": "running", "sleeping": false,
        "host": "127.0.0.1", "port": 5433, "user": "me", "password": null,
        "socket": "/Users/me/.yard/run", "url": "postgresql://me@127.0.0.1:5433"})
}

fn redis_connect() -> Value {
    json!({"instance": "redis", "engine": "redis", "state": "running", "sleeping": false,
        "host": "127.0.0.1", "port": 6380, "user": null, "password": null, "socket": null,
        "url": "redis://127.0.0.1:6380"})
}

/// An executable script in `dir`.
fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

#[test]
fn list_snapshot() {
    let d = MockDaemon::start(&[("db.list", Ok(lists()))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn list_json_is_proto_shape() {
    let d = MockDaemon::start(&[("db.list", Ok(lists()))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4", "--json"]).output().unwrap();
    let parsed: Vec<yard_proto::DbInstanceList> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed.len(), 2);
}

#[test]
fn db_sends_the_working_directory() {
    let d = MockDaemon::start(&[("db.list", Ok(json!([]))), ("db.connect", Ok(json!([])))]);
    let dir = tempfile::tempdir().unwrap();
    let out = d.yard().args(["db", "list"]).current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let p = &d.params("db.list")[0];
    assert_eq!(Path::new(p["path"].as_str().unwrap()), dir.path().canonicalize().unwrap());
    assert!(p.get("site").is_none() && p.get("service").is_none());
}

#[test]
fn service_flag_is_sent() {
    let d = MockDaemon::start(&[("db.list", Ok(json!([])))]);
    let out = d.yard().args(["db", "list", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("db.list")[0], json!({"service": "mysql-8.4"}));
    assert!(d.params("db.connect").is_empty(), "nothing to warn about with --service");
}

#[test]
fn list_without_service_warns_before_waking() {
    let d = MockDaemon::start(&[
        ("db.connect", Ok(json!([mysql_connect("stopped", true), redis_connect()]))),
        ("db.list", Ok(lists())),
    ]);
    let out = d.yard().args(["db", "list"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("waking mysql-8.4 to list its databases"), "{}", stderr(&out));
    assert!(!stderr(&out).contains("redis"));
}

#[test]
fn create_says_what_it_did() {
    let d = MockDaemon::start(&[(
        "db.create",
        Ok(json!({"instance": "mysql-8.4", "name": "shop", "created": true})),
    )]);
    let out = d.yard().args(["db", "create", "shop", "--service", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Created shop on mysql-8.4.\n");
    assert_eq!(d.params("db.create")[0], json!({"service": "mysql-8.4", "name": "shop"}));
    let d = MockDaemon::start(&[(
        "db.create",
        Ok(json!({"instance": "mysql-8.4", "name": "shop", "created": false})),
    )]);
    let out = d.yard().args(["db", "create", "shop", "--service", "mysql-8.4"]).output().unwrap();
    assert_eq!(stdout(&out), "shop already exists on mysql-8.4.\n");
}

#[test]
fn connect_snapshot_mysql_sleeping() {
    let d = MockDaemon::start(&[("db.connect", Ok(json!([mysql_connect("stopped", true)])))]);
    let out = d.yard().args(["db", "connect", "mysql-8.4"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
    assert_eq!(d.params("db.connect")[0], json!({"service": "mysql-8.4", "wake": false}));
}

#[test]
fn connect_snapshot_postgres_and_redis() {
    let d = MockDaemon::start(&[("db.connect", Ok(json!([pg_connect(), redis_connect()])))]);
    let out = d.yard().args(["db", "connect"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    insta::assert_snapshot!(stdout(&out));
}

#[test]
fn open_opens_the_url_with_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("opened");
    let open = script(dir.path(), "open", &format!("echo \"$1\" > '{}'", log.display()));
    let d = MockDaemon::start(&[("db.connect", Ok(json!([mysql_connect("running", false)])))]);
    let out = d
        .yard()
        .args(["db", "open", "shop", "--service", "mysql-8.4"])
        .env("YARD_OPEN", &open)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "mysql://root@127.0.0.1:3307/shop\n");
    assert_eq!(d.params("db.connect")[0], json!({"service": "mysql-8.4", "wake": true}));
}

#[test]
fn open_falls_back_to_details() {
    let dir = tempfile::tempdir().unwrap();
    let open = script(dir.path(), "open", "exit 1");
    let d = MockDaemon::start(&[("db.connect", Ok(json!([mysql_connect("running", false)])))]);
    let out =
        d.yard().args(["db", "open", "--service", "mysql-8.4"]).env("YARD_OPEN", &open).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("URL       mysql://root@127.0.0.1:3307"), "{}", stdout(&out));
    assert!(stderr(&out).contains("error: no app opens mysql:// URLs"), "{}", stderr(&out));
}

#[test]
fn shell_execs_the_answered_program() {
    let dir = tempfile::tempdir().unwrap();
    let program =
        script(dir.path(), "client", "echo \"args: $*\"; echo \"PGHOST=$PGHOST\"; echo \"EXTRA=$EXTRA\"");
    let cmd = json!({"instance": "postgres-17", "program": program, "args": ["-d", "app", "-c", "select 1"],
        "env": [["EXTRA", "yes"]], "env_remove": ["PGHOST"], "filter": {"kind": "none"}, "created": false,
        "notes": ["using postgres-17"]});
    let d = MockDaemon::start(&[("db.command", Ok(cmd))]);
    let out = d
        .yard()
        .args(["db", "shell", "app", "--", "-c", "select 1"])
        .env("PGHOST", "elsewhere")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "args: -d app -c select 1\nPGHOST=\nEXTRA=yes\n");
    assert!(stderr(&out).contains("using postgres-17"));
    let p = &d.params("db.command")[0];
    assert_eq!((p["action"].as_str(), p["name"].as_str()), (Some("shell"), Some("app")));
    assert_eq!(p["args"], json!(["-c", "select 1"]));
}

/// `db.command`'s answer running `program` with `args`.
fn command_for(program: &str, args: &[&str]) -> Value {
    json!({"instance": "mysql-8.4", "program": program, "args": args, "env": [], "env_remove": [],
        "filter": {"kind": "none"}, "created": false, "notes": []})
}

/// The one file in `dir` whose name starts with `prefix`.
fn only_file(dir: &Path, prefix: &str) -> std::path::PathBuf {
    let found: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    found[0].clone()
}

fn dated(name: &str, prefix: &str, ext: &str) -> bool {
    let Some(stamp) = name.strip_prefix(prefix).and_then(|r| r.strip_suffix(ext)) else { return false };
    stamp.len() == 15
        && stamp
            .chars()
            .enumerate()
            .all(|(i, c)| if [4, 7, 10].contains(&i) { c == '-' } else { c.is_ascii_digit() })
}

#[test]
fn dump_writes_plain_by_default() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let program = script(tools.path(), "mysqldump", "printf -- '-- dump\\nCREATE TABLE a (x int);\\n'");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &["shop"])))]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let file = only_file(cwd.path(), "shop-");
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(dated(&name, "shop-", ".sql"), "{name}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "-- dump\nCREATE TABLE a (x int);\n");
    assert!(stdout(&out).contains("Dumped shop from mysql-8.4"), "{}", stdout(&out));
    assert!(stdout(&out).contains("--single-transaction --routines"), "the tip: {}", stdout(&out));
    let p = &d.params("db.command")[0];
    assert_eq!((p["action"].as_str(), p["compress"].as_bool()), (Some("dump"), Some(false)));
}

#[test]
fn dump_compress_gzips() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let program = script(tools.path(), "mysqldump", "printf 'CREATE TABLE a (x int);\\n'");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &[])))]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "--compress", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let file = only_file(cwd.path(), "shop-");
    assert!(file.to_string_lossy().ends_with(".sql.gz"), "{file:?}");
    let plain = std::process::Command::new("/usr/bin/gzip").arg("-dc").arg(&file).output().unwrap();
    assert_eq!(plain.stdout, b"CREATE TABLE a (x int);\n");
    assert_eq!(d.params("db.command")[0]["compress"], true);
}

#[test]
fn gz_name_without_compress_is_refused() {
    let cwd = tempfile::tempdir().unwrap();
    let d = MockDaemon::start(&[]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "shop.sql.gz", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--compress"), "{}", stderr(&out));
    assert!(d.params("db.command").is_empty());
}

#[test]
fn existing_file_needs_overwrite() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("shop.sql"), "old").unwrap();
    let program = script(tools.path(), "mysqldump", "printf 'new\\n'");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &[])))]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "shop.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--overwrite"), "{}", stderr(&out));
    let out = d
        .yard()
        .args(["db", "dump", "shop", "shop.sql", "--overwrite", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(cwd.path().join("shop.sql")).unwrap(), "new\n");
}

#[test]
fn failed_dump_leaves_nothing() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let program = script(
        tools.path(),
        "mysqldump",
        "printf 'partial'; echo 'mysqldump: Got error: 1049: Unknown database' >&2; exit 2",
    );
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &[])))]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "x.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Unknown database"), "{}", stderr(&out));
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0, "no file and no .partial");
}

#[test]
fn ctrl_c_leaves_no_partial() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Loops in its own process, so its path stays in the process table while it runs.
    let program = script(tools.path(), "mysqldump", "printf 'started\\n'; while :; do sleep 0.1; done");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &[])))]);
    let child = d
        .yard_process()
        .args(["db", "dump", "shop", "x.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let partial = cwd.path().join("x.sql.partial");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::read_to_string(&partial).unwrap_or_default().is_empty() {
        assert!(std::time::Instant::now() < deadline, "no partial file");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Cancelled: no file was written."));
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
    let left = std::process::Command::new("/usr/bin/pgrep").args(["-f", &program]).output().unwrap();
    assert!(left.stdout.is_empty(), "the tool is still running");
}

#[test]
fn custom_format_name_and_no_gzip() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let program = script(tools.path(), "pg_dump", "printf 'PGDMP\\001'");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &["-Fc"])))]);
    let out = d
        .yard()
        .args(["db", "dump", "app", "--service", "postgres-17", "--", "-Fc"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let file = only_file(cwd.path(), "app-");
    assert!(dated(&file.file_name().unwrap().to_string_lossy(), "app-", ".dump"), "{file:?}");
    assert_eq!(std::fs::read(&file).unwrap(), b"PGDMP\x01");
    assert!(!stdout(&out).contains("--single-transaction"), "no MySQL tip for pg_dump");
}

#[test]
fn directory_format_is_written_by_the_tool() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let body = "while [ $# -gt 0 ]; do if [ \"$1\" = -f ]; then mkdir -p \"$2\"; echo toc > \"$2/toc.dat\"; fi; shift; done";
    let program = script(tools.path(), "pg_dump", body);
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &["-Fd"])))]);
    let out = d
        .yard()
        .args(["db", "dump", "app", "app.dir", "--service", "postgres-17", "--", "-Fd"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(cwd.path().join("app.dir/toc.dat")).unwrap(), "toc\n");
    assert!(!cwd.path().join("app.dir.partial").exists());
}

#[test]
fn progress_json_lines() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let program = script(tools.path(), "mysqldump", "printf 'CREATE TABLE a (x int);\\n'");
    let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &[])))]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "x.sql", "--progress-json", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let last = stderr(&out).lines().rfind(|l| l.starts_with('{')).unwrap().to_string();
    let v: Value = serde_json::from_str(&last).unwrap();
    assert_eq!(v["bytes"], 24);
}

/// `db.command`'s answer for an import through `program`, filtered by `filter`.
fn import_command(program: &str, filter: Value, created: bool) -> Value {
    json!({"instance": "mysql-8.4", "program": program, "args": ["--database=shop"], "env": [], "env_remove": [],
        "filter": filter, "created": created, "notes": []})
}

/// A client that keeps what it reads in `<dir>/stdin`.
fn capturing(dir: &Path) -> String {
    script(dir, "client", &format!("cat > '{}'", dir.join("stdin").display()))
}

#[test]
fn import_streams_the_file() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let dump = "CREATE TABLE a (x int);\nINSERT INTO a VALUES (1);\n";
    std::fs::write(cwd.path().join("shop.sql"), dump).unwrap();
    let d = MockDaemon::start(&[(
        "db.command",
        Ok(import_command(&capturing(tools.path()), json!({"kind": "mysql"}), true)),
    )]);
    let out = d
        .yard()
        .args(["db", "import", "shop", "shop.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(tools.path().join("stdin")).unwrap(), dump);
    assert!(stdout(&out).contains("Imported"), "{}", stdout(&out));
    assert!(stdout(&out).contains("(created it)"), "{}", stdout(&out));
    let p = &d.params("db.command")[0];
    assert_eq!((p["action"].as_str(), p["import_kind"].as_str()), (Some("import"), Some("plain")));
}

#[test]
fn import_gunzips_by_content() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let plain = cwd.path().join("dump.sql");
    std::fs::write(&plain, "CREATE TABLE a (x int);\n").unwrap();
    let gz = std::process::Command::new("/usr/bin/gzip").arg("-c").arg(&plain).output().unwrap().stdout;
    std::fs::write(&plain, gz).unwrap(); // gzipped, still named .sql
    let d = MockDaemon::start(&[(
        "db.command",
        Ok(import_command(&capturing(tools.path()), json!({"kind": "mysql"}), false)),
    )]);
    let out = d
        .yard()
        .args(["db", "import", "shop", "dump.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(tools.path().join("stdin")).unwrap(), "CREATE TABLE a (x int);\n");
}

#[test]
fn a_missing_file_creates_nothing() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("empty.sql"), "").unwrap();
    let d = MockDaemon::start(&[]);
    for file in ["missing.sql", "empty.sql"] {
        let out = d
            .yard()
            .args(["db", "import", "shop", file, "--service", "mysql-8.4"])
            .current_dir(cwd.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert!(stderr(&out).contains(file), "{}", stderr(&out));
    }
    assert!(d.params("db.command").is_empty(), "yardd was never asked, so nothing was created");
}

#[test]
fn a_switching_dump_stops_before_the_statement() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        cwd.path().join("all.sql"),
        "CREATE TABLE a (x int);\nUSE `other`;\nINSERT INTO a VALUES (1);\n",
    )
    .unwrap();
    let d = MockDaemon::start(&[(
        "db.command",
        Ok(import_command(&capturing(tools.path()), json!({"kind": "mysql"}), false)),
    )]);
    let out = d
        .yard()
        .args(["db", "import", "shop", "all.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("switches to database other"), "{}", stderr(&out));
    let sent = std::fs::read_to_string(tools.path().join("stdin")).unwrap();
    assert!(sent.contains("CREATE TABLE a") && !sent.contains("USE") && !sent.contains("INSERT"), "{sent}");
}

#[test]
fn owner_statements_are_skipped_and_counted() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        cwd.path().join("app.sql"),
        "CREATE TABLE t (a int);\nALTER TABLE t OWNER TO postgres;\nGRANT ALL ON t TO x;\n",
    )
    .unwrap();
    let filter = json!({"kind": "postgres", "keep_owners": false});
    let d = MockDaemon::start(&[("db.command", Ok(import_command(&capturing(tools.path()), filter, false)))]);
    let out = d
        .yard()
        .args(["db", "import", "app", "app.sql", "--service", "postgres-17"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Skipped 2 ownership and grant statements"), "{}", stdout(&out));
    assert_eq!(std::fs::read_to_string(tools.path().join("stdin")).unwrap(), "CREATE TABLE t (a int);\n");
    let out = d
        .yard()
        .args([
            "db",
            "import",
            "app",
            "app.sql",
            "--keep-owners",
            "--into-existing",
            "--service",
            "postgres-17",
        ])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let p = &d.params("db.command")[1];
    assert_eq!((p["keep_owners"].as_bool(), p["into_existing"].as_bool()), (Some(true), Some(true)));
}

#[test]
fn archive_goes_to_pg_restore() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("app.dump"), b"PGDMP\x01\x0eGRANT USE x;").unwrap();
    let d = MockDaemon::start(&[(
        "db.command",
        Ok(import_command(&capturing(tools.path()), json!({"kind": "none"}), false)),
    )]);
    let out = d
        .yard()
        .args(["db", "import", "app", "app.dump", "--service", "postgres-17"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(d.params("db.command")[0]["import_kind"], "archive");
    assert_eq!(std::fs::read(tools.path().join("stdin")).unwrap(), b"PGDMP\x01\x0eGRANT USE x;");
}

#[test]
fn failure_says_what_stays() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("shop.sql"), "SELECT 1;\n").unwrap();
    let program = script(
        tools.path(),
        "mysql",
        "cat > /dev/null; echo 'ERROR 1064 (42000) at line 1: near x' >&2; exit 1",
    );
    let d =
        MockDaemon::start(&[("db.command", Ok(import_command(&program, json!({"kind": "mysql"}), false)))]);
    let out = d
        .yard()
        .args(["db", "import", "shop", "shop.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("ERROR 1064") && err.contains("stays in shop") && err.contains("yard db shell shop"),
        "{err}"
    );
}

#[test]
fn ctrl_c_says_what_stays() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("shop.sql"), "SELECT 1;\n").unwrap();
    let started = tools.path().join("started");
    let program =
        script(tools.path(), "mysql", &format!("touch '{}'; while :; do sleep 0.1; done", started.display()));
    let d =
        MockDaemon::start(&[("db.command", Ok(import_command(&program, json!({"kind": "mysql"}), false)))]);
    let child = d
        .yard_process()
        .args(["db", "import", "shop", "shop.sql", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !started.exists() {
        assert!(std::time::Instant::now() < deadline, "the client never started");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Cancelled: what was imported so far stays in shop.")
    );
    let left = std::process::Command::new("/usr/bin/pgrep").args(["-f", &program]).output().unwrap();
    assert!(left.stdout.is_empty(), "the client is still running");
}

#[test]
fn dump_never_replaces_a_directory() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join("project")).unwrap();
    std::fs::write(cwd.path().join("project/keep.txt"), "mine").unwrap();
    let d = MockDaemon::start(&[]);
    let out = d
        .yard()
        .args(["db", "dump", "shop", "project", "--overwrite", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("is a directory"), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(cwd.path().join("project/keep.txt")).unwrap(), "mine");
    assert!(d.params("db.command").is_empty());
}

#[test]
fn a_truncated_gzip_fails() {
    let tools = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let plain: String = (0..20_000).map(|i| format!("INSERT INTO t VALUES ({i});\n")).collect();
    std::fs::write(cwd.path().join("p.sql"), &plain).unwrap();
    let gz = std::process::Command::new("/usr/bin/gzip")
        .arg("-c")
        .arg(cwd.path().join("p.sql"))
        .output()
        .unwrap()
        .stdout;
    std::fs::write(cwd.path().join("cut.sql.gz"), &gz[..gz.len() / 2]).unwrap();
    let d = MockDaemon::start(&[(
        "db.command",
        Ok(import_command(&capturing(tools.path()), json!({"kind": "mysql"}), false)),
    )]);
    let out = d
        .yard()
        .args(["db", "import", "shop", "cut.sql.gz", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stderr(&out).contains("gzip"), "{}", stderr(&out));
}

#[test]
fn a_gzip_with_nothing_inside_creates_nothing() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("junk.sql.gz"), [0x1f, 0x8b, 0, 0, 0, 0]).unwrap();
    let d = MockDaemon::start(&[]);
    let out = d
        .yard()
        .args(["db", "import", "shop", "junk.sql.gz", "--service", "mysql-8.4"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(d.params("db.command").is_empty());
}

#[test]
fn ctrl_c_to_the_whole_group_is_a_cancel() {
    // A terminal's Ctrl-C reaches the whole foreground group, the tool included.
    use std::os::unix::process::CommandExt;
    for _ in 0..3 {
        let tools = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let program = script(tools.path(), "mysqldump", "printf 'started\\n'; while :; do sleep 0.1; done");
        let d = MockDaemon::start(&[("db.command", Ok(command_for(&program, &[])))]);
        let child = d
            .yard_process()
            .args(["db", "dump", "shop", "x.sql", "--service", "mysql-8.4"])
            .current_dir(cwd.path())
            .process_group(0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let partial = cwd.path().join("x.sql.partial");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::fs::read_to_string(&partial).unwrap_or_default().is_empty() {
            assert!(std::time::Instant::now() < deadline, "no partial file");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) };
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(130), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
    }
}
