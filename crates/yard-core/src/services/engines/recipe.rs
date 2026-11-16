//! Recipes (recipes spec §6): an engine read from a TOML file. Parsing checks everything a
//! recipe can get wrong before Yard runs it, and reports the file and line; expansion then
//! cannot fail. Pure: the caller reads the file.

use super::{
    Connect, Console, EngineKey, HostHeader, InstanceCtx, Origin, PortSpec, ServiceEngine, SourceSpec,
    YARD_COMMANDS,
};
use crate::services::render::{SIGINT, SIGTERM, UnitCommand};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml::Spanned;
use yard_proto::{Account, ErrorCode, YardError};

/// SIGQUIT, which some servers take as a graceful stop.
pub const SIGQUIT: i32 = 3;

/// The only recipe format this Yard reads.
pub const SCHEMA: i64 = 1;

/// How a recipe may be loaded. `allow_file_urls` lets a `download` name a `file://` URL
/// (tests only; yardd sets it from `DaemonOptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadOptions {
    pub allow_file_urls: bool,
}

/// Why a recipe did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeError {
    pub file: PathBuf,
    /// 1-based, when Yard can tell.
    pub line: Option<usize>,
    pub message: String,
}

impl RecipeError {
    /// `ENGINE_RECIPE_INVALID`, `<file>:<line>: <why>`, hinting at `yard engines check`.
    pub fn to_yard_error(&self) -> YardError {
        let at = match self.line {
            Some(line) => format!("{}:{line}", self.file.display()),
            None => self.file.display().to_string(),
        };
        YardError::new(ErrorCode::EngineRecipeInvalid, format!("{at}: {}", self.message))
            .with_hint(format!("yard engines check {}", self.file.display()))
            .with_data(serde_json::json!({ "file": self.file.display().to_string(), "line": self.line }))
    }
}

/// A placeholder in a recipe string (spec §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Var {
    Home,
    Program,
    Node,
    Data,
    Dir,
    Run,
    Instance,
    Conf,
    Overrides,
    IncludeOverrides,
    User,
    /// The port's suffix: `""` for `main`.
    Port(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// With `quoted` (`{data:quoted}`), the value is double-quoted with `\\` and `"` escaped.
    Var(Var, bool),
}

/// A recipe string split once into text and placeholders; a value is never re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Template(Vec<Piece>);

/// What a template may name where it is.
struct Allowed<'a> {
    kind: SourceKind,
    ports: &'a [(String, PortSpec)],
    has_config: bool,
    include: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Brew,
    Path,
    Download,
    Npm,
}

impl Template {
    fn parse(text: &str, allowed: &Allowed) -> Result<Template, String> {
        let mut pieces = Vec::new();
        let mut literal = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    literal.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    literal.push('}');
                }
                '}' => return Err("a } with no {: write }} for a literal }".into()),
                '{' => {
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(c) => name.push(c),
                            None => return Err(format!("an unclosed {{{name}: write {{{{ for a literal {{")),
                        }
                    }
                    if !literal.is_empty() {
                        pieces.push(Piece::Text(std::mem::take(&mut literal)));
                    }
                    let (name, quoted) = match name.split_once(':') {
                        None => (name.as_str(), false),
                        Some((name, "quoted")) => (name, true),
                        Some((_, other)) => {
                            return Err(format!(
                                "{{{name}}}: :{other} is not a form Yard knows (only :quoted)"
                            ));
                        }
                    };
                    pieces.push(Piece::Var(var(name, allowed)?, quoted));
                }
                c => literal.push(c),
            }
        }
        if !literal.is_empty() {
            pieces.push(Piece::Text(literal));
        }
        Ok(Template(pieces))
    }
}

fn var(name: &str, allowed: &Allowed) -> Result<Var, String> {
    let only = |ok: bool, var: Var, why: &str| if ok { Ok(var) } else { Err(format!("{{{name}}} {why}")) };
    match name {
        "home" => {
            only(allowed.kind != SourceKind::Path, Var::Home, "is not set for a path source: use {program}")
        }
        "program" => only(allowed.kind == SourceKind::Path, Var::Program, "is only for a path source"),
        "node" => only(allowed.kind == SourceKind::Npm, Var::Node, "is only for an npm source"),
        "data" => Ok(Var::Data),
        "dir" => Ok(Var::Dir),
        "run" => Ok(Var::Run),
        "instance" => Ok(Var::Instance),
        "user" => Ok(Var::User),
        "overrides" => Ok(Var::Overrides),
        "conf" => only(allowed.has_config, Var::Conf, "needs a [run.config]"),
        "include_overrides" => only(
            allowed.include,
            Var::IncludeOverrides,
            "is only for run.config.text, with run.config.include",
        ),
        _ => {
            let Some(port) = name.strip_prefix("port.") else {
                return Err(format!("unknown placeholder {{{name}}}"));
            };
            match allowed.ports.iter().find(|(n, _)| n == port) {
                Some((_, spec)) => Ok(Var::Port(spec.suffix.clone())),
                None => Err(format!("{{{name}}}: there is no port named {port} in [ports]")),
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Config {
    /// What starts a comment line in the file (`#`); empty writes no header.
    comment: String,
    file: String,
    text: Template,
    include: Option<Template>,
    overrides: Option<String>,
}

#[derive(Debug, Clone)]
struct ConnectSpec {
    url: Template,
    shell: Option<Vec<Template>>,
    clients: Vec<String>,
}

/// An engine read from a recipe file.
#[derive(Debug, Clone)]
pub struct RecipeEngine {
    key: String,
    name: String,
    description: String,
    origin: Origin,
    source: SourceSpec,
    ports: Vec<(String, PortSpec)>,
    command: Vec<Template>,
    env: Vec<(String, Template)>,
    stop_signal: i32,
    start_timeout: Option<Duration>,
    dirs: Vec<String>,
    init: Option<Vec<Template>>,
    sleep: bool,
    config: Option<Config>,
    health: String,
    console: Option<Console>,
    env_lines: Vec<Template>,
    account: Account,
    connect: Option<ConnectSpec>,
}

impl RecipeEngine {
    /// The version a `download` or `npm` recipe pins.
    pub fn version(&self) -> Option<&str> {
        match &self.source {
            SourceSpec::Download { version, .. } | SourceSpec::Npm { version, .. } => Some(version),
            _ => None,
        }
    }

    fn expand(&self, t: &Template, ctx: &InstanceCtx) -> String {
        let mut out = String::new();
        for piece in &t.0 {
            match piece {
                Piece::Text(s) => out.push_str(s),
                Piece::Var(v, false) => out.push_str(&self.value(v, ctx)),
                Piece::Var(v, true) => out.push_str(&crate::render::quote::nginx_str(&self.value(v, ctx))),
            }
        }
        out
    }

    fn value(&self, v: &Var, ctx: &InstanceCtx) -> String {
        let show = |p: PathBuf| p.display().to_string();
        match v {
            Var::Home => ctx.home.display().to_string(),
            Var::Program => match &self.source {
                SourceSpec::Path { program } => program.display().to_string(),
                _ => String::new(),
            },
            Var::Node => ctx.node.map(|n| n.display().to_string()).unwrap_or_default(),
            Var::Data => show(ctx.paths.service_data(ctx.instance)),
            Var::Dir => show(ctx.paths.service_dir(ctx.instance)),
            Var::Run => show(ctx.paths.run_dir()),
            Var::Instance => ctx.instance.to_string(),
            Var::Conf => self
                .config
                .as_ref()
                .map(|c| show(ctx.paths.service_conf(ctx.instance, &c.file)))
                .unwrap_or_default(),
            Var::Overrides => show(ctx.paths.service_overrides(ctx.instance)),
            Var::IncludeOverrides => self
                .config
                .as_ref()
                .and_then(|c| c.include.as_ref())
                .map(|t| self.expand(t, ctx))
                .unwrap_or_default(),
            Var::User => ctx.macos_user.to_string(),
            Var::Port(suffix) => ctx.port(self, suffix).to_string(),
        }
    }

    fn unit(&self, args: &[Template], ctx: &InstanceCtx, env: bool) -> UnitCommand {
        let program = PathBuf::from(args.first().map(|t| self.expand(t, ctx)).unwrap_or_default());
        UnitCommand {
            program,
            args: args.iter().skip(1).map(|t| OsString::from(self.expand(t, ctx))).collect(),
            env: if env {
                self.env.iter().map(|(k, t)| (k.clone(), self.expand(t, ctx))).collect()
            } else {
                Vec::new()
            },
            stop_signal: self.stop_signal,
        }
    }
}

impl ServiceEngine for RecipeEngine {
    fn key(&self) -> &str {
        &self.key
    }
    fn display(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn origin(&self) -> Origin {
        self.origin.clone()
    }
    fn source(&self) -> SourceSpec {
        self.source.clone()
    }
    fn ports(&self) -> Vec<PortSpec> {
        self.ports.iter().map(|(_, p)| p.clone()).collect()
    }
    fn account(&self, _macos_user: &str) -> Account {
        self.account.clone()
    }
    fn dirs(&self) -> Vec<String> {
        self.dirs.clone()
    }
    fn command(&self, ctx: &InstanceCtx) -> UnitCommand {
        self.unit(&self.command, ctx, true)
    }
    fn init(&self, ctx: &InstanceCtx) -> Option<UnitCommand> {
        self.init.as_ref().map(|args| self.unit(args, ctx, true))
    }
    fn config_file(&self, ctx: &InstanceCtx) -> Option<(PathBuf, String)> {
        let c = self.config.as_ref()?;
        let header = match (c.comment.as_str(), &c.overrides) {
            ("", _) => String::new(),
            (comment, Some(_)) => {
                format!(
                    "{comment} Generated by Yard — edit {} instead\n",
                    ctx.paths.service_overrides(ctx.instance).display()
                )
            }
            (comment, None) => format!(
                "{comment} Generated by Yard from the {} recipe — Yard rewrites this file\n",
                self.key
            ),
        };
        Some((ctx.paths.service_conf(ctx.instance, &c.file), header + &self.expand(&c.text, ctx)))
    }
    fn user_overrides(&self) -> Option<String> {
        self.config.as_ref().and_then(|c| c.overrides.clone())
    }
    fn start_timeout(&self) -> Option<Duration> {
        self.start_timeout
    }
    fn may_sleep(&self) -> bool {
        self.sleep
    }
    fn health_port(&self) -> String {
        self.health.clone()
    }
    fn env_lines(&self, ctx: &InstanceCtx) -> Vec<String> {
        self.env_lines.iter().map(|t| self.expand(t, ctx)).collect()
    }
    fn console(&self) -> Option<Console> {
        self.console.clone()
    }
    fn has_connect(&self) -> bool {
        self.connect.is_some()
    }
    fn connect(&self, ctx: &InstanceCtx) -> Option<Connect> {
        let c = self.connect.as_ref()?;
        Some(Connect {
            url: self.expand(&c.url, ctx),
            shell: c
                .shell
                .as_ref()
                .map(|args| args.iter().map(|t| OsString::from(self.expand(t, ctx))).collect()),
            clients: c.clients.clone(),
            socket: None,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema: Spanned<i64>,
    key: Spanned<String>,
    name: String,
    #[serde(default)]
    description: String,
    source: toml::Table,
    ports: BTreeMap<Spanned<String>, Spanned<i64>>,
    run: RawRun,
    health: RawHealth,
    console: Option<RawConsole>,
    env: Option<RawEnv>,
    account: Option<RawAccount>,
    connect: Option<RawConnect>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum RawSource {
    Brew {
        formula: String,
    },
    Path {
        program: String,
    },
    Download {
        version: String,
        url: String,
        sha256: String,
        #[serde(default)]
        strip: u32,
    },
    Npm {
        package: String,
        version: String,
        node: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    command: Spanned<Vec<String>>,
    #[serde(default)]
    env: BTreeMap<String, Spanned<String>>,
    stop_signal: Option<Spanned<String>>,
    start_timeout: Option<Spanned<String>>,
    #[serde(default)]
    dirs: Vec<Spanned<String>>,
    init: Option<Spanned<Vec<String>>>,
    #[serde(default = "yes")]
    sleep: bool,
    config: Option<RawConfig>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    comment: Option<String>,
    file: Spanned<String>,
    text: Spanned<String>,
    include: Option<Spanned<String>>,
    overrides: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHealth {
    tcp: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConsole {
    port: Spanned<String>,
    host_header: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnv {
    lines: Spanned<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    user: Option<String>,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConnect {
    url: Spanned<String>,
    shell: Option<Spanned<Vec<String>>>,
    #[serde(default)]
    clients: Vec<String>,
}

/// Reports against one file, with lines from byte offsets or a table's header.
struct At<'a> {
    file: &'a Path,
    text: &'a str,
}

impl At<'_> {
    fn line(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())].matches('\n').count() + 1
    }

    fn err(&self, offset: Option<usize>, message: impl Into<String>) -> RecipeError {
        RecipeError {
            file: self.file.to_path_buf(),
            line: offset.map(|o| self.line(o)),
            message: message.into(),
        }
    }

    fn at<T>(&self, spanned: &Spanned<T>, message: impl Into<String>) -> RecipeError {
        self.err(Some(spanned.span().start), message)
    }

    /// The line of `[table]`.
    fn header(&self, table: &str, message: impl Into<String>) -> RecipeError {
        let header = format!("[{table}]");
        let line = self.text.lines().position(|l| l.trim() == header).map(|i| i + 1);
        RecipeError { file: self.file.to_path_buf(), line, message: message.into() }
    }
}

fn simple_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn exact_version(v: &str) -> bool {
    let (core, _) = v.split_once(['-', '+']).unwrap_or((v, ""));
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `30s`, `2m`: from one second to ten minutes.
fn duration(text: &str) -> Option<Duration> {
    let (digits, unit) = text.split_at(text.len().checked_sub(1)?);
    let n: u64 = digits.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60)?,
        _ => return None,
    };
    (1..=600).contains(&secs).then(|| Duration::from_secs(secs))
}

fn source(at: &At, table: toml::Table, opts: &LoadOptions) -> Result<(SourceKind, SourceSpec), RecipeError> {
    let raw = RawSource::deserialize(toml::Value::Table(table))
        .map_err(|e| at.header("source", format!("[source]: {}", e.message())))?;
    let bad = |m: String| Err(at.header("source", m));
    Ok(match raw {
        RawSource::Brew { formula } => {
            if formula.is_empty() || !formula.split('/').all(simple_name) || formula.split('/').count() > 3 {
                return bad(format!("formula {formula:?} is not a brew formula name"));
            }
            (SourceKind::Brew, SourceSpec::Brew { formula })
        }
        RawSource::Path { program } => {
            if !Path::new(&program).is_absolute() {
                return bad(format!("program {program:?} must be an absolute path"));
            }
            (SourceKind::Path, SourceSpec::Path { program: program.into() })
        }
        RawSource::Download { version, url, sha256, strip } => {
            let file_ok = opts.allow_file_urls && url.starts_with("file://");
            if !url.starts_with("https://") && !file_ok {
                return bad(format!("url {url:?} must start with https://"));
            }
            if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return bad("sha256 must be 64 hex digits, the SHA-256 of the file at url".into());
            }
            if !simple_name(&version) {
                return bad(format!("version {version:?} may hold letters, digits, ., _ and - only"));
            }
            (
                SourceKind::Download,
                SourceSpec::Download { version, url, sha256: sha256.to_lowercase(), strip },
            )
        }
        RawSource::Npm { package, version, node } => {
            if !exact_version(&version) {
                return bad(format!("version {version:?} must be an exact version, like 1.6.1"));
            }
            let package_ok = !package.is_empty()
                && package
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"@/._-".contains(&b));
            if !package_ok {
                return bad(format!("package {package:?} is not an npm package name"));
            }
            if crate::node::parse_spec(&node).is_err() {
                return bad(format!("node {node:?} is not a Node version, like 22 or 22.11.0"));
            }
            (SourceKind::Npm, SourceSpec::Npm { package, version, node })
        }
    })
}

fn ports(
    at: &At,
    raw: BTreeMap<Spanned<String>, Spanned<i64>>,
) -> Result<Vec<(String, PortSpec)>, RecipeError> {
    let mut entries: Vec<_> = raw.into_iter().collect();
    entries.sort_by_key(|(name, _)| name.span().start);
    if !entries.iter().any(|(name, _)| name.get_ref() == "main") {
        return Err(at.header("ports", "[ports] needs a port named main, the engine's main port"));
    }
    if entries.len() > 8 {
        return Err(at.header("ports", "[ports] lists at most 8 ports"));
    }
    let mut out = Vec::new();
    for (name, port) in entries {
        let n = name.get_ref();
        if EngineKey::new(n).is_err() {
            return Err(
                at.at(&name, format!("port name {n:?}: a lowercase letter, then letters, digits and -"))
            );
        }
        let usual = *port.get_ref();
        let Some(usual) = u16::try_from(usual).ok().filter(|p| *p >= 1024) else {
            return Err(at.at(&port, format!("port {n} = {usual}: use a port from 1024 to 65535")));
        };
        let suffix = if n == "main" { String::new() } else { n.clone() };
        out.push((n.clone(), PortSpec { suffix, usual }));
    }
    // The main port first: the registry lists an instance's entries in this order.
    out.sort_by_key(|(n, _)| n != "main");
    Ok(out)
}

/// Reads one recipe. `file` is where it came from (its stem must be the key).
pub fn parse_recipe(
    text: &str,
    file: &Path,
    origin: Origin,
    opts: &LoadOptions,
) -> Result<RecipeEngine, RecipeError> {
    let at = At { file, text };
    let raw: Raw =
        toml::from_str(text).map_err(|e| at.err(e.span().map(|s| s.start), e.message().to_string()))?;

    if *raw.schema.get_ref() != SCHEMA {
        return Err(at.at(
            &raw.schema,
            format!("schema {} is not one this Yard reads (it reads schema {SCHEMA})", raw.schema.get_ref()),
        ));
    }
    let key_text = raw.key.get_ref();
    let key = EngineKey::new(key_text).map_err(|e| at.at(&raw.key, e))?;
    if key.is_reserved() {
        return Err(at.at(&raw.key, format!("{key} is reserved: Yard's own engines and emailulator use it")));
    }
    if YARD_COMMANDS.contains(&key.as_str()) {
        return Err(at.at(
            &raw.key,
            format!("{key} is a Yard command's name, so `yard {key}` could not reach the engine"),
        ));
    }
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    if stem != key.as_str() {
        let name = file.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        return Err(at.at(&raw.key, format!("the file {name} must be named {key}.toml, after its key")));
    }

    let (kind, source) = source(&at, raw.source, opts)?;
    let ports = ports(&at, raw.ports)?;
    let has_config = raw.run.config.is_some();
    let allowed = Allowed { kind, ports: &ports, has_config, include: false };
    let template = |s: &Spanned<String>, allowed: &Allowed| {
        Template::parse(s.get_ref(), allowed).map_err(|m| at.at(s, m))
    };
    let list = |list: &Spanned<Vec<String>>, what: &str| -> Result<Vec<Template>, RecipeError> {
        if list.get_ref().is_empty() {
            return Err(at.at(list, format!("{what} is empty")));
        }
        let parsed: Vec<Template> = list
            .get_ref()
            .iter()
            .map(|s| Template::parse(s, &allowed))
            .collect::<Result<_, _>>()
            .map_err(|m| at.at(list, m))?;
        let first = parsed[0].0.first();
        if !matches!(first, Some(Piece::Var(Var::Home | Var::Program | Var::Node, false))) {
            return Err(at.at(
                list,
                format!(
                    "{what} must start with {{home}}, {{program}} or {{node}}: Yard runs nothing from PATH"
                ),
            ));
        }
        Ok(parsed)
    };

    let command = list(&raw.run.command, "run.command")?;
    let init = raw.run.init.as_ref().map(|i| list(i, "run.init")).transpose()?;
    let env = raw
        .run
        .env
        .iter()
        .map(|(k, v)| Ok((k.clone(), template(v, &allowed)?)))
        .collect::<Result<Vec<_>, RecipeError>>()?;
    let stop_signal = match raw.run.stop_signal.as_ref() {
        None => SIGTERM,
        Some(s) => match s.get_ref().as_str() {
            "TERM" => SIGTERM,
            "INT" => SIGINT,
            "QUIT" => SIGQUIT,
            other => return Err(at.at(s, format!("stop_signal {other}: use TERM, INT or QUIT"))),
        },
    };
    let start_timeout = raw
        .run
        .start_timeout
        .as_ref()
        .map(|s| {
            duration(s.get_ref()).ok_or_else(|| {
                at.at(
                    s,
                    format!(
                        "start_timeout {}: use seconds or minutes, like 30s or 2m (at most 10m)",
                        s.get_ref()
                    ),
                )
            })
        })
        .transpose()?;
    let dirs = raw
        .run
        .dirs
        .iter()
        .map(|d| {
            if simple_name(d.get_ref()) {
                Ok(d.get_ref().clone())
            } else {
                Err(at.at(d, format!("dirs: {:?} must be a plain directory name", d.get_ref())))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let config = match raw.run.config.as_ref() {
        None => None,
        Some(c) => {
            if !simple_name(c.file.get_ref()) || c.file.get_ref() == "overrides.conf" {
                return Err(at.at(
                    &c.file,
                    format!(
                        "run.config.file {:?} must be a plain file name other than overrides.conf",
                        c.file.get_ref()
                    ),
                ));
            }
            let include = c.include.as_ref().map(|i| template(i, &allowed)).transpose()?;
            let text_allowed = Allowed { kind, ports: &ports, has_config, include: include.is_some() };
            Some(Config {
                comment: c.comment.clone().unwrap_or_else(|| "#".into()),
                file: c.file.get_ref().clone(),
                text: template(&c.text, &text_allowed)?,
                include,
                overrides: c.overrides.clone(),
            })
        }
    };
    let name_of_port = |s: &Spanned<String>, what: &str| match ports.iter().find(|(n, _)| n == s.get_ref()) {
        Some((_, spec)) => Ok(spec.suffix.clone()),
        None => Err(at.at(s, format!("{what} names no port {} in [ports]", s.get_ref()))),
    };
    let health = name_of_port(&raw.health.tcp, "health.tcp")?;
    let console = match raw.console.as_ref() {
        None => None,
        Some(c) => Some(Console {
            port: name_of_port(&c.port, "console.port")?,
            host_header: match c.host_header.as_ref().map(|h| h.get_ref().as_str()) {
                None | Some("client") => HostHeader::Client,
                Some("upstream") => HostHeader::Upstream,
                Some(other) => {
                    let h = c.host_header.as_ref().map_or(&c.port, |h| h);
                    return Err(at.at(h, format!("console.host_header {other}: use client or upstream")));
                }
            },
            nginx_overrides: false,
        }),
    };
    let env_lines = match raw.env.as_ref() {
        None => Vec::new(),
        Some(e) => e
            .lines
            .get_ref()
            .iter()
            .map(|l| Template::parse(l, &allowed))
            .collect::<Result<_, _>>()
            .map_err(|m| at.at(&e.lines, m))?,
    };
    let connect = match raw.connect.as_ref() {
        None => None,
        Some(c) => Some(ConnectSpec {
            url: template(&c.url, &allowed)?,
            shell: c.shell.as_ref().map(|s| list(s, "connect.shell")).transpose()?,
            clients: c.clients.clone(),
        }),
    };
    let account = raw
        .account
        .map_or(Account { user: None, password: None }, |a| Account { user: a.user, password: a.password });

    Ok(RecipeEngine {
        key: key.as_str().to_string(),
        name: raw.name,
        description: raw.description,
        origin,
        source,
        ports,
        command,
        env,
        stop_signal,
        start_timeout,
        dirs,
        init,
        sleep: raw.run.sleep,
        config,
        health,
        console,
        env_lines,
        account,
        connect,
    })
}

/// `PATH_NOT_ALLOWED` for a `path` program Yard will not run: one that is not absolute, or one
/// in Herd's, DBngin's or Postgres.app's folders, whose copies are theirs (recipes spec §5).
/// `home` is the user's home.
pub fn path_allowed(program: &Path, home: &Path) -> Result<(), YardError> {
    let refuse = |why: &str| {
        Err(YardError::new(
            ErrorCode::PathNotAllowed,
            format!("Yard will not run {}: {why}", program.display()),
        )
        .with_hint("install the program yourself, or use a brew or download recipe"))
    };
    if !program.is_absolute() {
        return refuse("a path recipe names its program by absolute path");
    }
    let theirs = [
        home.join("Herd"),
        home.join("Library/Application Support/Herd"),
        PathBuf::from("/Applications/Herd.app"),
        PathBuf::from("/Applications/DBngin.app"),
        home.join("Library/Application Support/com.tinyapp.DBngin"),
        PathBuf::from("/Applications/Postgres.app"),
    ];
    // `starts_with` compares whole components, so `~/Herdsman` is not `~/Herd`.
    match theirs.iter().find(|dir| program.starts_with(dir)) {
        Some(dir) => refuse(&format!("it is in {}, another tool's", dir.display())),
        None => Ok(()),
    }
}
