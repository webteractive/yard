//! Runtime recipes (runtimes spec §5): a runtime read from a TOML file. Parsing checks
//! everything a recipe can get wrong before a shim runs it, and reports the file and line;
//! expansion then cannot fail. Pure: the caller reads the file.

use super::Origin;
use super::version::RuntimeVersion;
use crate::services::engines::LoadOptions;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use toml::Spanned;
use yard_proto::{ErrorCode, YardError};

/// The only runtime recipe format this Yard reads.
pub const SCHEMA: i64 = 1;

/// The names a shim may never take: Yard's own programs.
const YARD_PROGRAMS: [&str; 3] = ["yard", "yardd", "yard-shim"];

/// A runtime, as its recipe defines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRecipe {
    pub key: String,
    pub name: String,
    pub description: String,
    pub origin: Origin,
    pub driver: Option<Driver>,
    pub pin_files: Vec<String>,
    pub uses_when: Vec<String>,
    pub runs_on: Option<RunsOn>,
    /// Whether the shims ask wake.sock for the site's sleeping services first; a shim's own
    /// `wake` overrides it.
    pub wake: bool,
    pub requires: Vec<Requires>,
    pub shims: BTreeMap<String, ShimDef>,
    /// Directories under `{home}` put first on the child's PATH.
    pub path: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub source: SourceDef,
    pub versions: BTreeMap<RuntimeVersion, VersionEntry>,
}

/// The code a carried recipe hands its runtime to (a closed set, spec §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Php,
}

/// The runtime a recipe's programs run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunsOn {
    Php,
    Node,
}

/// A `[[requires]]` rule: versions from `from` on need at least `php`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requires {
    pub from: RuntimeVersion,
    pub php: RuntimeVersion,
}

/// One shim: what it execs (placeholders unexpanded), the arguments Yard refuses, and its
/// own `wake` when it differs from the recipe's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShimDef {
    pub exec: Vec<String>,
    pub refuse: Vec<Refuse>,
    pub wake: Option<bool>,
}

/// Arguments a shim refuses (`composer self-update`), with the hint it prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refuse {
    pub args: Vec<String>,
    pub hint: String,
}

/// Where a runtime's versions come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDef {
    /// Downloaded into `installs/<key>/<version>/`; `file` names a bare (non-archive) download.
    Download { url: String, strip: u32, file: Option<String>, index: Option<IndexDef> },
    /// Node's, through brew's fnm (carried only).
    Fnm,
    /// PHP's, from brew's kegs (carried only).
    Brew,
}

/// A vendor's version index (carried recipes only, spec §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    pub kind: IndexKind,
    pub url: String,
    pub sha256_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    /// getcomposer.org's `versions` JSON.
    Composer,
}

/// A `[versions]` entry: a download checked against `sha256`, or a program already at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionEntry {
    Sha256(String),
    Path(PathBuf),
}

/// Why a runtime recipe did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeError {
    pub file: PathBuf,
    /// 1-based, when Yard can tell.
    pub line: Option<usize>,
    pub message: String,
}

impl RecipeError {
    /// `RUNTIME_RECIPE_INVALID`, `<file>:<line>: <why>`, hinting at the file.
    pub fn to_yard_error(&self) -> YardError {
        let at = match self.line {
            Some(line) => format!("{}:{line}", self.file.display()),
            None => self.file.display().to_string(),
        };
        YardError::new(ErrorCode::RuntimeRecipeInvalid, format!("{at}: {}", self.message))
            .with_hint(format!("fix {}", self.file.display()))
            .with_data(serde_json::json!({ "file": self.file.display().to_string(), "line": self.line }))
    }
}

/// The values a recipe's placeholders take for one version (spec §5.1).
#[derive(Debug, Clone, Copy)]
pub struct Vars<'a> {
    pub home: &'a Path,
    pub version: &'a str,
    pub cache: &'a Path,
    pub root: &'a Path,
    pub php: Option<&'a Path>,
    pub node: Option<&'a Path>,
}

impl Vars<'_> {
    fn get(&self, name: &str) -> Option<String> {
        let path = |p: &Path| p.display().to_string();
        match name {
            "home" => Some(path(self.home)),
            "version" => Some(self.version.to_string()),
            "cache" => Some(path(self.cache)),
            "root" => Some(path(self.root)),
            "php" => self.php.map(path),
            "node" => self.node.map(path),
            _ => None,
        }
    }
}

/// The `{name}` placeholders in `s`, as byte ranges with their names.
fn placeholders(s: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(open) = s[i..].find('{').map(|o| o + i) {
        let Some(close) = s[open..].find('}').map(|c| c + open) else { break };
        let name = &s[open + 1..close];
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            out.push((open, close + 1, name));
            i = close + 1;
        } else {
            i = open + 1;
        }
    }
    out
}

/// Fills each placeholder once, as a whole value never re-read. Load checks guarantee every
/// placeholder a recipe uses has a value where it is used; one without is left as written.
pub fn expand(template: &str, vars: &Vars) -> String {
    fill(template, &|name| vars.get(name))
}

/// `{version}` in a URL.
pub fn expand_version(template: &str, version: &str) -> String {
    fill(template, &|name| (name == "version").then(|| version.to_string()))
}

fn fill(template: &str, value: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (start, end, name) in placeholders(template) {
        if let Some(v) = value(name) {
            out.push_str(&template[last..start]);
            out.push_str(&v);
            last = end;
        }
    }
    out.push_str(&template[last..]);
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema: Spanned<i64>,
    key: Spanned<String>,
    name: String,
    #[serde(default)]
    description: String,
    driver: Option<Spanned<String>>,
    pin_files: Option<Spanned<Vec<String>>>,
    uses_when: Option<Spanned<Vec<String>>>,
    runs_on: Option<Spanned<String>>,
    #[serde(default)]
    wake: bool,
    path: Option<Spanned<Vec<String>>>,
    #[serde(default)]
    env: BTreeMap<Spanned<String>, Spanned<String>>,
    /// Read only to refuse it: a default is the version written in `yard.toml`
    /// (explicit-defaults spec §2.1).
    #[serde(rename = "default")]
    defaults: Option<Spanned<toml::Value>>,
    #[serde(default)]
    requires: Vec<Spanned<RawRequires>>,
    #[serde(default)]
    shims: BTreeMap<Spanned<String>, Spanned<RawShim>>,
    source: toml::Table,
    #[serde(default)]
    versions: BTreeMap<Spanned<String>, Spanned<RawVersion>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequires {
    from: String,
    php: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShim {
    exec: Option<Spanned<Vec<String>>>,
    #[serde(default)]
    refuse: Vec<RawRefuse>,
    wake: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRefuse {
    args: Vec<String>,
    hint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVersion {
    sha256: Option<String>,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum RawSource {
    Download {
        url: String,
        #[serde(default)]
        strip: u32,
        file: Option<String>,
        index: Option<toml::Table>,
    },
    Fnm {},
    Brew {},
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndex {
    kind: String,
    url: String,
    sha256_url: String,
}

/// Reports against one file, with lines from byte offsets or a table's header.
struct At<'a> {
    file: &'a Path,
    text: &'a str,
}

impl At<'_> {
    fn err(&self, offset: Option<usize>, message: impl Into<String>) -> RecipeError {
        let line = offset.map(|o| self.text[..o.min(self.text.len())].matches('\n').count() + 1);
        RecipeError { file: self.file.to_path_buf(), line, message: message.into() }
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

fn runtime_key(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn shim_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._+-".contains(c))
}

fn file_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\0')
}

/// A path relative to `{home}` that stays inside it.
fn inside(rel: &str) -> bool {
    !rel.is_empty() && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)))
}

fn url_ok(url: &str, opts: &LoadOptions) -> bool {
    url.starts_with("https://") || (opts.allow_file_urls && url.starts_with("file://"))
}

/// Reads one runtime recipe. `file` is where it came from (its stem must be the key).
pub fn parse_runtime_recipe(
    text: &str,
    file: &Path,
    origin: Origin,
    opts: &LoadOptions,
) -> Result<RuntimeRecipe, RecipeError> {
    let at = At { file, text };
    let raw: Raw =
        toml::from_str(text).map_err(|e| at.err(e.span().map(|s| s.start), e.message().to_string()))?;
    let carried = matches!(origin, Origin::Carried);

    if *raw.schema.get_ref() != SCHEMA {
        return Err(at.at(
            &raw.schema,
            format!("schema {} is not one this Yard reads (it reads schema {SCHEMA})", raw.schema.get_ref()),
        ));
    }
    let key = raw.key.get_ref().clone();
    if !runtime_key(&key) {
        return Err(at.at(&raw.key, format!("key {key:?}: a lowercase letter, then letters, digits and -")));
    }
    if !carried && (key == "php" || key == "node") {
        return Err(at.at(
            &raw.key,
            format!("{key} is reserved: Yard's own {key} runtime needs its driver, which a user recipe cannot name"),
        ));
    }
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    if stem != key {
        let name = file.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        return Err(at.at(&raw.key, format!("the file {name} must be named {key}.toml, after its key")));
    }

    let driver = match &raw.driver {
        None => None,
        Some(d) if !carried => {
            return Err(at.at(d, format!("driver = {:?}: a user recipe cannot name a driver", d.get_ref())));
        }
        Some(d) if d.get_ref() == "php" => Some(Driver::Php),
        Some(d) => return Err(at.at(d, format!("driver {:?} is not one Yard has (php)", d.get_ref()))),
    };
    let runs_on = match &raw.runs_on {
        None => None,
        Some(r) if r.get_ref() == "php" => Some(RunsOn::Php),
        Some(r) if r.get_ref() == "node" => Some(RunsOn::Node),
        Some(r) => return Err(at.at(r, format!("runs_on {:?} must be php or node", r.get_ref()))),
    };

    let source = source(&at, raw.source, carried, opts)?;

    let names = |list: &Option<Spanned<Vec<String>>>, what: &str| -> Result<Vec<String>, RecipeError> {
        let Some(list) = list else { return Ok(vec![]) };
        match list.get_ref().iter().find(|n| !file_name(n)) {
            Some(bad) => Err(at.at(list, format!("{what}: {bad:?} must be a file name, without /"))),
            None => Ok(list.get_ref().clone()),
        }
    };
    let pin_files = names(&raw.pin_files, "pin_files")?;
    let uses_when = names(&raw.uses_when, "uses_when")?;
    let path = match &raw.path {
        None => vec!["bin".to_string()],
        Some(list) => match list.get_ref().iter().find(|d| !inside(d)) {
            Some(bad) => {
                return Err(at.at(list, format!("path: {bad:?} must be a directory inside {{home}}")));
            }
            None => list.get_ref().clone(),
        },
    };

    let allowed = |name: &str| match name {
        "home" | "version" | "cache" | "root" => Ok(()),
        "php" if runs_on == Some(RunsOn::Php) => Ok(()),
        "node" if runs_on == Some(RunsOn::Node) => Ok(()),
        "php" | "node" => Err(format!("{{{name}}} has no value here: it needs runs_on = \"{name}\"")),
        _ => Err(format!("unknown placeholder {{{name}}}")),
    };
    let template = |s: &str| -> Result<(), String> {
        placeholders(s).into_iter().try_for_each(|(_, _, name)| allowed(name))
    };

    let mut env = BTreeMap::new();
    for (k, v) in &raw.env {
        let name = k.get_ref();
        let ok = name.bytes().next().is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
            && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !ok {
            return Err(at.at(k, format!("env {name:?}: an upper-case variable name")));
        }
        template(v.get_ref()).map_err(|m| at.at(v, format!("env {name}: {m}")))?;
        env.insert(name.clone(), v.get_ref().clone());
    }

    let reserved_shims: BTreeSet<String> = if carried {
        BTreeSet::new()
    } else {
        super::Runtimes::carried().iter().filter(|r| r.key() != key).flat_map(|r| r.shims()).collect()
    };
    if raw.shims.is_empty() {
        return Err(at.err(None, "a recipe needs at least one [shims.<name>] table"));
    }
    let mut shims = BTreeMap::new();
    for (name, shim) in &raw.shims {
        let n = name.get_ref();
        if !shim_name(n) {
            return Err(at.at(name, format!("shim name {n:?}: lowercase letters, digits and . _ + -")));
        }
        if YARD_PROGRAMS.contains(&n.as_str()) || reserved_shims.contains(n) {
            return Err(at.at(name, format!("shim name {n} is taken by Yard or its own runtimes")));
        }
        let def = shim.get_ref();
        // Errors in exec point at it, or at the shim's table when exec is the default.
        let exec_at = |m: String| match &def.exec {
            Some(e) => at.at(e, m),
            None => at.at(shim, m),
        };
        let exec = def
            .exec
            .as_ref()
            .map(|e| e.get_ref().clone())
            .unwrap_or_else(|| vec![format!("{{home}}/bin/{n}")]);
        for arg in &exec {
            template(arg).map_err(|m| exec_at(format!("shims.{n}.exec: {m}")))?;
        }
        let first = exec.first().map(String::as_str).unwrap_or_default();
        let first_ok = (first == "{php}" && runs_on == Some(RunsOn::Php))
            || (first == "{node}" && runs_on == Some(RunsOn::Node))
            || first.strip_prefix("{home}/").is_some_and(inside);
        if !first_ok {
            return Err(exec_at(format!(
                "shims.{n}.exec must start with a program under {{home}}/, or {{php}} or {{node}} with runs_on"
            )));
        }
        let mut refuse = Vec::new();
        for r in &def.refuse {
            if r.args.is_empty() || r.hint.trim().is_empty() {
                return Err(at.at(shim, format!("shims.{n}.refuse: each entry needs args and a hint")));
            }
            refuse.push(Refuse { args: r.args.clone(), hint: r.hint.clone() });
        }
        shims.insert(n.clone(), ShimDef { exec, refuse, wake: def.wake });
    }

    let version = |s: &str, what: &str, offset: usize| -> Result<RuntimeVersion, RecipeError> {
        s.parse::<RuntimeVersion>().map_err(|e| at.err(Some(offset), format!("{what}: {e}")))
    };
    if let Some(d) = &raw.defaults {
        return Err(at.at(
            d,
            format!(
                "[[default]] is no longer read: a runtime's default is the exact version in yard.toml \
                 (yard runtime default {} <version>)",
                key
            ),
        ));
    }
    let mut requires = Vec::new();
    for r in &raw.requires {
        if runs_on != Some(RunsOn::Php) {
            return Err(at.at(r, "[[requires]] needs runs_on = \"php\""));
        }
        let rule = r.get_ref();
        requires.push(Requires {
            from: version(&rule.from, "[[requires]] from", r.span().start)?,
            php: version(&rule.php, "[[requires]] php", r.span().start)?,
        });
    }

    let mut versions = BTreeMap::new();
    for (v, entry) in &raw.versions {
        let parsed = v
            .get_ref()
            .parse::<RuntimeVersion>()
            .map_err(|e| at.at(v, format!("[versions]: {e}; name an exact version such as \"1.2.4\"")))?;
        let e = entry.get_ref();
        let item = match (&e.sha256, &e.path) {
            (Some(h), None) if h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
                VersionEntry::Sha256(h.to_lowercase())
            }
            (Some(_), None) => {
                return Err(at.at(entry, "sha256 must be 64 hex digits, the SHA-256 of the download"));
            }
            (None, Some(p)) if Path::new(p).is_absolute() => VersionEntry::Path(PathBuf::from(p)),
            (None, Some(p)) => return Err(at.at(entry, format!("path {p:?} must be an absolute path"))),
            _ => return Err(at.at(entry, "a [versions] entry needs either sha256 or path")),
        };
        versions.insert(parsed, item);
    }
    if let SourceDef::Download { index: None, .. } = &source
        && versions.is_empty()
    {
        return Err(at.header("source", "a download source needs [versions] entries or an index"));
    }

    Ok(RuntimeRecipe {
        key,
        name: raw.name,
        description: raw.description,
        origin,
        driver,
        pin_files,
        uses_when,
        runs_on,
        wake: raw.wake,
        requires,
        shims,
        path,
        env,
        source,
        versions,
    })
}

fn source(at: &At, table: toml::Table, carried: bool, opts: &LoadOptions) -> Result<SourceDef, RecipeError> {
    let raw = RawSource::deserialize(toml::Value::Table(table))
        .map_err(|e| at.header("source", format!("[source]: {}", e.message())))?;
    let bad = |m: String| Err(at.header("source", m));
    match raw {
        RawSource::Fnm {} | RawSource::Brew {} if !carried => {
            let kind = if matches!(raw, RawSource::Fnm {}) { "fnm" } else { "brew" };
            bad(format!("kind = \"{kind}\" is Yard's own; a user recipe's source is download"))
        }
        RawSource::Fnm {} => Ok(SourceDef::Fnm),
        RawSource::Brew {} => Ok(SourceDef::Brew),
        RawSource::Download { url, strip, file, index } => {
            if !url_ok(&url, opts) {
                return bad(format!("url {url:?} must start with https://"));
            }
            if placeholders(&url).iter().any(|(_, _, n)| *n != "version") {
                return bad(format!("url {url:?} may use only {{version}}"));
            }
            if let Some(f) = &file
                && !file_name(f)
            {
                return bad(format!("file {f:?} must be a file name, without /"));
            }
            let index = match index {
                None => None,
                Some(_) if !carried => {
                    return Err(at.header(
                        "source.index",
                        "[source.index]: only a recipe Yard carries may have an index; list [versions] instead",
                    ));
                }
                Some(t) => {
                    let raw = RawIndex::deserialize(toml::Value::Table(t))
                        .map_err(|e| at.header("source.index", format!("[source.index]: {}", e.message())))?;
                    if raw.kind != "composer" {
                        return Err(at.header(
                            "source.index",
                            format!("index kind {:?} is not one Yard has (composer)", raw.kind),
                        ));
                    }
                    for u in [&raw.url, &raw.sha256_url] {
                        if !url_ok(u, opts) {
                            return Err(
                                at.header("source.index", format!("url {u:?} must start with https://"))
                            );
                        }
                    }
                    Some(IndexDef { kind: IndexKind::Composer, url: raw.url, sha256_url: raw.sha256_url })
                }
            };
            Ok(SourceDef::Download { url, strip, file, index })
        }
    }
}
