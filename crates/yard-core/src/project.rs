//! The `.yard` project file (project-file spec §4): how Yard serves the project it sits in.

use crate::PhpVersion;
use crate::config::located_error;
use crate::config::yard::{ExtName, parse_ext_name};
use crate::config::{SiteEntry, SitesConfig};
use crate::constraint::best_version;
use crate::sites::detect_docroot;
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
}

/// Each linked site's `.yard`, by site name; a site without one has no entry.
pub type Projects = BTreeMap<String, Result<ProjectFile, YardError>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    php: Option<PhpVersion>,
    docroot: Option<Spanned<PathBuf>>,
    secure: Option<bool>,
    #[serde(default)]
    extensions: Vec<Spanned<String>>,
}

/// `PROJECT_FILE_INVALID` at the line at fault for a file that does not parse, has an
/// unknown key, a docroot outside the project or an extension that is not a name.
pub fn parse(text: &str, file: &Path) -> Result<ProjectFile, YardError> {
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
    Ok(ProjectFile { php: raw.php, docroot, secure: raw.secure, extensions })
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

/// The first line `yard init` writes.
pub const HEADER: &str = "# .yard: how Yard serves this project. Commit it.\n";

/// What `yard init` can tell about a project, never guessed (project-file spec §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub php: Option<PhpVersion>,
    /// Relative to the project; `.` for its root.
    pub docroot: PathBuf,
    pub secure: bool,
}

/// `php`: the site's isolation, else the version that best meets `composer.json`'s
/// `require.php`, else none. `docroot`: the site's override, else the detected one.
/// `secure`: the site's setting, else `secure_by_default`. A docroot outside the project
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
    let secure = entry.and_then(|e| e.secure).unwrap_or(secure_by_default);
    Ok(Detected { php, docroot, secure })
}

/// A new `.yard`. `secure` is written only when true: absent, HTTPS follows each
/// teammate's `secure_by_default`.
pub fn render(d: &Detected) -> String {
    let mut out = HEADER.to_string();
    if let Some(v) = d.php {
        out.push_str(&format!("php = \"{v}\"\n"));
    }
    out.push_str(&format!("docroot = {}\n", toml_edit::Value::from(d.docroot.display().to_string())));
    if d.secure {
        out.push_str("secure = true\n");
    }
    out
}

/// `existing` with the keys detection produced written over, keeping every other key and
/// comment: `php` only when detected, `docroot` always, `secure` only when true.
pub fn rewrite(existing: &str, d: &Detected) -> Result<String, YardError> {
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|e: toml_edit::TomlError| {
        YardError::new(ErrorCode::ProjectFileInvalid, format!(".yard does not parse: {}", e.message()))
            .with_hint("fix it first, or delete it and run yard init")
    })?;
    if let Some(v) = d.php {
        doc["php"] = toml_edit::value(v.to_string());
    }
    doc["docroot"] = toml_edit::value(d.docroot.display().to_string());
    if d.secure {
        doc["secure"] = toml_edit::value(true);
    }
    Ok(doc.to_string())
}

/// The `.yard` of each linked site. `read` gives `None` when there is no file, and an
/// error when there is one Yard cannot read.
pub fn load(sites: &SitesConfig, read: &dyn Fn(&Path) -> Option<Result<String, YardError>>) -> Projects {
    sites
        .sites
        .iter()
        .filter_map(|(name, entry)| {
            let file = entry.path.as_ref()?.join(FILE);
            let parsed = read(&file)?.and_then(|text| parse(&text, &file));
            Some((name.clone(), parsed))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteEntry;
    use crate::config::sites::parse_sites;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use yard_proto::{ErrorCode, Need};

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

    #[test]
    fn render_for_eepower() {
        let d = Detected { php: Some(v("7.3")), docroot: "public".into(), secure: false };
        assert_eq!(
            render(&d),
            "# .yard: how Yard serves this project. Commit it.\nphp = \"7.3\"\ndocroot = \"public\"\n"
        );
        let d = Detected { php: None, docroot: ".".into(), secure: true };
        assert_eq!(
            render(&d),
            "# .yard: how Yard serves this project. Commit it.\ndocroot = \".\"\nsecure = true\n"
        );
        assert!(parse(&render(&d), Path::new(FILE)).is_ok());
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
        assert_eq!(d, Detected { php: Some(v("7.1")), docroot: "public".into(), secure: false });
        // composer.json: the lowest supported fit when none installed fits.
        let d = detect(None, project, composer, &[v("8.4")], false, &is_dir).unwrap();
        assert_eq!(d.php, Some(v("7.3")));
        assert_eq!(detect(None, project, None, &[v("8.4")], false, &is_dir).unwrap().php, None);
        // The root is the docroot when there is no public/web/public_html.
        let d = detect(None, Path::new("/q"), None, &[], true, &is_dir).unwrap();
        assert_eq!((d.docroot, d.secure), (PathBuf::from("."), true));
        // Overrides: relative kept, absolute inside made relative, outside refused.
        let over = |d: &str| SiteEntry { docroot: Some(d.into()), secure: Some(false), ..Default::default() };
        let d = detect(Some(&over("web")), project, None, &[], true, &is_dir).unwrap();
        assert_eq!((d.docroot, d.secure), (PathBuf::from("web"), false));
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
        let d = Detected { php: Some(v("7.3")), docroot: "public".into(), secure: false };
        let out = rewrite(existing, &d).unwrap();
        let f = parse(&out, Path::new(FILE)).unwrap();
        assert_eq!((f.php, f.docroot.as_deref()), (Some(v("7.3")), Some(Path::new("public"))));
        assert_eq!(modules(&f), ["redis"]);
        assert!(out.starts_with("# ours\n"), "{out}");
        // A key detection does not produce is kept.
        let kept = rewrite(
            "php = \"7.1\"\nsecure = true\n",
            &Detected { php: None, docroot: ".".into(), secure: false },
        )
        .unwrap();
        let f = parse(&kept, Path::new(FILE)).unwrap();
        assert_eq!((f.php, f.secure), (Some(v("7.1")), Some(true)));
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
        let projects = load(&sites, &|p: &Path| files.get(p).cloned().map(Ok));
        assert_eq!(projects.keys().collect::<Vec<_>>(), ["ee"]);
        assert_eq!(projects["ee"].as_ref().unwrap().php, Some("7.3".parse().unwrap()));
    }
}
