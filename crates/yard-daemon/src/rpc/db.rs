//! `db.list`, `db.create`, `db.connect` and `db.command` (db spec §5–§11). yardd decides which
//! instance, wakes it, runs the small queries with the instance's own client over its socket,
//! and for shell, dump and import answers the exact command the client then runs: the client,
//! not yardd (a LaunchAgent), can open the user's files.

use super::handlers::{not_found, snapshot};
use super::info::site_containing;
use crate::daemon::Context;
use crate::services::{keg, macos_user, not_found as service_not_found, service_info};
use crate::sleep;
use crate::state::Snapshot;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use yard_core::config::services::ServiceEntry;
use yard_core::db::command::{MYSQL_ENV, PG_ENV, Target, build};
use yard_core::db::flags::{Tool, check, check_compress};
use yard_core::db::resolve::{Candidate, Picked, pick};
use yard_core::db::{check_name, connect, pick_postgres_keg, sql, supports_connect, supports_db, valid_name};
use yard_core::services::Engine;
use yard_proto::methods::{DbCommandParams, DbConnectParams, DbCreateParams, DbListParams, DbTarget};
use yard_proto::{
    ConnectInfo, DbAction, DbCommand, DbCreated, DbInstanceList, ErrorCode, ImportKind, ServiceInfo,
    ServiceState, YardError,
};

/// How long one of yardd's own queries may take.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a wake checks whether the instance runs yet.
const READY_POLL: Duration = Duration::from_millis(50);

pub(super) async fn list(ctx: &Context, p: DbListParams) -> Result<Vec<DbInstanceList>, YardError> {
    let snap = snapshot(ctx).await?;
    let wants = project_wants(&snap, &p.target)?;
    let instances: Vec<String> = if p.target.service.is_none() && wants.is_none() {
        candidates(&snap).into_iter().filter(|c| supports_db(c.engine)).map(|c| c.name).collect()
    } else {
        vec![resolve(&snap, &p.target, wants.as_deref(), supports_db)?.instance]
    };
    let mut out = Vec::new();
    for instance in instances {
        let entry = entry(&snap, &instance)?;
        let info = ready(ctx, &instance).await?;
        let mut databases =
            sql::parse_list(&query(ctx, &instance, entry, &info, None, sql::list(entry.engine)).await?);
        if entry.engine == Engine::Postgres {
            for db in &mut databases {
                // A name Yard would not take (another role made it) is listed but never put into
                // a query or `-d`: it could carry SQL or a connection string.
                if !valid_name(&db.name) {
                    continue;
                }
                let count = query(
                    ctx,
                    &instance,
                    entry,
                    &info,
                    Some(&db.name),
                    &sql::table_count(entry.engine, &db.name),
                )
                .await?;
                db.tables = count.trim().parse().unwrap_or(0);
            }
        }
        out.push(DbInstanceList { instance, engine: entry.engine.key().to_string(), databases });
    }
    Ok(out)
}

pub(super) async fn create(ctx: &Context, p: DbCreateParams) -> Result<DbCreated, YardError> {
    check_name(&p.name)?;
    let snap = snapshot(ctx).await?;
    let wants = project_wants(&snap, &p.target)?;
    let instance = resolve(&snap, &p.target, wants.as_deref(), supports_db)?.instance;
    let entry = entry(&snap, &instance)?;
    let info = ready(ctx, &instance).await?;
    let created = ensure(ctx, &instance, entry, &info, &p.name).await?;
    Ok(DbCreated { instance, name: p.name, created })
}

pub(super) async fn connect(ctx: &Context, p: DbConnectParams) -> Result<Vec<ConnectInfo>, YardError> {
    let snap = snapshot(ctx).await?;
    let wants = project_wants(&snap, &p.target)?;
    if p.wake {
        let instance = resolve(&snap, &p.target, wants.as_deref(), supports_connect)?.instance;
        let info = ready(ctx, &instance).await?;
        return Ok(vec![connect_info(ctx, &instance, entry(&snap, &instance)?.engine, &info)]);
    }
    let instances: Vec<String> = if p.target.service.is_none() && wants.is_none() {
        candidates(&snap).into_iter().filter(|c| supports_connect(c.engine)).map(|c| c.name).collect()
    } else {
        vec![resolve(&snap, &p.target, wants.as_deref(), supports_connect)?.instance]
    };
    instances
        .iter()
        .map(|instance| {
            let info = service_info(&snap, ctx.supervisor(), &ctx.sleeper, instance)
                .ok_or_else(|| service_not_found(instance))?;
            Ok(connect_info(ctx, instance, entry(&snap, instance)?.engine, &info))
        })
        .collect()
}

pub(super) async fn command(ctx: &Context, p: DbCommandParams) -> Result<DbCommand, YardError> {
    let snap = snapshot(ctx).await?;
    let wants = project_wants(&snap, &p.target)?;
    let picked = resolve(&snap, &p.target, wants.as_deref(), supports_db)?;
    let instance = picked.instance.clone();
    let entry = entry(&snap, &instance)?;
    let engine = entry.engine;
    if p.action != DbAction::Shell && p.name.is_none() {
        return Err(YardError::new(ErrorCode::InvalidArgument, "name the database"));
    }
    if let Some(name) = &p.name {
        check_name(name)?;
    }
    let kind = p.import_kind.unwrap_or(ImportKind::Plain);
    let tool = match (engine, p.action, kind) {
        (Engine::Postgres, DbAction::Shell, _) => Tool::Psql,
        (Engine::Postgres, DbAction::Dump, _) => Tool::PgDump,
        (Engine::Postgres, DbAction::Import, ImportKind::Plain) => Tool::Psql,
        (Engine::Postgres, DbAction::Import, _) => Tool::PgRestore,
        (_, DbAction::Dump, _) => Tool::Mysqldump,
        (_, _, ImportKind::Plain) => Tool::Mysql,
        (_, _, _) => {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                "that is a PostgreSQL archive; import it into a PostgreSQL instance",
            )
            .with_hint("yard db import <name> <file> --service <postgres instance>"));
        }
    };
    // Refused flags never wake anything.
    check(tool, p.action, &p.args)?;
    if p.action == DbAction::Dump {
        check_compress(tool, p.compress, &p.args)?;
    }

    let info = ready(ctx, &instance).await?;
    let (bin_dir, mut notes) = tools_dir(ctx, &instance, entry, tool);
    let mut created = false;
    if p.action == DbAction::Import {
        let name = p.name.as_deref().unwrap_or_default();
        created = ensure(ctx, &instance, entry, &info, name).await?;
        if !created && !p.into_existing {
            let db = (engine == Engine::Postgres).then_some(name);
            let tables = query(ctx, &instance, entry, &info, db, &sql::table_count(engine, name)).await?;
            if tables.trim().parse::<u64>().unwrap_or(0) > 0 {
                return Err(YardError::new(
                    ErrorCode::DbNotEmpty,
                    format!("{name} on {instance} already has tables; an import adds to what is there"),
                )
                .with_hint(format!("yard db import {name} <file> --into-existing")));
            }
        }
    }
    let socket = socket_of(ctx, &instance, engine).unwrap_or_default();
    let user = user_of(engine);
    let target = Target {
        engine,
        instance: &instance,
        bin_dir: &bin_dir,
        socket: &socket,
        port: port_of(&info),
        user: &user,
    };
    let import = (p.action == DbAction::Import).then_some((kind, p.file.as_deref()));
    let mut cmd = build(&target, p.action, p.name.as_deref(), &p.args, import, p.keep_owners);
    cmd.created = created;
    if picked.fallback {
        notes.insert(0, format!("using {instance}"));
    }
    cmd.notes = notes;
    Ok(cmd)
}

/// Creates `name` when it is missing; true when it did.
async fn ensure(
    ctx: &Context,
    instance: &str,
    entry: &ServiceEntry,
    info: &ServiceInfo,
    name: &str,
) -> Result<bool, YardError> {
    let exists = query(ctx, instance, entry, info, None, &sql::exists(entry.engine, name)).await?;
    if exists.trim().parse::<u64>().unwrap_or(0) > 0 {
        return Ok(false);
    }
    query(ctx, instance, entry, info, None, &sql::create(entry.engine, name)).await?;
    Ok(true)
}

fn candidates(snap: &Snapshot) -> Vec<Candidate> {
    snap.services
        .services
        .iter()
        .map(|(name, e)| Candidate { name: name.clone(), engine: e.engine })
        .collect()
}

fn entry<'a>(snap: &'a Snapshot, instance: &str) -> Result<&'a ServiceEntry, YardError> {
    snap.services.services.get(instance).ok_or_else(|| service_not_found(instance))
}

/// What the project at `site`, or the linked site containing `path`, asks for; `None` outside
/// a project.
fn project_wants(snap: &Snapshot, target: &DbTarget) -> Result<Option<Vec<String>>, YardError> {
    let site = match (&target.site, &target.path) {
        (Some(name), _) => {
            Some(snap.resolution.sites.iter().find(|s| s.name == *name).ok_or_else(|| not_found(name))?)
        }
        (None, Some(path)) => site_containing(snap, Path::new(path)),
        (None, None) => None,
    };
    Ok(site.map(|s| sleep::needed_by(snap, &s.name)))
}

fn resolve(
    snap: &Snapshot,
    target: &DbTarget,
    wants: Option<&[String]>,
    allow: fn(Engine) -> bool,
) -> Result<Picked, YardError> {
    pick(target.service.as_deref(), wants, &candidates(snap), allow)
}

/// Wakes `instance` and waits until it runs (db spec §9): a stopped one is refused, a failed
/// one answers its failure, and one not up within `db_wake_wait` is `SERVICE_WAKE_TIMEOUT`.
async fn ready(ctx: &Context, instance: &str) -> Result<ServiceInfo, YardError> {
    let snap = snapshot(ctx).await?;
    if entry(&snap, instance)?.stopped {
        return Err(YardError::new(ErrorCode::ServiceStopped, format!("{instance} is stopped"))
            .with_hint(format!("yard service start {instance}")));
    }
    let held = sleep::wake(ctx, &[instance.to_string()], ctx.db_wake_wait).await;
    if !held.is_empty() {
        return Err(YardError::new(ErrorCode::PortInUse, held.join("; ")).with_hint("yard services"));
    }
    let deadline = Instant::now() + ctx.db_wake_wait;
    loop {
        let snap = snapshot(ctx).await?;
        let info = service_info(&snap, ctx.supervisor(), &ctx.sleeper, instance)
            .ok_or_else(|| service_not_found(instance))?;
        if info.state == ServiceState::Running {
            return Ok(info);
        }
        if let Some(e) = info.state.failure(instance) {
            return Err(e);
        }
        if Instant::now() >= deadline {
            return Err(YardError::new(
                ErrorCode::ServiceWakeTimeout,
                format!("{instance} did not come up within {} s", ctx.db_wake_wait.as_secs_f32()),
            )
            .with_hint(format!("yard service logs {instance}")));
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

fn port_of(info: &ServiceInfo) -> u16 {
    info.ports.first().map_or(0, |p| p.port)
}

fn user_of(engine: Engine) -> String {
    match engine {
        Engine::Postgres => macos_user(),
        _ => "root".to_string(),
    }
}

/// MySQL's socket file, PostgreSQL's socket directory; Redis has none.
fn socket_of(ctx: &Context, instance: &str, engine: Engine) -> Option<PathBuf> {
    match engine {
        Engine::Mysql => Some(ctx.paths.mysql_socket(instance)),
        Engine::Postgres => Some(ctx.paths.run_dir()),
        _ => None,
    }
}

fn connect_info(ctx: &Context, instance: &str, engine: Engine, info: &ServiceInfo) -> ConnectInfo {
    let socket = socket_of(ctx, instance, engine);
    let mut c = connect::info(
        instance,
        engine,
        info.state.clone(),
        port_of(info),
        &info.account,
        socket.as_deref(),
        None,
    );
    c.sleeping = info.sleeping_since.is_some() && info.state == ServiceState::Stopped;
    c
}

/// The `bin` directory a tool runs from (db spec §7.1) and the note that says so when it is
/// not the instance's own keg.
fn tools_dir(ctx: &Context, instance: &str, entry: &ServiceEntry, tool: Tool) -> (PathBuf, Vec<String>) {
    let own = keg(&ctx.brew, &entry.formula).join("bin");
    if entry.engine != Engine::Postgres {
        return (own, vec![]);
    }
    let Some(major) = entry.version.as_deref().and_then(|v| v.parse::<u32>().ok()) else {
        return (own, vec![]);
    };
    let opt = ctx.brew.prefix.join("opt");
    let installed: Vec<(u32, PathBuf)> = std::fs::read_dir(&opt)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix("postgresql@")?.parse::<u32>().ok()?;
            let bin = opt.join(&name).join("bin");
            bin.join(tool.program()).exists().then_some((n, bin))
        })
        .collect();
    match pick_postgres_keg(major, &installed) {
        Some((m, bin)) if m != major => (bin, vec![format!("using {} {m} for {instance}", tool.program())]),
        Some((_, bin)) => (bin, vec![]),
        None => (own, vec![]),
    }
}

/// Runs `sql` with the instance's own client over its socket and answers what it printed.
async fn query(
    ctx: &Context,
    instance: &str,
    entry: &ServiceEntry,
    info: &ServiceInfo,
    database: Option<&str>,
    sql: &str,
) -> Result<String, YardError> {
    let engine = entry.engine;
    let bin = keg(&ctx.brew, &entry.formula).join("bin");
    let socket = socket_of(ctx, instance, engine).unwrap_or_default().display().to_string();
    let mut cmd = match engine {
        Engine::Postgres => {
            let mut c = tokio::process::Command::new(bin.join("psql"));
            c.args([
                "--no-psqlrc",
                "-At",
                "-F",
                "\t",
                "-h",
                &socket,
                "-p",
                &port_of(info).to_string(),
                "-U",
                &macos_user(),
            ])
            .args(["-d", database.unwrap_or("postgres"), "-c", sql]);
            for var in PG_ENV {
                c.env_remove(var);
            }
            c
        }
        _ => {
            let mut c = tokio::process::Command::new(bin.join("mysql"));
            c.args([
                "--no-defaults",
                "--protocol=socket",
                "-N",
                "-B",
                &format!("--socket={socket}"),
                "--user=root",
            ]);
            for var in MYSQL_ENV {
                c.env_remove(var);
            }
            if let Some(db) = database {
                c.arg(format!("--database={db}"));
            }
            c.args(["-e", sql]);
            c
        }
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let failed = |message: String, tail: Vec<String>| {
        YardError::new(ErrorCode::DbToolFailed, message)
            .with_hint(format!("yard service logs {instance}"))
            .with_data(serde_json::json!({ "tail": tail }))
    };
    let out = tokio::time::timeout(QUERY_TIMEOUT, cmd.output())
        .await
        .map_err(|_| failed(format!("{instance} did not answer within 30 s"), vec![]))?
        .map_err(|e| failed(format!("cannot run {}'s client: {e}", instance), vec![]))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail: Vec<String> =
            lines[lines.len().saturating_sub(20)..].iter().map(|l| l.to_string()).collect();
        let last = tail.last().cloned().unwrap_or_else(|| "no output".to_string());
        return Err(failed(format!("{instance}'s client failed: {last}"), tail));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
