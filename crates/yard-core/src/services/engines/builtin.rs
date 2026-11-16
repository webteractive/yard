//! The engines in Yard's code: MySQL, PostgreSQL and MinIO (recipes spec §2). Their logic is
//! more than a recipe can say: MySQL's legacy-PHP logins, PostgreSQL's locale and `initdb`,
//! `yard <engine>`'s database commands, and MinIO's tap and console.

use super::{
    Connect, Console, DbFlavor, HostHeader, InstanceCtx, LineSpec, MYSQL_LINES, Origin, POSTGRES_LINES,
    PortSpec, ServiceEngine, SourceSpec,
};
use crate::services::DATABASE_START_TIMEOUT;
use crate::services::render::{self, UnitCommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use yard_proto::Account;

pub fn all() -> Vec<Arc<dyn ServiceEngine>> {
    vec![Arc::new(Mysql), Arc::new(Postgres), Arc::new(Minio)]
}

fn account(user: Option<&str>, password: Option<&str>) -> Account {
    Account { user: user.map(str::to_string), password: password.map(str::to_string) }
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| l.to_string()).collect()
}

/// The `DB_*` lines Laravel reads, for MySQL and PostgreSQL.
fn db_env(connection: &str, port: u16, account: &Account) -> Vec<String> {
    vec![
        format!("DB_CONNECTION={connection}"),
        "DB_HOST=127.0.0.1".to_string(),
        format!("DB_PORT={port}"),
        format!("DB_USERNAME={}", account.user.clone().unwrap_or_default()),
        format!("DB_PASSWORD={}", account.password.clone().unwrap_or_default()),
    ]
}

fn db_url(scheme: &str, account: &Account, port: u16) -> String {
    let login = account.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
    format!("{scheme}://{login}127.0.0.1:{port}")
}

fn one_port(usual: u16) -> Vec<PortSpec> {
    vec![PortSpec { suffix: String::new(), usual }]
}

#[derive(Debug)]
struct Mysql;

impl ServiceEngine for Mysql {
    fn key(&self) -> &str {
        "mysql"
    }
    fn display(&self) -> &str {
        "MySQL"
    }
    fn origin(&self) -> Origin {
        Origin::BuiltIn
    }
    fn source(&self) -> SourceSpec {
        SourceSpec::BrewLines(&MYSQL_LINES)
    }
    fn lines(&self) -> Option<&'static LineSpec> {
        Some(&MYSQL_LINES)
    }
    fn ports(&self) -> Vec<PortSpec> {
        one_port(3306)
    }
    fn account(&self, _macos_user: &str) -> Account {
        account(Some("root"), Some(""))
    }
    fn command(&self, ctx: &InstanceCtx) -> UnitCommand {
        render::mysql_command(ctx.instance, ctx.home, ctx.paths)
    }
    fn init(&self, ctx: &InstanceCtx) -> Option<UnitCommand> {
        Some(render::mysql_init(ctx.instance, ctx.home, ctx.paths))
    }
    fn config_file(&self, ctx: &InstanceCtx) -> Option<(PathBuf, String)> {
        Some(render::mysql_cnf(ctx.instance, ctx.paths, ctx.port(self, "")))
    }
    fn user_overrides(&self) -> Option<String> {
        Some(render::MYSQL_OVERRIDES.to_string())
    }
    fn start_timeout(&self) -> Option<Duration> {
        Some(DATABASE_START_TIMEOUT)
    }
    fn env_lines(&self, ctx: &InstanceCtx) -> Vec<String> {
        db_env("mysql", ctx.port(self, ""), &self.account(ctx.macos_user))
    }
    fn databases(&self) -> Option<DbFlavor> {
        Some(DbFlavor::Mysql)
    }
    fn has_connect(&self) -> bool {
        true
    }
    fn connect(&self, ctx: &InstanceCtx) -> Option<Connect> {
        Some(Connect {
            url: db_url("mysql", &self.account(ctx.macos_user), ctx.port(self, "")),
            shell: None,
            clients: lines(&[
                "TablePlus, Sequel Ace: open the URL, or paste it",
                "DBeaver, DataGrip: a new MySQL connection with the fields above",
            ]),
            socket: Some(ctx.paths.mysql_socket(ctx.instance)),
        })
    }
}

#[derive(Debug)]
struct Postgres;

impl ServiceEngine for Postgres {
    fn key(&self) -> &str {
        "postgres"
    }
    fn display(&self) -> &str {
        "PostgreSQL"
    }
    fn origin(&self) -> Origin {
        Origin::BuiltIn
    }
    fn source(&self) -> SourceSpec {
        SourceSpec::BrewLines(&POSTGRES_LINES)
    }
    fn lines(&self) -> Option<&'static LineSpec> {
        Some(&POSTGRES_LINES)
    }
    fn ports(&self) -> Vec<PortSpec> {
        one_port(5432)
    }
    /// The superuser is the macOS user with no password, as in Postgres.app (M4b §5.2).
    fn account(&self, macos_user: &str) -> Account {
        account(Some(macos_user), Some(""))
    }
    fn command(&self, ctx: &InstanceCtx) -> UnitCommand {
        render::postgres_command(ctx.instance, ctx.home, ctx.paths, ctx.port(self, ""))
    }
    fn init(&self, ctx: &InstanceCtx) -> Option<UnitCommand> {
        Some(render::postgres_init(ctx.instance, ctx.home, ctx.paths, ctx.macos_user))
    }
    fn start_timeout(&self) -> Option<Duration> {
        Some(DATABASE_START_TIMEOUT)
    }
    fn env_lines(&self, ctx: &InstanceCtx) -> Vec<String> {
        db_env("pgsql", ctx.port(self, ""), &self.account(ctx.macos_user))
    }
    fn databases(&self) -> Option<DbFlavor> {
        Some(DbFlavor::Postgres)
    }
    fn has_connect(&self) -> bool {
        true
    }
    fn connect(&self, ctx: &InstanceCtx) -> Option<Connect> {
        Some(Connect {
            url: db_url("postgresql", &self.account(ctx.macos_user), ctx.port(self, "")),
            shell: None,
            clients: lines(&[
                "TablePlus: open the URL, or paste it",
                "DBeaver, DataGrip, Postico: a new PostgreSQL connection with the fields above",
            ]),
            socket: Some(ctx.paths.run_dir()),
        })
    }
}

#[derive(Debug)]
struct Minio;

impl ServiceEngine for Minio {
    fn key(&self) -> &str {
        "minio"
    }
    fn display(&self) -> &str {
        "MinIO"
    }
    fn origin(&self) -> Origin {
        Origin::BuiltIn
    }
    /// The `minio/stable` tap's, so the install outlives homebrew/core's deprecated formula.
    fn source(&self) -> SourceSpec {
        SourceSpec::Brew { formula: "minio/stable/minio".into() }
    }
    fn ports(&self) -> Vec<PortSpec> {
        vec![
            PortSpec { suffix: String::new(), usual: 9000 },
            PortSpec { suffix: "console".into(), usual: 9001 },
        ]
    }
    fn account(&self, _macos_user: &str) -> Account {
        account(Some("minioadmin"), Some("minioadmin"))
    }
    fn command(&self, ctx: &InstanceCtx) -> UnitCommand {
        render::minio_command(
            ctx.instance,
            ctx.home,
            ctx.paths,
            ctx.port(self, ""),
            ctx.port(self, "console"),
        )
    }
    fn user_overrides(&self) -> Option<String> {
        Some(render::MINIO_OVERRIDES.to_string())
    }
    fn env_lines(&self, ctx: &InstanceCtx) -> Vec<String> {
        let account = self.account(ctx.macos_user);
        vec![
            format!("AWS_ACCESS_KEY_ID={}", account.user.unwrap_or_default()),
            format!("AWS_SECRET_ACCESS_KEY={}", account.password.unwrap_or_default()),
            "AWS_DEFAULT_REGION=us-east-1".to_string(),
            format!("AWS_ENDPOINT=http://127.0.0.1:{}", ctx.port(self, "")),
            "AWS_USE_PATH_STYLE_ENDPOINT=true".to_string(),
        ]
    }
    fn console(&self) -> Option<Console> {
        Some(Console { port: "console".into(), host_header: HostHeader::Upstream, nginx_overrides: true })
    }
}
