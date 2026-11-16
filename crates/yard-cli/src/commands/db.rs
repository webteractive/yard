//! `yard db` (db spec §6, §8): yardd picks and wakes the instance and builds every command;
//! this side prints, opens the URL, and runs the client in the user's terminal.

use super::{Ctx, Outcome, cwd, cwd_absolute};
use crate::cli::DbAt;
use crate::dbrun::{self, Progress, Ran};
use crate::output::{for_agents, table};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use yard_core::db::flags::{DumpFormat, dump_format};
use yard_core::present::bytes_text as human;
use yard_proto::methods::{self, DbCommandParams, DbConnectParams, DbCreateParams, DbListParams, DbTarget};
use yard_proto::{
    ConnectInfo, DbAction, DbCommand, DbCreated, DbDumped, DbImported, DbInstanceList, ErrorCode, ImportKind,
    ServiceState, YardError,
};

/// The params' target. With no flag yardd needs the directory, as an absolute real path.
pub fn target(at: DbAt) -> Result<DbTarget, YardError> {
    let path = if at.site.is_none() && at.service.is_none() { Some(cwd_absolute()?) } else { None };
    Ok(DbTarget { path, site: at.site, service: at.service, engine: None })
}

pub async fn list(ctx: &Ctx, at: DbAt) -> Outcome {
    let target = target(at)?;
    let mut client = ctx.client().await?;
    if target.service.is_none() && !ctx.json {
        // Say which sleeping instances the list is about to wake (db spec §6.1).
        let params = DbConnectParams { target: target.clone(), wake: false };
        let infos: Vec<ConnectInfo> = client.call(methods::DB_CONNECT, params).await.unwrap_or_default();
        for info in infos.iter().filter(|i| matches!(i.engine.as_str(), "mysql" | "postgres") && i.sleeping) {
            eprintln!("waking {} to list its databases", info.instance);
        }
    }
    let lists: Vec<DbInstanceList> = client.call(methods::DB_LIST, DbListParams { target }).await?;
    ctx.emit(&lists, || list_text(&lists));
    Ok(0)
}

fn list_text(lists: &[DbInstanceList]) -> String {
    let mut out = String::new();
    for list in lists {
        out.push_str(&format!("{}\n", list.instance));
        if list.databases.is_empty() {
            out.push_str(&format!(
                "No databases on {} yet. yard db create <name> makes one.\n\n",
                list.instance
            ));
            continue;
        }
        let rows: Vec<Vec<String>> = list
            .databases
            .iter()
            .map(|d| vec![d.name.clone(), human(d.size_bytes), d.tables.to_string()])
            .collect();
        out.push_str(&table(&["DATABASE", "SIZE", "TABLES"], &rows));
        out.push('\n');
    }
    if lists.is_empty() {
        out.push_str("No MySQL or PostgreSQL instances yet. Add one with yard service add mysql.\n\n");
    }
    out.push_str(&for_agents("yard db list"));
    out
}

pub async fn create(ctx: &Ctx, name: String, at: DbAt) -> Outcome {
    let target = target(at)?;
    let mut client = ctx.client().await?;
    let made: DbCreated = client.call(methods::DB_CREATE, DbCreateParams { target, name }).await?;
    ctx.emit(&made, || match made.created {
        true => format!("Created {} on {}.\n", made.name, made.instance),
        false => format!("{} already exists on {}.\n", made.name, made.instance),
    });
    Ok(0)
}

pub async fn connect(ctx: &Ctx, at: DbAt) -> Outcome {
    let target = target(at)?;
    let mut client = ctx.client().await?;
    let infos: Vec<ConnectInfo> =
        client.call(methods::DB_CONNECT, DbConnectParams { target, wake: false }).await?;
    ctx.emit(&infos, || {
        let blocks: Vec<String> = infos.iter().map(connect_text).collect();
        format!("{}\n{}", blocks.join("\n"), for_agents("yard db connect"))
    });
    Ok(0)
}

/// What a GUI client needs, as spec §8 lays it out.
pub fn connect_text(i: &ConnectInfo) -> String {
    let n = &i.instance;
    let state = match &i.state {
        ServiceState::Running => "running".to_string(),
        ServiceState::Stopped if i.sleeping => {
            format!("sleeping — yard db open or yard db shell wakes it; or yard service start {n}")
        }
        ServiceState::Stopped => format!("stopped — yard service start {n}"),
        ServiceState::Starting => "starting".to_string(),
        ServiceState::Failed { .. } => "failed — yard services says why".to_string(),
        ServiceState::Other => "unknown".to_string(),
    };
    let mut out = format!("{n}  {state}\n");
    let mut field = |name: &str, value: &str| out.push_str(&format!("  {name:<9} {value}\n"));
    field("Host", &i.host);
    field("Port", &i.port.to_string());
    if let Some(user) = &i.user {
        field("User", user);
    }
    field("Password", i.password.as_deref().unwrap_or("(none)"));
    if let Some(socket) = &i.socket {
        let note =
            if i.engine == "postgres" { "  (the directory; a client takes it as the host)" } else { "" };
        field("Socket", &format!("{socket}{note}"));
    }
    field("URL", &i.url);
    out.push('\n');
    let clients = match i.engine.as_str() {
        "postgres" => [
            "TablePlus: yard db open [database], or paste the URL".to_string(),
            "DBeaver, DataGrip, Postico: a new PostgreSQL connection with the fields above".to_string(),
        ],
        "redis" => [
            "TablePlus: paste the URL".to_string(),
            "Another Redis client: a connection to the host and port above".to_string(),
        ],
        _ => [
            "TablePlus, Sequel Ace: yard db open [database], or paste the URL".to_string(),
            "DBeaver, DataGrip: a new MySQL connection with the fields above".to_string(),
        ],
    };
    for line in clients {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

pub async fn open(ctx: &Ctx, name: Option<String>, at: DbAt) -> Outcome {
    let target = target(at)?;
    let mut client = ctx.client().await?;
    let infos: Vec<ConnectInfo> =
        client.call(methods::DB_CONNECT, DbConnectParams { target, wake: true }).await?;
    let info = infos.into_iter().next().ok_or_else(|| YardError::internal("yardd named no instance"))?;
    let url = match &name {
        Some(db) => format!("{}/{db}", info.url),
        None => info.url.clone(),
    };
    let opened = std::process::Command::new(opener()).arg(&url).status().is_ok_and(|s| s.success());
    if !opened {
        print!("{}", connect_text(&info));
        let scheme = url.split("://").next().unwrap_or("mysql");
        return Err(YardError::new(ErrorCode::DbNoHandler, format!("no app opens {scheme}:// URLs"))
            .with_hint("install TablePlus or Sequel Ace, or use the details above")
            .into());
    }
    ctx.emit(&info, || format!("Opened {url}.\n"));
    Ok(0)
}

/// macOS's `open`; tests point `YARD_OPEN` at a stand-in (debug builds only).
fn opener() -> std::path::PathBuf {
    if cfg!(debug_assertions)
        && let Some(path) = std::env::var_os("YARD_OPEN")
    {
        return path.into();
    }
    "/usr/bin/open".into()
}

pub async fn shell(ctx: &Ctx, name: Option<String>, at: DbAt, args: Vec<String>) -> Outcome {
    let target = target(at)?;
    let mut client = ctx.client().await?;
    let params = DbCommandParams {
        target,
        action: DbAction::Shell,
        name,
        args,
        compress: false,
        into_existing: false,
        keep_owners: false,
        import_kind: None,
        file: None,
    };
    let cmd: DbCommand = client.call(methods::DB_COMMAND, params).await?;
    for note in &cmd.notes {
        eprintln!("{note}");
    }
    Err(exec(&cmd).into())
}

/// Replaces this process with the command; it returns only when that fails.
fn exec(cmd: &DbCommand) -> YardError {
    let mut c = std::process::Command::new(&cmd.program);
    c.args(&cmd.args);
    for var in &cmd.env_remove {
        c.env_remove(var);
    }
    c.envs(cmd.env.iter().map(|(k, v)| (k, v)));
    let e = c.exec();
    YardError::new(ErrorCode::DbToolFailed, format!("cannot run {}: {e}", cmd.program))
}

/// `yard db dump`'s own flags.
pub struct DumpArgs {
    pub file: Option<PathBuf>,
    pub compress: bool,
    pub overwrite: bool,
    pub progress_json: bool,
}

pub async fn dump(ctx: &Ctx, name: String, how: DumpArgs, at: DbAt, args: Vec<String>) -> Outcome {
    let format = dump_format(&args);
    let here = cwd()?;
    let out = match &how.file {
        Some(f) => here.join(f),
        None => here.join(dbrun::default_dump_name(&name, format, how.compress, &dbrun::local_stamp())),
    };
    // Settled before yardd is asked, so a refusal wakes nothing.
    if how.file.is_some() && out.to_string_lossy().ends_with(".gz") && !how.compress {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            "a .gz name needs --compress, which gzips the dump",
        )
        .with_hint("--compress")
        .into());
    }
    // A directory is only ever pg_dump -Fd's own output: --overwrite must never empty a folder.
    if out.is_dir() && !(format == DumpFormat::Directory && out.join("toc.dat").is_file()) {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{} is a directory; name a file to write", out.display()),
        )
        .into());
    }
    if out.exists() && !how.overwrite {
        return Err(YardError::new(ErrorCode::FileExists, format!("{} is already there", out.display()))
            .with_hint("--overwrite")
            .into());
    }
    let parent = out.parent().map(PathBuf::from).unwrap_or_default();
    if !parent.is_dir() {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{} is not a directory", parent.display()),
        )
        .into());
    }
    let target = target(at)?;
    let mut client = ctx.client().await?;
    let params = DbCommandParams {
        target,
        action: DbAction::Dump,
        name: Some(name.clone()),
        args: args.clone(),
        compress: how.compress,
        into_existing: false,
        keep_owners: false,
        import_kind: None,
        file: None,
    };
    let cmd: DbCommand = client.call(methods::DB_COMMAND, params).await?;
    drop(client);
    for note in &cmd.notes {
        eprintln!("{note}");
    }
    let progress = Progress::pick(how.progress_json);
    let directory = format == DumpFormat::Directory;
    match dbrun::dump(&cmd, &out, how.compress, directory, &name, progress).await? {
        Ran::Cancelled => {
            eprintln!("Cancelled: no file was written.");
            Ok(130)
        }
        Ran::Done(bytes) => {
            let done =
                DbDumped { instance: cmd.instance.clone(), name, file: out.display().to_string(), bytes };
            let mysql_plain =
                cmd.program.ends_with("/mysqldump") && !args.iter().any(|a| a == "--single-transaction");
            ctx.emit(&done, || {
                let mut text = format!(
                    "Dumped {} from {} to {} ({}).\n",
                    done.name,
                    done.instance,
                    done.file,
                    human(done.bytes)
                );
                if mysql_plain {
                    text.push_str(
                        "Tip: -- --single-transaction --routines gives a consistent dump with procedures.\n",
                    );
                }
                text
            });
            Ok(0)
        }
    }
}

/// `yard db import`'s own flags.
pub struct ImportArgs {
    pub file: PathBuf,
    pub into_existing: bool,
    pub keep_owners: bool,
    pub progress_json: bool,
}

pub async fn import(ctx: &Ctx, name: String, how: ImportArgs, at: DbAt, args: Vec<String>) -> Outcome {
    let file = cwd()?.join(&how.file);
    // The file is read before yardd is asked, so a bad one creates no database.
    let (head, gzip) = dbrun::head(&file).await?;
    let is_dir = file.is_dir();
    let kind = yard_core::db::detect::kind(&head, is_dir, file.join("toc.dat").is_file())?;
    let target = target(at)?;
    let mut client = ctx.client().await?;
    let params = DbCommandParams {
        target,
        action: DbAction::Import,
        name: Some(name.clone()),
        args,
        compress: false,
        into_existing: how.into_existing,
        keep_owners: how.keep_owners,
        import_kind: Some(kind),
        // pg_restore reads a directory, or an archive that is not gzipped, by path (so -j works).
        file: (kind == ImportKind::Directory || (kind == ImportKind::Archive && !gzip))
            .then(|| file.display().to_string()),
    };
    let cmd: DbCommand = client.call(methods::DB_COMMAND, params).await?;
    drop(client);
    for note in &cmd.notes {
        eprintln!("{note}");
    }
    let filter = yard_core::db::filter::Filter::new(&cmd.filter, &name).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, "yardd asked for an import filter this yard does not know")
            .with_hint("yard self-update")
    })?;
    let progress = Progress::pick(how.progress_json);
    match dbrun::import(&cmd, &file, kind, gzip, filter, &name, progress).await? {
        Ran::Cancelled => {
            eprintln!("Cancelled: what was imported so far stays in {name}.");
            Ok(130)
        }
        Ran::Done(done) => {
            let result = DbImported {
                instance: cmd.instance.clone(),
                name,
                file: file.display().to_string(),
                created: cmd.created,
                bytes: done.bytes,
                skipped: done.skipped,
            };
            ctx.emit(&result, || {
                let created = if result.created { " (created it)" } else { "" };
                let mut text = format!(
                    "Imported {} into {} on {}{created}.\n",
                    result.file, result.name, result.instance
                );
                if result.skipped > 0 {
                    text.push_str(&format!(
                        "Skipped {} ownership and grant statements (--keep-owners keeps them).\n",
                        result.skipped
                    ));
                }
                text
            });
            Ok(0)
        }
    }
}
