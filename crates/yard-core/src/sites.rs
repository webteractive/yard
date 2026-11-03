//! Turning `sites.toml` entries into served sites (spec §5.1). A directory is served
//! only when it is linked.

use crate::config::{SitesConfig, YardConfig};
use crate::project::{FileKind, IgnoredBecause, IgnoredFiles, Projects};
use std::path::{Component, Path, PathBuf};
use yard_proto::{ErrorCode, IgnoredManagedFile, ProjectFileKind, SiteInfo, SiteKind, SiteSource, YardError};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resolution {
    pub sites: Vec<SiteInfo>,
    pub warnings: Vec<String>,
}

/// The DNS label a directory name serves as: lowercased, spaces and `_` become `-`,
/// then only `[a-z0-9.-]`, starting and ending alphanumeric, with no `..`.
pub fn host_label(dir_name: &str) -> Option<String> {
    let label: String =
        dir_name.to_lowercase().chars().map(|c| if c == ' ' || c == '_' { '-' } else { c }).collect();
    let valid_chars =
        label.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    (valid_chars && alnum(label.chars().next()) && alnum(label.chars().last()) && !label.contains(".."))
        .then_some(label)
}

/// Whether `name`, a key of `sites.toml`, is a site name Yard serves: a host label as
/// `host_label` makes one. Only those name a directory under the data root, so a
/// hand-written `"/etc"` or `"../x"` never becomes a path.
pub fn is_site_name(name: &str) -> bool {
    host_label(name).as_deref() == Some(name)
}

pub fn detect_docroot(project: &Path, is_dir: &dyn Fn(&Path) -> bool) -> PathBuf {
    ["public", "web", "public_html"]
        .iter()
        .map(|d| project.join(d))
        .find(|p| is_dir(p))
        .unwrap_or_else(|| project.to_path_buf())
}

/// `(host, port)` of a proxy target; the port becomes an `external` registry entry.
pub fn parse_proxy_target(url: &str) -> Result<(String, u16), YardError> {
    let bad = || {
        YardError::new(ErrorCode::InvalidArgument, format!("invalid proxy target {url:?}"))
            .with_hint("use http://<host>:<port>, e.g. http://127.0.0.1:5173")
    };
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).ok_or_else(bad)?;
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = authority.rsplit_once(':').ok_or_else(bad)?;
    let port: u16 = port.parse().map_err(|_| bad())?;
    if host.is_empty() || port == 0 {
        return Err(bad());
    }
    Ok((host.to_string(), port))
}

/// `project` joined with a docroot relative to it, without a `.` component.
fn under(project: &Path, docroot: &Path) -> PathBuf {
    let rel: PathBuf = docroot.components().filter(|c| *c != Component::CurDir).collect();
    if rel.as_os_str().is_empty() { project.to_path_buf() } else { project.join(rel) }
}

/// Every served site. A linked site with a `.yard` in `projects` takes each setting from
/// that file, or the machine default where it is silent, never from `sites.toml`.
pub fn resolve(
    cfg: &YardConfig,
    sites: &SitesConfig,
    projects: &Projects,
    tld: &str,
    http_port: u16,
    https_port: u16,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Resolution {
    let mut warnings = Vec::new();
    let url = |host: &str, secure: bool| match (secure, http_port, https_port) {
        (false, 80, _) => format!("http://{host}/"),
        (false, p, _) => format!("http://{host}:{p}/"),
        (true, _, 443) => format!("https://{host}/"),
        (true, _, p) => format!("https://{host}:{p}/"),
    };
    let docroot_for = |project: &Path, over: Option<&PathBuf>| match over {
        Some(d) if d.is_absolute() => d.clone(),
        Some(d) => project.join(d),
        None => detect_docroot(project, is_dir),
    };

    let mut out = Vec::new();
    for (name, entry) in &sites.sites {
        if !is_site_name(name) {
            warnings.push(format!("sites.toml: {name:?} is not a valid host name; skipped"));
            continue;
        }
        let host = format!("{name}.{tld}");
        let secure = entry.secure.unwrap_or(cfg.secure_by_default);
        match (&entry.path, &entry.proxy) {
            (Some(_), Some(_)) => {
                warnings.push(format!("sites.toml: {name} has both path and proxy; skipped"));
            }
            (Some(path), None) if !is_dir(path) => {
                warnings.push(format!(
                    "sites.toml: linked site {name} points at {}, which is missing",
                    path.display()
                ));
            }
            (Some(path), None) => {
                let loaded = projects.get(name);
                let (php, docroot, secure, project_error) = match loaded.map(|l| &l.parsed) {
                    Some(Ok(file)) => (
                        file.php.unwrap_or(cfg.default_php),
                        file.docroot
                            .as_ref()
                            .map_or_else(|| detect_docroot(path, is_dir), |d| under(path, d)),
                        file.secure.unwrap_or(cfg.secure_by_default),
                        None,
                    ),
                    Some(Err(e)) => (
                        cfg.default_php,
                        detect_docroot(path, is_dir),
                        cfg.secure_by_default,
                        Some(e.clone()),
                    ),
                    None => (
                        entry.php.unwrap_or(cfg.default_php),
                        docroot_for(path, entry.docroot.as_ref()),
                        secure,
                        None,
                    ),
                };
                out.push(SiteInfo {
                    name: name.clone(),
                    url: url(&host, secure),
                    host,
                    source: SiteSource::Linked,
                    kind: SiteKind::Php {
                        path: path.display().to_string(),
                        docroot: docroot.display().to_string(),
                        php: php.to_string(),
                    },
                    secure,
                    project_file: loaded.map(|l| l.file.display().to_string()),
                    project_file_kind: loaded.map(|l| match l.kind {
                        FileKind::Committed => ProjectFileKind::Project,
                        FileKind::Managed => ProjectFileKind::Managed,
                    }),
                    ignored_managed_file: None,
                    needs: vec![],
                    project_error,
                    node: None,
                });
            }
            (None, Some(target)) => match parse_proxy_target(target) {
                Ok(_) => out.push(SiteInfo {
                    name: name.clone(),
                    url: url(&host, secure),
                    host,
                    source: SiteSource::Proxy,
                    kind: SiteKind::Proxy { target: target.clone() },
                    secure,
                    project_file: None,
                    project_file_kind: None,
                    ignored_managed_file: None,
                    needs: vec![],
                    project_error: None,
                    node: None,
                }),
                Err(e) => warnings.push(format!("sites.toml: {name}: {}", e.message)),
            },
            (None, None) => warnings.push(format!("sites.toml: {name} has no path or proxy; skipped")),
        }
    }
    Resolution { sites: out, warnings }
}

/// Sets `ignored_managed_file` on each site with a managed `.yard` that is not used.
pub fn mark_ignored(sites: &mut [SiteInfo], ignored: &IgnoredFiles) {
    for site in sites {
        let Some(i) = ignored.get(&site.name) else { continue };
        let (because, project) = match &i.because {
            IgnoredBecause::Committed => (yard_proto::IgnoredBecause::Committed, None),
            IgnoredBecause::OtherProject(p) => {
                (yard_proto::IgnoredBecause::OtherProject, p.as_ref().map(|p| p.display().to_string()))
            }
        };
        site.ignored_managed_file =
            Some(IgnoredManagedFile { file: i.file.display().to_string(), because, project });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SiteEntry, SitesConfig, YardConfig};
    use crate::project::Projects;
    use std::collections::{BTreeMap, HashSet};
    use std::path::{Path, PathBuf};
    use yard_proto::{ErrorCode, SiteKind, SiteSource};

    struct Fs(HashSet<PathBuf>);

    impl Fs {
        fn new(dirs: &[&str]) -> Self {
            Fs(dirs.iter().map(PathBuf::from).collect())
        }
        fn is_dir(&self) -> impl Fn(&Path) -> bool + '_ {
            |p| self.0.contains(p)
        }
    }

    fn cfg() -> YardConfig {
        YardConfig {
            default_php: crate::PhpVersion::DEFAULT,
            tld: None,
            secure_by_default: false,
            extensions: Default::default(),
            default_node: None,
            service_sleep_after: None,
            tunnel_domains: vec![],
        }
    }

    fn sites(entries: &[(&str, SiteEntry)]) -> SitesConfig {
        SitesConfig {
            sites: entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>(),
        }
    }

    fn linked(path: &str) -> SiteEntry {
        SiteEntry { path: Some(path.into()), ..Default::default() }
    }

    fn php_parts(site: &yard_proto::SiteInfo) -> (&str, &str) {
        match &site.kind {
            SiteKind::Php { path, docroot, .. } => (path, docroot),
            other => panic!("expected php site, got {other:?}"),
        }
    }

    fn project(name: &str, text: &str) -> Projects {
        let file = Path::new("/code").join(name).join(crate::project::FILE);
        let parsed = crate::project::parse(text, &file);
        Projects::from([(
            name.to_string(),
            crate::project::Loaded { file, kind: FileKind::Committed, parsed },
        )])
    }

    #[test]
    fn a_managed_file_is_named_with_its_kind_and_an_ignored_one_is_marked() {
        let fs = Fs::new(&["/code/ee", "/code/app"]);
        let file = PathBuf::from("/r/sites/ee/.yard");
        let parsed = crate::project::parse("php = \"7.3\"\n", &file);
        let p = Projects::from([(
            "ee".to_string(),
            crate::project::Loaded { file, kind: FileKind::Managed, parsed },
        )]);
        let s = sites(&[("ee", linked("/code/ee")), ("app", linked("/code/app"))]);
        let mut r = resolve(&cfg(), &s, &p, "yard", 8080, 8443, &fs.is_dir());
        let ignored = IgnoredFiles::from([(
            "app".to_string(),
            crate::project::Ignored {
                file: "/r/sites/app/.yard".into(),
                because: IgnoredBecause::OtherProject(Some("/code/old-app".into())),
            },
        )]);
        mark_ignored(&mut r.sites, &ignored);
        let ee = r.sites.iter().find(|s| s.name == "ee").unwrap();
        assert_eq!(ee.project_file.as_deref(), Some("/r/sites/ee/.yard"));
        assert_eq!(ee.project_file_kind, Some(ProjectFileKind::Managed));
        assert_eq!(ee.ignored_managed_file, None);
        let app = r.sites.iter().find(|s| s.name == "app").unwrap();
        assert_eq!(app.project_file_kind, None);
        assert_eq!(
            app.ignored_managed_file,
            Some(IgnoredManagedFile {
                file: "/r/sites/app/.yard".into(),
                because: yard_proto::IgnoredBecause::OtherProject,
                project: Some("/code/old-app".into()),
            })
        );
    }

    #[test]
    fn project_file_settings_win_over_sites_toml() {
        let fs = Fs::new(&["/code/ee", "/code/ee/public", "/code/ee/web"]);
        let entry = SiteEntry {
            php: Some("7.1".parse().unwrap()),
            secure: Some(true),
            docroot: Some("web".into()),
            ..linked("/code/ee")
        };
        let r = resolve(
            &cfg(),
            &sites(&[("ee", entry)]),
            &project("ee", "php = \"7.3\"\n"),
            "yard",
            8080,
            8443,
            &fs.is_dir(),
        );
        let site = &r.sites[0];
        match &site.kind {
            SiteKind::Php { php, docroot, .. } => {
                assert_eq!((php.as_str(), docroot.as_str()), ("7.3", "/code/ee/public"))
            }
            other => panic!("{other:?}"),
        }
        assert!(!site.secure);
        assert_eq!(site.url, "http://ee.yard:8080/");
        assert_eq!(site.project_file.as_deref(), Some("/code/ee/.yard"));
        assert!(!site.needs_setup());
    }

    #[test]
    fn project_file_docroot_and_secure() {
        let fs = Fs::new(&["/code/ee", "/code/ee/public"]);
        let p = project("ee", "docroot = \"web/dist\"\nsecure = true\n");
        let r = resolve(&cfg(), &sites(&[("ee", linked("/code/ee"))]), &p, "yard", 8080, 8443, &fs.is_dir());
        assert_eq!(php_parts(&r.sites[0]).1, "/code/ee/web/dist");
        assert!(r.sites[0].secure);
    }

    #[test]
    fn invalid_project_file_marks_the_site() {
        let fs = Fs::new(&["/code/ee"]);
        let p = project("ee", "nodejs = \"20\"\n");
        let entry = SiteEntry { php: Some("7.1".parse().unwrap()), ..linked("/code/ee") };
        let r = resolve(&cfg(), &sites(&[("ee", entry)]), &p, "yard", 8080, 8443, &fs.is_dir());
        let site = &r.sites[0];
        assert!(site.needs_setup());
        assert_eq!(site.project_error.as_ref().unwrap().code, ErrorCode::ProjectFileInvalid);
        match &site.kind {
            SiteKind::Php { php, .. } => assert_eq!(php, "8.4"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn secure_site_has_an_https_url() {
        let fs = Fs::new(&["/code/blog"]);
        let s = sites(&[("blog", SiteEntry { secure: Some(true), ..linked("/code/blog") })]);
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 8080, 8443, &fs.is_dir());
        assert!(r.sites[0].secure);
        assert_eq!(r.sites[0].url, "https://blog.yard:8443/");
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 80, 443, &fs.is_dir());
        assert_eq!(r.sites[0].url, "https://blog.yard/");
    }

    #[test]
    fn secure_by_default_secures_unless_the_site_says_no() {
        let fs = Fs::new(&["/a", "/b"]);
        let cfg = YardConfig { secure_by_default: true, ..cfg() };
        let s = sites(&[("a", linked("/a")), ("b", SiteEntry { secure: Some(false), ..linked("/b") })]);
        let r = resolve(&cfg, &s, &Projects::new(), "yard", 8080, 8443, &fs.is_dir());
        let secure: Vec<(&str, bool)> = r.sites.iter().map(|s| (s.name.as_str(), s.secure)).collect();
        assert_eq!(secure, [("a", true), ("b", false)]);
        assert_eq!(r.sites[1].url, "http://b.yard:8080/");
    }

    #[test]
    fn a_directory_is_served_only_when_linked() {
        let fs = Fs::new(&["/h/Herd", "/h/Herd/blog", "/h/Herd/blog/public"]);
        let r = resolve(&cfg(), &SitesConfig::default(), &Projects::new(), "yard", 8080, 8443, &fs.is_dir());
        assert!(r.sites.is_empty(), "{:?}", r.sites);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn linked_dir_becomes_site_with_public_docroot() {
        let fs = Fs::new(&["/code/blog", "/code/blog/public"]);
        let r = resolve(
            &cfg(),
            &sites(&[("blog", linked("/code/blog"))]),
            &Projects::new(),
            "yard",
            8080,
            8443,
            &fs.is_dir(),
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let s = &r.sites[0];
        assert_eq!(
            (s.name.as_str(), s.host.as_str(), s.url.as_str()),
            ("blog", "blog.yard", "http://blog.yard:8080/")
        );
        assert_eq!(s.source, SiteSource::Linked);
        assert_eq!(php_parts(s), ("/code/blog", "/code/blog/public"));
        match &s.kind {
            SiteKind::Php { php, .. } => assert_eq!(php, "8.4"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn isolated_site_resolves_to_its_version() {
        let fs = Fs::new(&["/code/old", "/code/new"]);
        let s = sites(&[
            ("old", SiteEntry { php: Some("7.1".parse().unwrap()), ..linked("/code/old") }),
            ("new", linked("/code/new")),
        ]);
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 8080, 8443, &fs.is_dir());
        let php = |name: &str| match &r.sites.iter().find(|s| s.name == name).unwrap().kind {
            SiteKind::Php { php, .. } => php.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(php("old"), "7.1");
        assert_eq!(php("new"), "8.4");
    }

    #[test]
    fn port_80_is_omitted_from_url() {
        let fs = Fs::new(&["/p/a"]);
        let r = resolve(
            &cfg(),
            &sites(&[("a", linked("/p/a"))]),
            &Projects::new(),
            "test",
            80,
            443,
            &fs.is_dir(),
        );
        assert_eq!(r.sites[0].url, "http://a.test/");
    }

    #[test]
    fn docroot_detection_order() {
        let fs = Fs::new(&["/a/web", "/a/public_html", "/b/public_html"]);
        assert_eq!(detect_docroot(Path::new("/a"), &fs.is_dir()), PathBuf::from("/a/web"));
        assert_eq!(detect_docroot(Path::new("/b"), &fs.is_dir()), PathBuf::from("/b/public_html"));
        assert_eq!(detect_docroot(Path::new("/c"), &fs.is_dir()), PathBuf::from("/c"));
    }

    #[test]
    fn missing_linked_path_is_skipped_with_warning() {
        let r = resolve(
            &cfg(),
            &sites(&[("gone", linked("/nowhere"))]),
            &Projects::new(),
            "yard",
            8080,
            8443,
            &Fs::new(&[]).is_dir(),
        );
        assert!(r.sites.is_empty());
        assert!(r.warnings[0].contains("gone"), "{:?}", r.warnings);
    }

    #[test]
    fn proxy_site() {
        let s = sites(&[(
            "api",
            SiteEntry { proxy: Some("http://127.0.0.1:3000".into()), ..Default::default() },
        )]);
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 8080, 8443, &Fs::new(&[]).is_dir());
        assert_eq!(r.sites[0].kind, SiteKind::Proxy { target: "http://127.0.0.1:3000".into() });
        assert_eq!(r.sites[0].source, SiteSource::Proxy);
        assert_eq!(r.sites[0].host, "api.yard");
    }

    #[test]
    fn docroot_override_relative_and_absolute() {
        let fs = Fs::new(&["/p/a", "/p/b", "/p/a/public"]);
        let s = sites(&[
            ("a", SiteEntry { docroot: Some("dist".into()), ..linked("/p/a") }),
            ("b", SiteEntry { docroot: Some("/elsewhere/www".into()), ..linked("/p/b") }),
        ]);
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 8080, 8443, &fs.is_dir());
        assert_eq!(php_parts(&r.sites[0]).1, "/p/a/dist");
        assert_eq!(php_parts(&r.sites[1]).1, "/elsewhere/www");
    }

    #[test]
    fn docroot_without_path_or_proxy_is_skipped() {
        let s = sites(&[("blog", SiteEntry { docroot: Some("public".into()), ..Default::default() })]);
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 8080, 8443, &Fs::new(&[]).is_dir());
        assert!(r.sites.is_empty());
        assert_eq!(r.warnings, ["sites.toml: blog has no path or proxy; skipped"]);
    }

    #[test]
    fn invalid_site_name_is_skipped_with_warning() {
        let r = resolve(
            &cfg(),
            &sites(&[("My App", linked("/x"))]),
            &Projects::new(),
            "yard",
            8080,
            8443,
            &Fs::new(&["/x"]).is_dir(),
        );
        assert!(r.sites.is_empty());
        assert!(r.warnings[0].contains("My App"), "{:?}", r.warnings);
    }

    #[test]
    fn entry_with_path_and_proxy_is_skipped_with_warning() {
        let s = sites(&[("both", SiteEntry { proxy: Some("http://127.0.0.1:1".into()), ..linked("/x") })]);
        let r = resolve(&cfg(), &s, &Projects::new(), "yard", 8080, 8443, &Fs::new(&["/x"]).is_dir());
        assert!(r.sites.is_empty());
        assert!(r.warnings[0].contains("both"));
    }

    #[test]
    fn host_label_rules() {
        assert_eq!(host_label("MyApp").as_deref(), Some("myapp"));
        assert_eq!(host_label("a_b c").as_deref(), Some("a-b-c"));
        assert_eq!(host_label(".hidden"), None);
        assert_eq!(host_label("-lead"), None);
        assert_eq!(host_label("a..b"), None);
        assert_eq!(host_label("trail-"), None);
        assert_eq!(host_label(""), None);
    }

    #[test]
    fn proxy_target_requires_scheme_and_port() {
        assert_eq!(parse_proxy_target("http://127.0.0.1:3000").unwrap(), ("127.0.0.1".to_string(), 3000));
        assert_eq!(
            parse_proxy_target("https://localhost:5173/app").unwrap(),
            ("localhost".to_string(), 5173)
        );
        for bad in
            ["localhost:3000", "http://localhost", "ftp://h:1", "http://:3000", "http://h:0", "http://h:x"]
        {
            assert_eq!(parse_proxy_target(bad).unwrap_err().code, ErrorCode::InvalidArgument, "{bad}");
        }
    }
}
