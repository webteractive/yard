//! The engine contract (recipes spec §4): every service engine Yard runs implements
//! [`ServiceEngine`], and [`Engines`] is the set yardd, the CLI and Yard.app ask. MySQL,
//! PostgreSQL and MinIO are Rust implementations here; recipes (TOML) are the other kind.

mod builtin;
pub mod carried;
mod key;
pub mod recipe;

pub use key::{
    EngineKey, LineShape, LineSpec, MYSQL_LINES, POSTGRES_LINES, instance_name, parse_instance,
    versioned_lines,
};
pub use recipe::{LoadOptions, RecipeEngine, RecipeError, parse_recipe};

/// Every top-level `yard` command and alias: a recipe key may not be one, so `yard <key>`
/// always reaches the engine (recipes spec §9.1). A yard-cli test keeps it equal to the CLI.
pub const YARD_COMMANDS: &[&str] = &[
    "start",
    "stop",
    "status",
    "version",
    "uninstall",
    "self-update",
    "doctor",
    "sites",
    "link",
    "init",
    "unlink",
    "proxy",
    "unproxy",
    "docroot",
    "isolate",
    "unisolate",
    "secure",
    "unsecure",
    "trust",
    "untrust",
    "ports",
    "php",
    "node",
    "ext",
    "xdebug",
    "install-shims",
    "services",
    "service",
    "db",
    "env",
    "info",
    "sources",
    "source",
    "tunnel",
    "untunnel",
    "tunnels",
    "restart",
    "helper",
    "engines",
    "pgsql",
    "help",
];

use crate::Paths;
use crate::services::render::UnitCommand;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use yard_proto::{Account, PortRef};

/// One port an engine listens on: `suffix` names its registry entry (`""` is `<instance>`,
/// anything else `<instance>-<suffix>`), and `usual` is the port it is offered first. The
/// first is the engine's main port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub suffix: String,
    pub usual: u16,
}

/// Where an engine's program comes from (recipes spec §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    /// One brew formula, in full for a tap's (`minio/stable/minio`).
    Brew { formula: String },
    /// A brew formula per release line (MySQL, PostgreSQL).
    BrewLines(&'static LineSpec),
    /// A program the user installed.
    Path { program: PathBuf },
    /// A pinned download, checked against `sha256`.
    Download { version: String, url: String, sha256: String, strip: u32 },
    /// An exact npm package version, run on Yard's Node `node`.
    Npm { package: String, version: String, node: String },
}

/// Where an engine is defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    Carried,
    User(PathBuf),
}

/// Which database family `yard <engine> list|create|dump|import` speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFlavor {
    Mysql,
    Postgres,
}

/// The `Host` a console's nginx block sends upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostHeader {
    /// The upstream's own address (MinIO's console checks it).
    Upstream,
    /// The browser's.
    Client,
}

/// A web console at `<key>.<tld>`, proxied by nginx to the port named `port` (a suffix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Console {
    pub port: String,
    pub host_header: HostHeader,
    /// Whether the block includes `services/<i>/overrides.conf` (nginx directives; MinIO's).
    pub nginx_overrides: bool,
}

/// What a client needs to connect: a URL, a prompt to run (program first), the GUI clients
/// that take it, and a socket when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub url: String,
    pub shell: Option<Vec<OsString>>,
    pub clients: Vec<String>,
    pub socket: Option<PathBuf>,
}

/// One instance, as the engine sees it. `home` is the keg for a brew engine, the install root
/// for a download or npm one, and the program's directory for a path one.
#[derive(Debug, Clone, Copy)]
pub struct InstanceCtx<'a> {
    pub instance: &'a str,
    pub home: &'a Path,
    pub paths: &'a Paths,
    pub ports: &'a [PortRef],
    pub macos_user: &'a str,
    /// Yard's Node for an npm engine.
    pub node: Option<&'a Path>,
    /// The instance's own random secret (`services/<i>/secret`), for an engine whose recipe uses
    /// `{secret}`: an API or master key no other Mac and no web page knows.
    pub secret: Option<&'a str>,
}

impl InstanceCtx<'_> {
    /// The port of the entry with `suffix` (`""` is the main one), else its usual port.
    pub fn port(&self, engine: &dyn ServiceEngine, suffix: &str) -> u16 {
        if suffix.is_empty() {
            return main_port(engine, self.instance, self.ports);
        }
        let name = format!("{}-{suffix}", self.instance);
        self.ports.iter().find(|p| p.name == name).map_or_else(
            || engine.ports().into_iter().find(|p| p.suffix == suffix).map_or(0, |p| p.usual),
            |p| p.port,
        )
    }
}

/// The contract (recipes spec §4). Every fact Yard needs about an engine comes from here.
pub trait ServiceEngine: Send + Sync + Debug {
    /// The `.yard` `[services]` key and `services.toml`'s `engine`.
    fn key(&self) -> &str;
    /// What people see: `MySQL`, a recipe's `name`.
    fn display(&self) -> &str;
    fn description(&self) -> &str {
        ""
    }
    fn origin(&self) -> Origin;
    fn source(&self) -> SourceSpec;
    /// The release lines, for MySQL and PostgreSQL only.
    fn lines(&self) -> Option<&'static LineSpec> {
        None
    }
    fn ports(&self) -> Vec<PortSpec>;
    fn account(&self, macos_user: &str) -> Account;
    /// Directories under `services/<i>/` reconcile creates (Redis's `data`).
    fn dirs(&self) -> Vec<String> {
        Vec::new()
    }
    fn command(&self, ctx: &InstanceCtx) -> UnitCommand;
    /// The one-time command that creates the data, for the engines that need one.
    fn init(&self, _ctx: &InstanceCtx) -> Option<UnitCommand> {
        None
    }
    /// The generated config file and its text.
    fn config_file(&self, _ctx: &InstanceCtx) -> Option<(PathBuf, String)> {
        None
    }
    /// What `services/<i>/overrides.conf` starts as; `None` when the engine has none.
    fn user_overrides(&self) -> Option<String> {
        None
    }
    /// `None` is the supervisor's start grace.
    fn start_timeout(&self) -> Option<Duration> {
        None
    }
    fn may_sleep(&self) -> bool {
        true
    }
    /// The suffix of the port the health check connects to (`""`, the main port).
    fn health_port(&self) -> String {
        String::new()
    }
    fn env_lines(&self, ctx: &InstanceCtx) -> Vec<String>;
    fn console(&self) -> Option<Console> {
        None
    }
    fn databases(&self) -> Option<DbFlavor> {
        None
    }
    fn connect(&self, _ctx: &InstanceCtx) -> Option<Connect> {
        None
    }
    /// Whether [`connect`](Self::connect) says anything, whatever the instance.
    fn has_connect(&self) -> bool {
        false
    }
    /// Whether the engine uses the instance's secret, which yardd then makes.
    fn needs_secret(&self) -> bool {
        false
    }
}

/// The instance's main port: the entry named for the instance, else the first entry that is
/// not one of the engine's other ports, else the engine's usual main port.
pub fn main_port(engine: &dyn ServiceEngine, instance: &str, ports: &[PortRef]) -> u16 {
    let specs = engine.ports();
    let others: Vec<String> =
        specs.iter().filter(|p| !p.suffix.is_empty()).map(|p| format!("{instance}-{}", p.suffix)).collect();
    ports
        .iter()
        .find(|p| p.name == instance)
        .or_else(|| ports.iter().find(|p| !others.contains(&p.name)))
        .map_or_else(|| specs.first().map_or(0, |p| p.usual), |p| p.port)
}

/// The engines Yard knows, by key, and the user's recipes that did not load.
#[derive(Debug, Clone, Default)]
pub struct Engines {
    engines: BTreeMap<String, Arc<dyn ServiceEngine>>,
    /// Recipe files that did not load and left no engine behind.
    errors: Vec<RecipeError>,
    /// Recipe files that did not load while an engine of their key runs on: the file's last
    /// good version, or the carried recipe it shadowed (recipes spec §7).
    stale: BTreeMap<String, RecipeError>,
}

impl Engines {
    /// The engines in Yard's code and the recipes it carries.
    pub fn builtin() -> Engines {
        let mut engines = Engines::default();
        for engine in builtin::all().into_iter().chain(carried::engines()) {
            engines.engines.insert(engine.key().to_string(), engine);
        }
        engines
    }

    pub fn get(&self, key: &str) -> Option<&Arc<dyn ServiceEngine>> {
        self.engines.get(key)
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn ServiceEngine>> {
        self.engines.values()
    }

    /// Every key, sorted.
    pub fn keys(&self) -> Vec<String> {
        self.engines.keys().cloned().collect()
    }

    /// The recipe files that did not load and left no engine.
    pub fn errors(&self) -> &[RecipeError] {
        &self.errors
    }

    /// Why `key`'s recipe file did not load, while its engine runs on an older version.
    pub fn stale(&self, key: &str) -> Option<&RecipeError> {
        self.stale.get(key)
    }

    /// `base` (the built-ins and carried recipes) with the user's recipes, `user`, one result per
    /// file (recipes spec §7). A recipe replaces a carried one with its key; one that names a
    /// built-in's key is an error and changes nothing. A file that does not load keeps its key's
    /// last good version from `last_good` in force, else the carried recipe it shadowed, else
    /// leaves no engine. `last_good` is updated to this load: a file that loaded, or the version
    /// kept for a broken one; a key with no file any more is forgotten.
    pub fn with_user(
        base: &Engines,
        user: Vec<Result<RecipeEngine, RecipeError>>,
        last_good: &mut BTreeMap<String, Arc<RecipeEngine>>,
    ) -> Engines {
        let mut engines =
            Engines { engines: base.engines.clone(), errors: Vec::new(), stale: BTreeMap::new() };
        let mut kept = BTreeMap::new();
        for result in user {
            match result {
                Ok(recipe) => {
                    let recipe = Arc::new(recipe);
                    let key = recipe.key().to_string();
                    kept.insert(key.clone(), recipe.clone());
                    engines.engines.insert(key, recipe);
                }
                Err(e) => {
                    let key = e.file.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
                    let base_engine = base.engines.get(&key);
                    if let Some(good) = last_good.get(&key) {
                        kept.insert(key.clone(), good.clone());
                        engines.engines.insert(key.clone(), good.clone());
                        engines.stale.insert(key, e);
                    } else if base_engine.is_some_and(|b| b.origin() == Origin::Carried) {
                        engines.stale.insert(key, e);
                    } else {
                        engines.errors.push(e);
                    }
                }
            }
        }
        *last_good = kept;
        engines.refuse_port_entry_keys();
        engines
    }

    /// Refuses a user recipe whose key is another engine's port entry (`minio-console`,
    /// `typesense-peering`, or `cache-stack` beside a `cache` with a port named `stack`): its
    /// instance's main entry would be that one, and the two would share a port.
    fn refuse_port_entry_keys(&mut self) {
        let entries: Vec<(String, String)> = self
            .engines
            .values()
            .flat_map(|e| {
                let key = e.key().to_string();
                e.ports()
                    .into_iter()
                    .filter(|p| !p.suffix.is_empty())
                    .map(move |p| (format!("{key}-{}", p.suffix), key.clone()))
            })
            .collect();
        for (entry, owner) in entries {
            let Some(engine) = self.engines.get(&entry) else { continue };
            let Origin::User(file) = engine.origin() else { continue };
            self.engines.remove(&entry);
            self.stale.remove(&entry);
            self.errors.push(RecipeError {
                file,
                line: None,
                message: format!(
                    "{entry} is the {owner} engine's port entry {entry}: an instance named {entry} would share its port; \
                     give the recipe another key"
                ),
            });
        }
    }

    /// [`Engines::builtin`], built once: the carried recipes are parsed the first time only.
    pub fn builtin_shared() -> &'static Engines {
        static BUILTIN: std::sync::LazyLock<Engines> = std::sync::LazyLock::new(Engines::builtin);
        &BUILTIN
    }
}

/// What `yard <engine> <verb>` can do with an engine (recipes spec §9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Shell,
    List,
    Create,
    Dump,
    Import,
    Connect,
    Open,
}

impl Verb {
    pub const ALL: [Verb; 7] =
        [Verb::Shell, Verb::List, Verb::Create, Verb::Dump, Verb::Import, Verb::Connect, Verb::Open];

    pub fn name(self) -> &'static str {
        match self {
            Verb::Shell => "shell",
            Verb::List => "list",
            Verb::Create => "create",
            Verb::Dump => "dump",
            Verb::Import => "import",
            Verb::Connect => "connect",
            Verb::Open => "open",
        }
    }

    pub fn parse(word: &str) -> Option<Verb> {
        Verb::ALL.into_iter().find(|v| v.name() == word)
    }
}

/// The verbs `engine` has: a database engine all seven; one with connect details a prompt (when
/// it names one), `connect` and `open`; one with a console `open`.
pub fn engine_verbs(engine: &dyn ServiceEngine) -> Vec<Verb> {
    if engine.databases().is_some() {
        return Verb::ALL.to_vec();
    }
    let paths = Paths::new("/");
    let ctx = InstanceCtx {
        instance: engine.key(),
        home: Path::new("/"),
        paths: &paths,
        ports: &[],
        macos_user: "",
        secret: None,
        node: None,
    };
    let connect = engine.connect(&ctx);
    let mut verbs = vec![];
    if connect.as_ref().is_some_and(|c| c.shell.is_some()) {
        verbs.push(Verb::Shell);
    }
    if connect.is_some() {
        verbs.push(Verb::Connect);
    }
    if connect.is_some() || engine.console().is_some() {
        verbs.push(Verb::Open);
    }
    verbs
}

/// The database family of the engine with `key`, when it has one. Only built-ins do, and a
/// recipe can never take a built-in's key, so the built-in set answers for every Mac.
pub fn db_flavor(key: &str) -> Option<DbFlavor> {
    Engines::builtin_shared().get(key)?.databases()
}

#[cfg(test)]
mod recipe_tests;
#[cfg(test)]
mod tests;
