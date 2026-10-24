//! The `.yard` project file (project-file spec §4): how Yard serves the project it sits in,
//! from the project itself or from Yard's data root (managed-file spec §4).

use crate::config::located_error;
use crate::config::yard::{ExtName, parse_ext_name};
use crate::config::{ServicesConfig, SiteEntry, SitesConfig};
use crate::constraint::best_version;
use crate::services::{Engine, instance_name};
use crate::sites::detect_docroot;
use crate::{Paths, PhpVersion};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use toml::Spanned;
use yard_proto::{ErrorCode, Need, YardError};

/// The file's name, at the project root.
pub const FILE: &str = ".yard";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectFile {
    pub php: Option<PhpVersion>,
    /// Relative to the project, and inside it.
    pub docroot: Option<PathBuf>,
    pub secure: Option<bool>,
    pub extensions: Vec<ExtName>,
    /// `[services]`: the engines the project needs, shared with every other project.
    pub services: BTreeMap<Engine, ServiceWant>,
}

/// What a `[services]` value asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceWant {
    /// A release line (`"8.4"`, `"17"`).
    Line(String),
    /// `true`: any instance of the engine.
    Default,
}

/// Where the `.yard` a site is served from lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// `<project>/.yard`, the project's own.
    Committed,
    /// `<data root>/sites/<name>/.yard`.
    Managed,
}

/// The `.yard` a site is served from.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub file: PathBuf,
    pub kind: FileKind,
    pub parsed: Result<ProjectFile, YardError>,
}

impl Loaded {
    /// The file's settings, when it could be read and parsed.
    pub fn ok(&self) -> Option<&ProjectFile> {
        self.parsed.as_ref().ok()
    }
}

/// A managed `.yard` that exists for a site but is not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ignored {
    pub file: PathBuf,
    pub because: IgnoredBecause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoredBecause {
    /// The project has its own `.yard`, which always wins.
    Committed,
    /// Its `project` names another directory, or none.
    OtherProject(Option<PathBuf>),
}

/// The `.yard` each linked site is served from, by site name; a site without one has no
/// entry.
pub type Projects = BTreeMap<String, Loaded>;

/// By site name, a managed `.yard` that exists but is not used.
pub type IgnoredFiles = BTreeMap<String, Ignored>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    php: Option<PhpVersion>,
    docroot: Option<Spanned<PathBuf>>,
    secure: Option<bool>,
    #[serde(default)]
    extensions: Vec<Spanned<String>>,
    #[serde(default)]
    services: BTreeMap<Spanned<String>, Spanned<toml::Value>>,
    project: Option<Spanned<PathBuf>>,
}

/// `PROJECT_FILE_INVALID` at the line at fault for a file that does not parse, has an
/// unknown key, a docroot outside the project or with a `$` nginx would expand, an
/// extension that is not a name, or a `[services]` entry that is not an engine with one of
/// its lines or `true`.
pub fn parse(text: &str, file: &Path) -> Result<ProjectFile, YardError> {
    parse_as(text, file, FileKind::Committed).map(|(parsed, _)| parsed)
}

/// A managed `.yard`: what [`parse`] accepts, plus `project`, the absolute path of the
/// directory it was written for (`None` when the file names none).
pub fn parse_managed(text: &str, file: &Path) -> Result<(ProjectFile, Option<PathBuf>), YardError> {
    parse_as(text, file, FileKind::Managed)
}

fn parse_as(text: &str, file: &Path, kind: FileKind) -> Result<(ProjectFile, Option<PathBuf>), YardError> {
    let invalid = |span, message: &str| {
        located_error(
            ErrorCode::ProjectFileInvalid,
            file,
            text,
            span,
            message,
            format!("edit {}", file.display()),
        )
    };
    let raw: Raw = toml::from_str(text).map_err(|e| invalid(e.span(), e.message()))?;
    let docroot = match raw.docroot {
        // A cloned repository must not be able to serve `/` or a sibling directory.
        Some(d) if !inside(d.get_ref()) => {
            let message =
                format!("docroot {:?} must be a path inside the project, such as \"public\"", d.get_ref());
            return Err(invalid(Some(d.span()), &message));
        }
        // nginx expands `$` in `root`: one site's file must not fail `nginx -t` for all.
        Some(d) if d.get_ref().to_string_lossy().chars().any(|c| c == '$' || c.is_control()) => {
            let message = format!("docroot {:?} cannot contain $ or control characters", d.get_ref());
            return Err(invalid(Some(d.span()), &message));
        }
        d => d.map(Spanned::into_inner),
    };
    let extensions = raw
        .extensions
        .into_iter()
        .map(|name| {
            parse_ext_name(name.get_ref())
                .map_err(|e| invalid(Some(name.span()), &format!("extensions: {}", e.message)))
        })
        .collect::<Result<_, _>>()?;
    let services = parse_services(raw.services, &invalid)?;
    let project = match (raw.project, kind) {
        (Some(p), FileKind::Committed) => {
            return Err(invalid(Some(p.span()), "project is only for a managed .yard"));
        }
        (Some(p), FileKind::Managed) if !p.get_ref().is_absolute() => {
            let message = format!("project {:?} must be an absolute path", p.get_ref());
            return Err(invalid(Some(p.span()), &message));
        }
        (p, _) => p.map(Spanned::into_inner),
    };
    Ok((ProjectFile { php: raw.php, docroot, secure: raw.secure, extensions, services }, project))
}

/// `[services]`: keys are engines, values a release line of that engine or `true`.
fn parse_services(
    raw: BTreeMap<Spanned<String>, Spanned<toml::Value>>,
    invalid: &dyn Fn(Option<std::ops::Range<usize>>, &str) -> YardError,
) -> Result<BTreeMap<Engine, ServiceWant>, YardError> {
    let mut services = BTreeMap::new();
    for (key, value) in raw {
        let name = key.get_ref();
        let Some(engine) = Engine::from_key(name) else {
            let known: Vec<&str> = Engine::ALL.iter().map(|e| e.key()).collect();
            return Err(invalid(
                Some(key.span()),
                &format!("[services]: unknown service {name:?}; the services are {}", known.join(", ")),
            ));
        };
        let want = match value.get_ref() {
            toml::Value::Boolean(true) => ServiceWant::Default,
            toml::Value::String(line) if engine.valid_line(line) => ServiceWant::Line(line.clone()),
            other => {
                let expected = match engine.default_line() {
                    Some(line) => format!("a release line such as \"{line}\", or true"),
                    None => "true".to_string(),
                };
                let written = match other {
                    toml::Value::String(s) => format!("{s:?}"),
                    toml::Value::Integer(n) => n.to_string(),
                    toml::Value::Float(n) => n.to_string(),
                    toml::Value::Boolean(b) => b.to_string(),
                    _ => "that".to_string(),
                };
                return Err(invalid(
                    Some(value.span()),
                    &format!("[services]: {name} must be {expected}, not {written}"),
                ));
            }
        };
        services.insert(engine, want);
    }
    Ok(services)
}

fn inside(path: &Path) -> bool {
    path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// What `file` asks for that is missing, for a site on `v`. `has_ext` says whether a
/// module is present for `v`; while `v` is not installed it can only know what
/// `yard.toml` manages, so every other listed extension is a need until PHP is there.
pub fn needs(
    file: &ProjectFile,
    v: PhpVersion,
    installed: &[PhpVersion],
    has_ext: &dyn Fn(&str) -> bool,
) -> Vec<Need> {
    let php = v.to_string();
    let missing_php = (!installed.contains(&v)).then(|| Need::Php { php: php.clone() });
    let missing_exts = file
        .extensions
        .iter()
        .filter(|e| !has_ext(&e.module))
        .map(|e| Need::Extension { name: e.written.clone(), php: php.clone() });
    missing_php.into_iter().chain(missing_exts).collect()
}

/// The `[services]` entries of `file` with no instance in `services`. A line is met by
/// that exact instance, `true` by any instance of the engine.
pub fn service_needs(file: &ProjectFile, services: &ServicesConfig) -> Vec<Need> {
    file.services
        .iter()
        .filter_map(|(&engine, want)| {
            let (met, version) = match want {
                ServiceWant::Line(line) => {
                    (services.services.contains_key(&instance_name(engine, Some(line))), Some(line.clone()))
                }
                ServiceWant::Default => (services.services.values().any(|s| s.engine == engine), None),
            };
            (!met).then(|| Need::Service { engine: engine.key().to_string(), version })
        })
        .collect()
}

/// The first line `yard init` writes.
pub const HEADER: &str = "# .yard: how Yard serves this project. Commit it.\n";

/// What `yard init` can tell about a project, never guessed (project-file spec §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub php: Option<PhpVersion>,
    /// Relative to the project; `.` for its root.
    pub docroot: PathBuf,
    /// `None` leaves HTTPS to each teammate's `secure_by_default`.
    pub secure: Option<bool>,
}

/// `php`: the site's isolation, else the version that best meets `composer.json`'s
/// `require.php`, else none. `docroot`: the site's override, else the detected one.
/// `secure`: the site's own setting, else `true` under `secure_by_default`, else none, so
/// the file never changes how this machine serves the site. A docroot outside the project
/// is `INVALID_ARGUMENT`, since `.yard` cannot say it.
pub fn detect(
    entry: Option<&SiteEntry>,
    project: &Path,
    composer_json: Option<&str>,
    installed: &[PhpVersion],
    secure_by_default: bool,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Result<Detected, YardError> {
    let php = entry.and_then(|e| e.php).or_else(|| {
        let json: serde_json::Value = serde_json::from_str(composer_json?).ok()?;
        best_version(json.get("require")?.get("php")?.as_str()?, installed).map(|(v, _)| v)
    });
    let absolute = match entry.and_then(|e| e.docroot.as_ref()) {
        Some(d) if d.is_absolute() => d.clone(),
        Some(d) => project.join(d),
        None => detect_docroot(project, is_dir),
    };
    let docroot = match absolute.strip_prefix(project) {
        Ok(rel) if rel.as_os_str().is_empty() => PathBuf::from("."),
        Ok(rel) if inside(rel) => rel.to_path_buf(),
        _ => {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "the docroot {} is outside {}, which .yard cannot say",
                    absolute.display(),
                    project.display()
                ),
            )
            .with_hint("yard docroot <a path inside the project>"));
        }
    };
    let secure = entry.and_then(|e| e.secure).or(secure_by_default.then_some(true));
    Ok(Detected { php, docroot, secure })
}

/// What `yard init` writes for `[services]`: one commented line per engine, since Yard does
/// not read `.env` to guess them (M4b spec §9). The user uncomments what the project needs.
const SERVICES_BLOCK: &str =
    "\n# [services]\n# mysql = \"8.4\"\n# postgres = \"17\"\n# redis = true\n# minio = true\n";

/// A new `.yard`. `secure` is written only when detected: absent, HTTPS follows each
/// teammate's `secure_by_default`.
pub fn render(d: &Detected) -> String {
    let mut out = HEADER.to_string();
    if let Some(v) = d.php {
        out.push_str(&format!("php = \"{v}\"\n"));
    }
    out.push_str(&format!("docroot = {}\n", toml_edit::Value::from(d.docroot.display().to_string())));
    if let Some(on) = d.secure {
        out.push_str(&format!("secure = {on}\n"));
    }
    out.push_str(SERVICES_BLOCK);
    out
}

/// `existing` with the keys detection produced written over, keeping every other key and
/// comment: `php` and `secure` only when detected, `docroot` always.
pub fn rewrite(existing: &str, d: &Detected) -> Result<String, YardError> {
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|e: toml_edit::TomlError| {
        YardError::new(ErrorCode::ProjectFileInvalid, format!(".yard does not parse: {}", e.message()))
            .with_hint("fix it first, or delete it and run yard init")
    })?;
    if let Some(v) = d.php {
        doc["php"] = toml_edit::value(v.to_string());
    }
    doc["docroot"] = toml_edit::value(d.docroot.display().to_string());
    if let Some(on) = d.secure {
        doc["secure"] = toml_edit::value(on);
    }
    Ok(doc.to_string())
}

/// The `.yard` each linked site is served from, and the managed files that are not used
/// (managed-file spec §4). `read` gives `None` when there is no file, and an error when
/// there is one Yard cannot read. The project's own `.yard` wins as soon as `read` finds
/// anything there, usable or not; else the site's managed file, while its `project` is
/// the site's `path` exactly. A managed file that does not parse is the site's error.
pub fn load(
    sites: &SitesConfig,
    paths: &Paths,
    read: &dyn Fn(&Path) -> Option<Result<String, YardError>>,
) -> (Projects, IgnoredFiles) {
    let mut projects = Projects::new();
    let mut ignored = IgnoredFiles::new();
    for (name, entry) in &sites.sites {
        let Some(path) = entry.path.as_ref() else { continue };
        let committed = path.join(FILE);
        let managed = paths.managed_project_file(name);
        if let Some(text) = read(&committed) {
            let parsed = text.and_then(|text| parse(&text, &committed));
            projects.insert(name.clone(), Loaded { file: committed, kind: FileKind::Committed, parsed });
            if read(&managed).is_some() {
                ignored.insert(name.clone(), Ignored { file: managed, because: IgnoredBecause::Committed });
            }
            continue;
        }
        let Some(text) = read(&managed) else { continue };
        match text.and_then(|text| parse_managed(&text, &managed)) {
            Ok((parsed, Some(project))) if project == *path => {
                projects.insert(
                    name.clone(),
                    Loaded { file: managed, kind: FileKind::Managed, parsed: Ok(parsed) },
                );
            }
            Ok((_, project)) => {
                ignored.insert(
                    name.clone(),
                    Ignored { file: managed, because: IgnoredBecause::OtherProject(project) },
                );
            }
            Err(e) => {
                projects
                    .insert(name.clone(), Loaded { file: managed, kind: FileKind::Managed, parsed: Err(e) });
            }
        }
    }
    (projects, ignored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;
    use crate::config::SiteEntry;
    use crate::config::sites::parse_sites;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use yard_proto::{ErrorCode, Need, YardError};

    const FILE: &str = "/code/eepower/.yard";

    fn parse_ok(text: &str) -> ProjectFile {
        parse(text, Path::new(FILE)).unwrap()
    }

    fn modules(file: &ProjectFile) -> Vec<&str> {
        file.extensions.iter().map(|e| e.module.as_str()).collect()
    }

    #[test]
    fn parses_every_key() {
        let f = parse_ok(
            "# .yard: how Yard serves this project. Commit it.\nphp = \"7.3\"\ndocroot = \"public\"\nsecure = true\nextensions = [\"redis\", \"imagick\"]\n",
        );
        assert_eq!(f.php, Some("7.3".parse().unwrap()));
        assert_eq!(f.docroot, Some(PathBuf::from("public")));
        assert_eq!(f.secure, Some(true));
        assert_eq!(modules(&f), ["redis", "imagick"]);
    }

    #[test]
    fn empty_file_is_all_defaults() {
        assert_eq!(parse_ok(""), ProjectFile::default());
    }

    #[test]
    fn unknown_key_names_file_and_line() {
        let e = parse("php = \"7.3\"\nnode = \"20\"\n", Path::new(FILE)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert_eq!(e.data.as_ref().unwrap()["line"], 2);
        assert!(e.message.contains("node") && e.message.contains(FILE), "{}", e.message);
        assert_eq!(e.hint.as_deref(), Some(format!("edit {FILE}").as_str()));
    }

    #[test]
    fn unsupported_php_is_invalid() {
        let e = parse("php = \"5.6\"\n", Path::new(FILE)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert!(e.message.contains("7.0–8.5"), "{}", e.message);
    }

    #[test]
    fn docroot_must_stay_inside_the_project() {
        for bad in ["/", "/etc", "../other", "public/../../x"] {
            let e = parse(&format!("php = \"8.4\"\ndocroot = {bad:?}\n"), Path::new(FILE)).unwrap_err();
            assert_eq!(e.code, ErrorCode::ProjectFileInvalid, "{bad}");
            assert_eq!(e.data.unwrap()["line"], 2, "{bad}");
        }
        for ok in [".", "public", "web/dist"] {
            assert_eq!(parse_ok(&format!("docroot = {ok:?}\n")).docroot, Some(PathBuf::from(ok)));
        }
    }

    #[test]
    fn docroot_cannot_name_an_nginx_variable() {
        // nginx expands `$` inside `root`, so one site's file would fail `nginx -t` for all.
        for bad in ["public$foo", "x$http_host", "pub\tlic", "pub\nlic"] {
            let e = parse(&format!("docroot = {bad:?}\n"), Path::new(FILE)).unwrap_err();
            assert_eq!(e.code, ErrorCode::ProjectFileInvalid, "{bad:?}");
        }
    }

    #[test]
    fn extensions_are_names_not_paths() {
        for bad in ["/x/redis.so", "redis.so", "../x"] {
            let e = parse(&format!("extensions = [{bad:?}]\n"), Path::new(FILE)).unwrap_err();
            assert_eq!(e.code, ErrorCode::ProjectFileInvalid, "{bad}");
        }
        let f = parse_ok("extensions = [\"xdebug-2.9.8\"]\n");
        assert_eq!(modules(&f), ["xdebug"]);
        assert_eq!(f.extensions[0].written, "xdebug-2.9.8");
    }

    #[test]
    fn needs_table() {
        let v = |s: &str| s.parse::<crate::PhpVersion>().unwrap();
        let file = parse_ok("php = \"7.3\"\nextensions = [\"redis\"]\n");
        let php = Need::Php { php: "7.3".into() };
        let redis = Need::Extension { name: "redis".into(), php: "7.3".into() };
        // Missing PHP: what yard.toml manages for it is known, compiled-in modules are not.
        assert_eq!(needs(&file, v("7.3"), &[v("8.4")], &|_| false), [php.clone(), redis.clone()]);
        assert_eq!(needs(&file, v("7.3"), &[v("8.4")], &|m| m == "redis"), [php]);
        assert_eq!(needs(&file, v("7.3"), &[v("7.3")], &|m| m == "redis"), []);
        assert_eq!(needs(&file, v("7.3"), &[v("7.3")], &|_| false), [redis]);
        let pinned = parse_ok("extensions = [\"xdebug-2.9.8\"]\n");
        assert_eq!(
            needs(&pinned, v("8.4"), &[v("8.4")], &|_| false),
            [Need::Extension { name: "xdebug-2.9.8".into(), php: "8.4".into() }]
        );
        assert_eq!(needs(&pinned, v("8.4"), &[v("8.4")], &|m| m == "xdebug"), []);
    }

    fn v(s: &str) -> crate::PhpVersion {
        s.parse().unwrap()
    }

    fn service_err(text: &str) -> YardError {
        parse(text, Path::new(FILE)).unwrap_err()
    }

    #[test]
    fn project_services_parse() {
        let f = parse_ok(
            "php = \"8.4\"\n\n[services]\nmysql = \"8.4\"\nredis = true\npostgres = \"17\"\nminio = true\n",
        );
        assert_eq!(
            f.services,
            BTreeMap::from([
                (Engine::Mysql, ServiceWant::Line("8.4".into())),
                (Engine::Postgres, ServiceWant::Line("17".into())),
                (Engine::Redis, ServiceWant::Default),
                (Engine::Minio, ServiceWant::Default),
            ])
        );
        assert_eq!(parse_ok("[services]\n").services, BTreeMap::new());
        assert_eq!(parse_ok("").services, BTreeMap::new());
        assert_eq!(parse_ok("[services]\nmysql = true\n").services[&Engine::Mysql], ServiceWant::Default);
    }

    #[test]
    fn project_services_reject_what_the_spec_rejects() {
        let cases = [
            ("redis = \"8\"", "redis"),
            ("mysql = \"latest\"", "mysql"),
            ("mysql = \"8.4.13\"", "mysql"),
            ("postgres = \"17.2\"", "postgres"),
            ("mysql = 8.4", "mysql"),
            ("postgres = 17", "postgres"),
            ("minio = false", "minio"),
            ("mysql = false", "mysql"),
            ("mysql = { version = \"8.4\" }", "mysql"),
            ("mongo = true", "mongo"),
        ];
        for (line, key) in cases {
            let e = service_err(&format!("php = \"8.4\"\n\n[services]\n{line}\n"));
            assert_eq!(e.code, ErrorCode::ProjectFileInvalid, "{line}");
            assert_eq!(e.data.as_ref().unwrap()["line"], 4, "{line}: {}", e.message);
            assert!(e.message.contains(key) && e.message.contains(FILE), "{line}: {}", e.message);
            assert_eq!(e.hint.as_deref(), Some(format!("edit {FILE}").as_str()), "{line}");
        }
        // The line at fault is the entry's, not the table's.
        let e = service_err("[services]\nmysql = \"8.4\"\nredis = \"8\"\n");
        assert_eq!(e.data.as_ref().unwrap()["line"], 3);
        // A scalar where the table goes.
        assert_eq!(service_err("services = \"mysql\"\n").code, ErrorCode::ProjectFileInvalid);
    }

    #[test]
    fn service_needs_table() {
        let services = |text: &str| {
            crate::config::services::parse_services(Some(text), Path::new("/r/services.toml")).unwrap()
        };
        let mysql_80 = services(
            "[services.\"mysql-8.0\"]\nengine = \"mysql\"\nversion = \"8.0\"\nformula = \"mysql@8.0\"\n",
        );
        let none = ServicesConfig::default();
        let want = |text: &str| parse_ok(text);
        let need = |engine: &str, version: Option<&str>| Need::Service {
            engine: engine.into(),
            version: version.map(Into::into),
        };
        assert_eq!(
            service_needs(&want("[services]\nmysql = \"8.4\"\n"), &mysql_80),
            [need("mysql", Some("8.4"))]
        );
        assert_eq!(service_needs(&want("[services]\nmysql = \"8.0\"\n"), &mysql_80), []);
        assert_eq!(service_needs(&want("[services]\nmysql = true\n"), &mysql_80), []);
        assert_eq!(service_needs(&want("[services]\nredis = true\n"), &none), [need("redis", None)]);
        assert_eq!(
            service_needs(&want("[services]\npostgres = true\n"), &mysql_80),
            [need("postgres", None)]
        );
        // A stopped instance still meets the need (spec 9).
        let stopped = services("[services.redis]\nengine = \"redis\"\nformula = \"redis\"\nstopped = true\n");
        assert_eq!(service_needs(&want("[services]\nredis = true\n"), &stopped), []);
        // Several at once come out in engine order.
        assert_eq!(
            service_needs(&want("[services]\nminio = true\nredis = true\nmysql = \"8.4\"\n"), &none),
            [need("mysql", Some("8.4")), need("redis", None), need("minio", None)]
        );
        assert_eq!(service_needs(&ProjectFile::default(), &none), []);
    }

    /// Spec §9's last bullet: one commented line per engine, since Yard does not read `.env`.
    const BLOCK: &str =
        "\n# [services]\n# mysql = \"8.4\"\n# postgres = \"17\"\n# redis = true\n# minio = true\n";

    #[test]
    fn render_for_eepower() {
        let d = Detected { php: Some(v("7.3")), docroot: "public".into(), secure: None };
        assert_eq!(
            render(&d),
            format!(
                "# .yard: how Yard serves this project. Commit it.\nphp = \"7.3\"\ndocroot = \"public\"\n{BLOCK}"
            )
        );
        let off = Detected { php: None, docroot: ".".into(), secure: Some(false) };
        assert!(
            render(&off).ends_with(&format!("docroot = \".\"\nsecure = false\n{BLOCK}")),
            "{}",
            render(&off)
        );
        let d = Detected { php: None, docroot: ".".into(), secure: Some(true) };
        assert_eq!(
            render(&d),
            format!(
                "# .yard: how Yard serves this project. Commit it.\ndocroot = \".\"\nsecure = true\n{BLOCK}"
            )
        );
        // The block is all comments: it parses and asks for nothing.
        assert_eq!(parse(&render(&d), Path::new(FILE)).unwrap().services, BTreeMap::new());
        // Uncommenting it is what the user does next.
        let on = render(&d)
            .replace("# [services]", "[services]")
            .replace("# mysql", "mysql")
            .replace("# redis", "redis");
        let f = parse(&on, Path::new(FILE)).unwrap();
        assert_eq!(f.services[&Engine::Mysql], ServiceWant::Line("8.4".into()));
        assert_eq!(f.services[&Engine::Redis], ServiceWant::Default);
    }

    #[test]
    fn detect_table() {
        let dirs = ["/p/public"];
        let is_dir = |p: &Path| dirs.iter().any(|d| Path::new(d) == p);
        let project = Path::new("/p");
        let composer = Some(r#"{"require": {"php": "^7.3"}}"#);
        // Isolation wins over composer.json.
        let entry = SiteEntry { php: Some(v("7.1")), ..Default::default() };
        let d = detect(Some(&entry), project, composer, &[v("7.1")], false, &is_dir).unwrap();
        assert_eq!(d, Detected { php: Some(v("7.1")), docroot: "public".into(), secure: None });
        // composer.json: the lowest supported fit when none installed fits.
        let d = detect(None, project, composer, &[v("8.4")], false, &is_dir).unwrap();
        assert_eq!(d.php, Some(v("7.3")));
        assert_eq!(detect(None, project, None, &[v("8.4")], false, &is_dir).unwrap().php, None);
        // The root is the docroot when there is no public/web/public_html.
        let d = detect(None, Path::new("/q"), None, &[], true, &is_dir).unwrap();
        assert_eq!((d.docroot, d.secure), (PathBuf::from("."), Some(true)));
        // Overrides: relative kept, absolute inside made relative, outside refused.
        let over = |d: &str| SiteEntry { docroot: Some(d.into()), secure: Some(false), ..Default::default() };
        // An explicit unsecure under secure_by_default is kept, so serving does not change.
        let d = detect(Some(&over("web")), project, None, &[], true, &is_dir).unwrap();
        assert_eq!((d.docroot, d.secure), (PathBuf::from("web"), Some(false)));
        assert_eq!(
            detect(Some(&over("/p/dist")), project, None, &[], false, &is_dir).unwrap().docroot,
            PathBuf::from("dist")
        );
        for outside in ["/elsewhere/www", "../x"] {
            let e = detect(Some(&over(outside)), project, None, &[], false, &is_dir).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{outside}");
        }
    }

    #[test]
    fn rewrite_keeps_extensions_and_comments() {
        let existing = "# ours\nphp = \"7.1\" # legacy\nextensions = [\"redis\"]\ndocroot = \"web\"\n";
        let d = Detected { php: Some(v("7.3")), docroot: "public".into(), secure: None };
        let out = rewrite(existing, &d).unwrap();
        let f = parse(&out, Path::new(FILE)).unwrap();
        assert_eq!((f.php, f.docroot.as_deref()), (Some(v("7.3")), Some(Path::new("public"))));
        assert_eq!(modules(&f), ["redis"]);
        assert!(out.starts_with("# ours\n"), "{out}");
        // A key detection does not produce is kept.
        let kept = rewrite(
            "php = \"7.1\"\nsecure = true\n",
            &Detected { php: None, docroot: ".".into(), secure: None },
        )
        .unwrap();
        let f = parse(&kept, Path::new(FILE)).unwrap();
        assert_eq!((f.php, f.secure), (Some(v("7.1")), Some(true)));
        let off = Detected { php: None, docroot: ".".into(), secure: Some(false) };
        let f = parse(&rewrite("secure = true\n", &off).unwrap(), Path::new(FILE)).unwrap();
        assert_eq!(f.secure, Some(false));
        assert_eq!(rewrite("php = [", &d).unwrap_err().code, ErrorCode::ProjectFileInvalid);
    }

    #[test]
    fn load_reads_only_linked_sites() {
        let sites = parse_sites(
            Some(
                "[sites.api]\nproxy = \"http://127.0.0.1:3000\"\n[sites.bare]\npath = \"/code/bare\"\n[sites.ee]\npath = \"/code/ee\"\n",
            ),
            Path::new("/r/sites.toml"),
        )
        .unwrap();
        let files = BTreeMap::from([
            (PathBuf::from("/code/ee/.yard"), "php = \"7.3\"\n".to_string()),
            (PathBuf::from("/code/.yard"), "php = \"7.1\"\n".to_string()),
        ]);
        let (projects, ignored) = load(&sites, &Paths::new("/r"), &|p: &Path| files.get(p).cloned().map(Ok));
        assert_eq!(projects.keys().collect::<Vec<_>>(), ["ee"]);
        assert_eq!(projects["ee"].ok().unwrap().php, Some("7.3".parse().unwrap()));
        assert!(ignored.is_empty());
    }

    const MANAGED: &str = "/r/sites/ee/.yard";

    #[test]
    fn project_key_is_only_for_a_managed_file() {
        let e = parse("project = \"/code/ee\"\n", Path::new(FILE)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert_eq!(e.data.as_ref().unwrap()["line"], 1);
        assert!(e.message.contains("project is only for a managed .yard"), "{}", e.message);
        let (f, project) =
            parse_managed("php = \"7.3\"\nproject = \"/code/ee\"\n", Path::new(MANAGED)).unwrap();
        assert_eq!((f.php, project), (Some(v("7.3")), Some(PathBuf::from("/code/ee"))));
        let (_, none) = parse_managed("php = \"7.3\"\n", Path::new(MANAGED)).unwrap();
        assert_eq!(none, None);
        let e = parse_managed("project = \"code/ee\"\n", Path::new(MANAGED)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert!(e.message.contains(MANAGED), "{}", e.message);
    }

    type Files = BTreeMap<PathBuf, Result<String, YardError>>;

    fn load_files(sites_toml: &str, files: &Files) -> (Projects, IgnoredFiles) {
        let sites = parse_sites(Some(sites_toml), Path::new("/r/sites.toml")).unwrap();
        load(&sites, &Paths::new("/r"), &|p: &Path| files.get(p).cloned())
    }

    fn managed(project: &str, php: &str) -> Result<String, YardError> {
        Ok(format!(
            "# .yard for x, kept by Yard outside the project.\nphp = {php:?}\nproject = {project:?}\n"
        ))
    }

    #[test]
    fn load_picks_committed_then_a_matching_managed_file() {
        let sites = "[sites.ee]\npath = \"/code/ee\"\n[sites.app]\npath = \"/code/app\"\n\
            [sites.other]\npath = \"/code/other\"\n[sites.broken]\npath = \"/code/broken\"\n\
            [sites.api]\nproxy = \"http://127.0.0.1:3000\"\n[sites.bare]\npath = \"/code/bare\"\n";
        let dangling =
            YardError::new(ErrorCode::ProjectFileInvalid, "/code/broken/.yard is a symlink to nothing");
        let files: Files = BTreeMap::from([
            (PathBuf::from("/code/ee/.yard"), Ok("php = \"7.3\"\n".to_string())),
            (PathBuf::from("/r/sites/ee/.yard"), managed("/code/ee", "7.4")),
            (PathBuf::from("/r/sites/app/.yard"), managed("/code/app", "7.4")),
            (PathBuf::from("/r/sites/other/.yard"), managed("/code/elsewhere", "7.4")),
            (PathBuf::from("/code/broken/.yard"), Err(dangling)),
            (PathBuf::from("/r/sites/broken/.yard"), managed("/code/broken", "7.4")),
            (PathBuf::from("/r/sites/api/.yard"), managed("/code/api", "7.4")),
        ]);
        let (p, ignored) = load_files(sites, &files);
        assert_eq!(p.keys().collect::<Vec<_>>(), ["app", "broken", "ee"]);
        assert_eq!((p["ee"].kind, p["ee"].ok().unwrap().php), (FileKind::Committed, Some(v("7.3"))));
        assert_eq!(p["ee"].file, PathBuf::from("/code/ee/.yard"));
        assert_eq!(ignored["ee"].file, PathBuf::from("/r/sites/ee/.yard"));
        assert_eq!(ignored["ee"].because, IgnoredBecause::Committed);
        assert_eq!(
            (p["app"].kind, p["app"].file.as_path()),
            (FileKind::Managed, Path::new("/r/sites/app/.yard"))
        );
        assert_eq!(p["app"].ok().unwrap().php, Some(v("7.4")));
        assert!(!ignored.contains_key("app"));
        // Another project's file is not used: the site resolves as if it had none.
        assert_eq!(ignored["other"].because, IgnoredBecause::OtherProject(Some("/code/elsewhere".into())));
        // An unusable committed file never falls through to the managed one.
        assert_eq!(p["broken"].kind, FileKind::Committed);
        assert!(p["broken"].ok().is_none() && p["broken"].parsed.is_err());
        assert_eq!(ignored["broken"].because, IgnoredBecause::Committed);
        // A proxy has no project, so its managed file is never read.
        assert!(!p.contains_key("api") && !ignored.contains_key("api"));
    }

    #[test]
    fn a_managed_file_without_project_is_not_used() {
        let files: Files =
            BTreeMap::from([(PathBuf::from("/r/sites/ee/.yard"), Ok("php = \"7.4\"\n".to_string()))]);
        let (p, ignored) = load_files("[sites.ee]\npath = \"/code/ee\"\n", &files);
        assert!(p.is_empty());
        assert_eq!(ignored["ee"].because, IgnoredBecause::OtherProject(None));
    }

    #[test]
    fn a_broken_managed_file_is_the_sites_error() {
        let files: Files =
            BTreeMap::from([(PathBuf::from("/r/sites/ee/.yard"), Ok("php = 7.3\n".to_string()))]);
        let (p, _) = load_files("[sites.ee]\npath = \"/code/ee\"\n", &files);
        assert_eq!(p["ee"].kind, FileKind::Managed);
        let e = p["ee"].parsed.as_ref().unwrap_err();
        assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
        assert!(e.message.contains(MANAGED), "{}", e.message);
    }
}
