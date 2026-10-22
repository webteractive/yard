//! The `.yard` project file (project-file spec §4): how Yard serves the project it sits in.

use crate::PhpVersion;
use crate::config::SitesConfig;
use crate::config::located_error;
use crate::config::yard::{ExtName, parse_ext_name};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use toml::Spanned;
use yard_proto::{ErrorCode, YardError};

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
    use crate::config::sites::parse_sites;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use yard_proto::ErrorCode;

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
