//! `env.print` and `sites.info`: what a project runs on (M4b spec §7.3, §7.4). Both only
//! read the last good snapshot and the brew keg; neither writes anything.

use super::emailulator::is_emailulator;
use super::handlers::{not_found, snapshot};
use super::services::instance_info;
use crate::daemon::Context;
use crate::emailulator::{next_step, serving_agent};
use crate::services::{default_instance, not_found as service_not_found, service_info};
use crate::sleep::Sleeper;
use crate::state::Snapshot;
use crate::supervisor::Supervisor;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_core::PhpVersion;
use yard_core::emailulator::{NAME as EMAILULATOR, env_lines as mail_env};
use yard_core::php::full_version;
use yard_core::project::{FileKind, ServiceWant};
use yard_core::services::{Engine, env_lines, instance_name};
use yard_proto::methods::{EnvParams, SiteDetailParams};
use yard_proto::{
    EnvBlock, ErrorCode, ServiceInfo, SettingSource, SiteDetail, SiteInfo, SiteKind, SiteService, YardError,
};

/// The note on the second block when two databases are printed together.
const DB_CLASH: &str = "both set DB_*; the second block wins in a .env";

/// `service` names one instance; else `site`, else the linked site whose path contains
/// `path` (the innermost when projects nest); and a project prints the services its `.yard`
/// lists. With none of them, or a `path` that is in no linked project, every instance.
pub(super) async fn env_print(ctx: &Context, p: EnvParams) -> Result<Vec<EnvBlock>, YardError> {
    let snap = snapshot(ctx).await?;
    let supervisor = ctx.supervisor();
    if let Some(name) = &p.service {
        if is_emailulator(name) {
            return mail_block(&snap, false).map(|b| vec![b]).ok_or_else(|| service_not_found(name));
        }
        let engine = snap.services.services.get(name).map(|e| e.engine);
        let info = service_info(&snap, supervisor, &ctx.sleeper, name);
        return match (engine, info) {
            (Some(engine), Some(info)) => Ok(vec![instance_block(engine, &info)]),
            _ => Err(service_not_found(name)),
        };
    }
    let site = match (&p.site, &p.path) {
        (Some(name), _) => {
            Some(snap.resolution.sites.iter().find(|s| s.name == *name).ok_or_else(|| not_found(name))?)
        }
        (None, Some(path)) => site_containing(&snap, Path::new(path)),
        (None, None) => None,
    };
    // A project prints emailulator's lines when its `.yard` asks for them; the whole list does
    // while an agent is installed.
    let mail = match site {
        Some(site) => match snap.projects.get(&site.name).map(|l| &l.parsed) {
            Some(Ok(file)) if file.emailulator => mail_block(&snap, true),
            _ => None,
        },
        None => mail_block(&snap, false),
    };
    let blocks = match site {
        Some(site) => project_blocks(&snap, supervisor, &ctx.sleeper, site)?,
        None => snap
            .services
            .services
            .iter()
            .filter_map(|(name, entry)| {
                Some((entry.engine, service_info(&snap, supervisor, &ctx.sleeper, name)?))
            })
            .map(|(engine, info)| (engine, instance_block(engine, &info)))
            .collect(),
    };
    let mut blocks = note_clashes(blocks);
    blocks.extend(mail);
    if let Some(site) = site {
        blocks.extend(tunnel_block(ctx, &site.name).await);
    }
    Ok(blocks)
}

/// The public `APP_URL` while `site` is shared through a tunnel that has a URL (tunnels spec
/// §10); Yard never writes it into `.env`.
async fn tunnel_block(ctx: &Context, site: &str) -> Option<EnvBlock> {
    let infos = super::tunnels::infos(&ctx.reconciler, &ctx.tunnels).await;
    let url = infos.into_iter().find(|t| t.site == site)?.url?;
    Some(EnvBlock {
        instance: "tunnel".into(),
        lines: vec![format!("APP_URL={url}")],
        note: Some(format!("while {site} is shared through a tunnel; Yard does not write .env")),
    })
}

/// emailulator's `MAIL_*` lines while its LaunchAgent owns mail, with a note when it does not
/// answer. With no agent, `wanted` (a `.yard` asks for it) gives a block naming the command
/// that installs one; otherwise there is none.
fn mail_block(snap: &Snapshot, wanted: bool) -> Option<EnvBlock> {
    let instance = EMAILULATOR.to_string();
    let Some(agent) = serving_agent(snap.emailulator.as_ref()) else {
        let command = next_step(snap.emailulator.as_ref());
        return wanted.then(|| EnvBlock {
            instance,
            lines: vec![],
            note: Some(format!("emailulator is not catching mail: {command}")),
        });
    };
    let lines = mail_env(&agent.smtp.ip().to_string(), agent.smtp.port())
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let note = (!agent.answering).then(|| "emailulator is not answering; see yard doctor".to_string());
    Some(EnvBlock { instance, lines, note })
}

/// One block per `[services]` entry of the site's `.yard`, in engine order. A site with no
/// `.yard` asks for nothing, so it has none; one whose `.yard` does not parse is that error.
fn project_blocks(
    snap: &Snapshot,
    supervisor: &Supervisor,
    sleeper: &Sleeper,
    site: &SiteInfo,
) -> Result<Vec<(Engine, EnvBlock)>, YardError> {
    let file = match snap.projects.get(&site.name).map(|l| &l.parsed) {
        None => return Ok(vec![]),
        Some(Err(e)) => return Err(e.clone()),
        Some(Ok(file)) => file,
    };
    Ok(file
        .services
        .iter()
        .map(|(&engine, want)| {
            let (line, name) = wanted_instance(snap, engine, want);
            let block = match name.and_then(|n| service_info(snap, supervisor, sleeper, &n)) {
                Some(info) => instance_block(engine, &info),
                None => missing_block(engine, line),
            };
            (engine, block)
        })
        .collect())
}

/// The release line a `[services]` value asks for (`None` for `true`) and the instance that
/// meets it, when there is one: the exact one for a line, the default line's else the first
/// by name for `true`.
fn wanted_instance<'a>(
    snap: &'a Snapshot,
    engine: Engine,
    want: &'a ServiceWant,
) -> (Option<&'a str>, Option<String>) {
    match want {
        ServiceWant::Line(line) => {
            let name = instance_name(engine, Some(line));
            (Some(line.as_str()), snap.services.services.contains_key(&name).then_some(name))
        }
        ServiceWant::Default => (None, default_instance(&snap.services, engine).map(str::to_string)),
    }
}

/// The `.env` lines of an instance, from the ports the registry pinned it to. An instance
/// with no registry entry gets no lines, never the engine's usual port: that is not where it
/// runs. MinIO's API port is the entry named for the instance, so a lone console entry is no
/// entry either.
fn instance_block(engine: Engine, info: &ServiceInfo) -> EnvBlock {
    if !info.ports.iter().any(|p| p.name == info.name) {
        return EnvBlock {
            instance: info.name.clone(),
            lines: vec![],
            note: Some(format!(
                "{} has no port in ports.toml yet; yardd registers one on its next reconcile",
                info.name
            )),
        };
    }
    EnvBlock { instance: info.name.clone(), lines: env_lines(engine, &info.ports, &info.account), note: None }
}

/// A block with no lines naming the command that adds what the `.yard` asks for.
fn missing_block(engine: Engine, line: Option<&str>) -> EnvBlock {
    let instance = instance_name(engine, line);
    let command = match line.or(engine.default_line()) {
        Some(line) if engine.versioned() => format!("yard service add {} --version {line}", engine.key()),
        _ => format!("yard service add {}", engine.key()),
    };
    EnvBlock { note: Some(format!("{instance} is not added: {command}")), instance, lines: vec![] }
}

/// The second block that sets `DB_*` says it overrides the first: pasted into one `.env`,
/// it does.
fn note_clashes(blocks: Vec<(Engine, EnvBlock)>) -> Vec<EnvBlock> {
    let mut seen_database = false;
    blocks
        .into_iter()
        .map(|(engine, mut block)| {
            if matches!(engine, Engine::Mysql | Engine::Postgres) && !block.lines.is_empty() {
                if seen_database {
                    block.note = Some(DB_CLASH.to_string());
                }
                seen_database = true;
            }
            block
        })
        .collect()
}

/// The linked site whose directory contains `path`, the innermost when projects nest. Both
/// sides are resolved, since `sites.toml` may spell a path through a symlink (`/var` on
/// macOS) and `path` may name a directory that is not there yet.
fn site_containing<'a>(snap: &'a Snapshot, path: &Path) -> Option<&'a SiteInfo> {
    let path = resolve(path);
    snap.resolution
        .sites
        .iter()
        .filter_map(|site| match &site.kind {
            SiteKind::Php { path: root, .. } => Some((site, resolve(Path::new(root)))),
            SiteKind::Proxy { .. } | SiteKind::Other => None,
        })
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(site, _)| site)
}

/// `path` with symlinks resolved as far as it exists, and the rest appended as written.
fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut head = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(head) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                head = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn outside(what: Option<&str>) -> YardError {
    let message = match what {
        Some(path) => format!("{path} is not inside a linked project"),
        None => "no site was named, and no directory was given".to_string(),
    };
    YardError::new(ErrorCode::SiteNotFound, message).with_hint("yard info <site> or yard link")
}

/// The site by name, or the one containing `path`, with where each setting comes from, the
/// PHP patch version its keg runs and its `[services]`.
pub(super) async fn site_detail(ctx: &Context, p: SiteDetailParams) -> Result<SiteDetail, YardError> {
    let snap = snapshot(ctx).await?;
    let site = match (&p.site, &p.path) {
        (Some(name), _) => {
            snap.resolution.sites.iter().find(|s| s.name == *name).ok_or_else(|| not_found(name))?
        }
        (None, Some(path)) => site_containing(&snap, Path::new(path)).ok_or_else(|| outside(Some(path)))?,
        (None, None) => return Err(outside(None)),
    };
    let supervisor = ctx.supervisor();
    let services = match snap.projects.get(&site.name).map(|l| &l.parsed) {
        Some(Ok(file)) => file
            .services
            .iter()
            .map(|(&engine, want)| {
                let (line, name) = wanted_instance(&snap, engine, want);
                SiteService {
                    engine: engine.key().to_string(),
                    want: line.map(str::to_string),
                    service: name.and_then(|n| instance_info(&snap, supervisor, &ctx.sleeper, &n)),
                }
            })
            .collect(),
        _ => vec![],
    };
    let mut site = [site.clone()];
    crate::node::fill_node(&mut site, &ctx.paths);
    let [site] = site;
    Ok(SiteDetail { php_full: php_full(ctx, &site), sources: sources(&snap, &site), services, site })
}

/// The full version of the PHP keg the site runs: the Cellar directory `opt/php@<v>` links
/// to, without brew's revision. `None` for a proxy, a version that is not installed, or a
/// keg that is not a link into the Cellar.
fn php_full(ctx: &Context, site: &SiteInfo) -> Option<String> {
    let SiteKind::Php { php, .. } = &site.kind else { return None };
    let version: PhpVersion = php.parse().ok()?;
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let opt = ctx.brew.php_opt(version, &|p: &Path| p.exists(), &read_link).ok()?;
    full_version(&read_link(&opt)?.to_string_lossy())
}

/// Where `php`, `docroot` and `secure` come from: the `.yard` when the site has one, the
/// project's or its managed one (it owns them: `sites.toml`'s are not read then), else
/// `sites.toml` where its entry sets them, else the default. A proxy has only `secure`.
fn sources(snap: &Snapshot, site: &SiteInfo) -> BTreeMap<String, SettingSource> {
    let entry = snap.sites_cfg.sites.get(&site.name);
    let project = snap.projects.get(&site.name);
    let file = project.and_then(|p| p.ok());
    let from_file = match project.map(|p| p.kind) {
        Some(FileKind::Managed) => SettingSource::ManagedFile,
        _ => SettingSource::ProjectFile,
    };
    let source = |in_file: bool, in_sites_toml: bool| match (project.is_some(), in_file, in_sites_toml) {
        (true, true, _) => from_file,
        (true, false, _) => SettingSource::Default,
        (false, _, true) => SettingSource::SitesToml,
        (false, _, false) => SettingSource::Default,
    };
    let secure = source(file.is_some_and(|f| f.secure.is_some()), entry.is_some_and(|e| e.secure.is_some()));
    let mut out = BTreeMap::from([("secure".to_string(), secure)]);
    if matches!(site.kind, SiteKind::Php { .. }) {
        let php = source(file.is_some_and(|f| f.php.is_some()), entry.is_some_and(|e| e.php.is_some()));
        let docroot =
            source(file.is_some_and(|f| f.docroot.is_some()), entry.is_some_and(|e| e.docroot.is_some()));
        out.insert("php".to_string(), php);
        out.insert("docroot".to_string(), docroot);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::{PortRef, ServiceState};

    fn info(name: &str, engine: Engine, ports: &[(&str, u16)]) -> ServiceInfo {
        ServiceInfo {
            name: name.into(),
            engine: engine.key().into(),
            version: None,
            formula: name.into(),
            ports: ports.iter().map(|(n, p)| PortRef { name: (*n).into(), port: *p }).collect(),
            state: ServiceState::Stopped,
            stopped_by_user: false,
            account: engine.account("me"),
            console: None,
            used_by: vec![],
            sleeping_since: None,
            sleep_after_secs: None,
            sleep_after_source: yard_proto::SettingSource::Default,
            managed_by: yard_proto::ServiceManager::Yard,
        }
    }

    #[test]
    fn a_block_prints_the_registered_port() {
        let block = instance_block(Engine::Redis, &info("redis", Engine::Redis, &[("redis", 6391)]));
        assert_eq!(block.lines[1], "REDIS_PORT=6391");
        assert_eq!(block.note, None);
    }

    #[test]
    fn an_instance_with_no_registry_entry_prints_no_lines_and_never_the_usual_port() {
        for (engine, name, ports) in [
            (Engine::Redis, "redis", &[][..]),
            (Engine::Mysql, "mysql-8.4", &[][..]),
            // MinIO's console entry is not its API port.
            (Engine::Minio, "minio", &[("minio-console", 9001)][..]),
        ] {
            let block = instance_block(engine, &info(name, engine, ports));
            assert_eq!(block.lines, Vec::<String>::new(), "{name}");
            let note = block.note.unwrap();
            assert!(note.starts_with(&format!("{name} has no port")), "{note}");
        }
    }

    #[test]
    fn a_missing_instance_names_the_command_that_adds_it() {
        let note = |engine, line| missing_block(engine, line).note.unwrap();
        assert_eq!(
            note(Engine::Mysql, Some("8.4")),
            "mysql-8.4 is not added: yard service add mysql --version 8.4"
        );
        assert_eq!(
            note(Engine::Postgres, None),
            "postgres-17 is not added: yard service add postgres --version 17"
        );
        assert_eq!(note(Engine::Redis, None), "redis is not added: yard service add redis");
    }

    #[test]
    fn only_a_second_block_with_lines_carries_the_clash() {
        let lines = |instance: &str| EnvBlock {
            instance: instance.into(),
            lines: vec!["DB_CONNECTION=x".into()],
            note: None,
        };
        let missing = missing_block(Engine::Postgres, Some("17"));
        let blocks = note_clashes(vec![
            (Engine::Mysql, lines("mysql-8.4")),
            (Engine::Postgres, missing.clone()),
            (Engine::Redis, lines("redis")),
        ]);
        assert_eq!(blocks[1], missing, "a block with no lines clashes with nothing");
        assert_eq!(blocks[2].note, None, "redis sets no DB_*");
        let blocks =
            note_clashes(vec![(Engine::Mysql, lines("mysql-8.4")), (Engine::Postgres, lines("postgres-17"))]);
        assert_eq!((blocks[0].note.as_deref(), blocks[1].note.as_deref()), (None, Some(DB_CLASH)));
    }
}
