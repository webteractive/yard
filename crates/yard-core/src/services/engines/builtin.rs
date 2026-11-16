//! The engines in Yard's code. Until the enum goes (recipes plan Task 5) each one answers from
//! the enum's existing functions, so the contract is proved against them before they move.

use super::{
    Connect, Console, DbFlavor, HostHeader, InstanceCtx, LineSpec, MYSQL_LINES, Origin, POSTGRES_LINES,
    PortSpec, ServiceEngine, SourceSpec, main_port,
};
use crate::services::render::{self, UnitCommand};
use crate::services::{DATABASE_START_TIMEOUT, Engine};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use yard_proto::Account;

/// MySQL, PostgreSQL, MinIO, and Redis until it becomes a recipe (Task 4).
pub fn all() -> Vec<Arc<dyn ServiceEngine>> {
    Engine::ALL.into_iter().map(|e| Arc::new(Builtin(e)) as Arc<dyn ServiceEngine>).collect()
}

#[derive(Debug)]
struct Builtin(Engine);

/// The lines a GUI client needs, by engine (the CLI's and Yard.app's tables until now).
fn clients(engine: Engine) -> Vec<String> {
    let lines: [&str; 2] = match engine {
        Engine::Postgres => [
            "TablePlus: open the URL, or paste it",
            "DBeaver, DataGrip, Postico: a new PostgreSQL connection with the fields above",
        ],
        Engine::Redis => {
            ["TablePlus: paste the URL", "Another Redis client: a connection to the host and port above"]
        }
        _ => [
            "TablePlus, Sequel Ace: open the URL, or paste it",
            "DBeaver, DataGrip: a new MySQL connection with the fields above",
        ],
    };
    lines.iter().map(|l| l.to_string()).collect()
}

impl ServiceEngine for Builtin {
    fn key(&self) -> &str {
        self.0.key()
    }

    fn display(&self) -> &str {
        self.0.display()
    }

    fn origin(&self) -> Origin {
        Origin::BuiltIn
    }

    fn source(&self) -> SourceSpec {
        match self.0 {
            Engine::Mysql => SourceSpec::BrewLines(&MYSQL_LINES),
            Engine::Postgres => SourceSpec::BrewLines(&POSTGRES_LINES),
            Engine::Redis => SourceSpec::Brew { formula: "redis".into() },
            Engine::Minio => SourceSpec::Brew { formula: "minio/stable/minio".into() },
        }
    }

    fn lines(&self) -> Option<&'static LineSpec> {
        match self.0 {
            Engine::Mysql => Some(&MYSQL_LINES),
            Engine::Postgres => Some(&POSTGRES_LINES),
            Engine::Redis | Engine::Minio => None,
        }
    }

    fn ports(&self) -> Vec<PortSpec> {
        self.0
            .usual_ports()
            .iter()
            .map(|(suffix, usual)| PortSpec { suffix: suffix.to_string(), usual: *usual })
            .collect()
    }

    fn account(&self, macos_user: &str) -> Account {
        self.0.account(macos_user)
    }

    fn dirs(&self) -> Vec<String> {
        if self.0 == Engine::Redis { vec!["data".into()] } else { Vec::new() }
    }

    fn command(&self, ctx: &InstanceCtx) -> UnitCommand {
        render::command(self.0, ctx.instance, ctx.home, ctx.paths, ctx.ports)
    }

    fn init(&self, ctx: &InstanceCtx) -> Option<UnitCommand> {
        render::init_command(self.0, ctx.instance, ctx.home, ctx.paths, ctx.macos_user)
    }

    fn config_file(&self, ctx: &InstanceCtx) -> Option<(PathBuf, String)> {
        render::config_file(self.0, ctx.instance, ctx.paths, ctx.ports)
    }

    fn user_overrides(&self) -> Option<String> {
        render::user_overrides(self.0).map(str::to_string)
    }

    fn start_timeout(&self) -> Option<Duration> {
        matches!(self.0, Engine::Mysql | Engine::Postgres).then_some(DATABASE_START_TIMEOUT)
    }

    fn env_lines(&self, ctx: &InstanceCtx) -> Vec<String> {
        crate::services::env_lines(self.0, ctx.ports, &self.account(ctx.macos_user))
    }

    fn console(&self) -> Option<Console> {
        (self.0 == Engine::Minio).then(|| Console {
            port: "console".into(),
            host_header: HostHeader::Upstream,
            nginx_overrides: true,
        })
    }

    fn databases(&self) -> Option<DbFlavor> {
        match self.0 {
            Engine::Mysql => Some(DbFlavor::Mysql),
            Engine::Postgres => Some(DbFlavor::Postgres),
            Engine::Redis | Engine::Minio => None,
        }
    }

    fn connect(&self, ctx: &InstanceCtx) -> Option<Connect> {
        let port = main_port(self, ctx.instance, ctx.ports);
        let account = self.account(ctx.macos_user);
        let login = account.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        let (scheme, socket, shell) = match self.0 {
            Engine::Mysql => ("mysql", Some(ctx.paths.mysql_socket(ctx.instance)), None),
            Engine::Postgres => ("postgresql", Some(ctx.paths.run_dir()), None),
            Engine::Redis => (
                "redis",
                None,
                Some(vec![
                    ctx.home.join("bin/redis-cli").into_os_string(),
                    "-p".into(),
                    port.to_string().into(),
                ]),
            ),
            Engine::Minio => return None,
        };
        Some(Connect {
            url: format!("{scheme}://{login}127.0.0.1:{port}"),
            shell,
            clients: clients(self.0),
            socket,
        })
    }
}
