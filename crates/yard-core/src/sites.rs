//! Turning parked directories and `sites.toml` entries into served sites (spec §5.1).

use crate::config::{SitesConfig, YardConfig};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, SiteInfo, SiteKind, SiteSource, YardError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A directory, or a symlink that resolves to one.
    Dir,
    File,
    BrokenLink,
}

/// One child of a parked path, as the daemon listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
}

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

pub fn resolve(
    cfg: &YardConfig,
    sites: &SitesConfig,
    parked: &[(PathBuf, Vec<DirEntry>)],
    tld: &str,
    http_port: u16,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Resolution {
    let mut warnings = Vec::new();
    let found = scan_parked(parked, &mut warnings);
    let php = cfg.default_php.to_string();
    let url = |host: &str| match http_port {
        80 => format!("http://{host}/"),
        p => format!("http://{host}:{p}/"),
    };
    let php_site = |name: &str, source, project: &Path, docroot: &Path| SiteInfo {
        name: name.to_string(),
        host: format!("{name}.{tld}"),
        url: url(&format!("{name}.{tld}")),
        source,
        kind: SiteKind::Php {
            path: project.display().to_string(),
            docroot: docroot.display().to_string(),
            php: php.clone(),
        },
    };
    let docroot_for = |project: &Path, over: Option<&PathBuf>| match over {
        Some(d) if d.is_absolute() => d.clone(),
        Some(d) => project.join(d),
        None => detect_docroot(project, is_dir),
    };

    let mut out: BTreeMap<String, SiteInfo> = BTreeMap::new();
    let mut docroot_overrides: BTreeMap<&str, &PathBuf> = BTreeMap::new();
    for (name, entry) in &sites.sites {
        if host_label(name).as_deref() != Some(name.as_str()) {
            warnings.push(format!("sites.toml: {name:?} is not a valid host name; skipped"));
            continue;
        }
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
                let docroot = docroot_for(path, entry.docroot.as_ref());
                out.insert(name.clone(), php_site(name, SiteSource::Linked, path, &docroot));
            }
            (None, Some(target)) => match parse_proxy_target(target) {
                Ok(_) => {
                    let host = format!("{name}.{tld}");
                    out.insert(
                        name.clone(),
                        SiteInfo {
                            name: name.clone(),
                            url: url(&host),
                            host,
                            source: SiteSource::Proxy,
                            kind: SiteKind::Proxy { target: target.clone() },
                        },
                    );
                }
                Err(e) => warnings.push(format!("sites.toml: {name}: {}", e.message)),
            },
            (None, None) => match (&entry.docroot, found.contains_key(name)) {
                (Some(d), true) => {
                    docroot_overrides.insert(name, d);
                }
                _ => warnings.push(format!(
                    "sites.toml: {name} has no path or proxy and matches no parked site; skipped"
                )),
            },
        }
    }
    for (name, project) in &found {
        if !out.contains_key(name) {
            let docroot = docroot_for(project, docroot_overrides.get(name.as_str()).copied());
            out.insert(name.clone(), php_site(name, SiteSource::Parked, project, &docroot));
        }
    }
    Resolution { sites: out.into_values().collect(), warnings }
}

/// Parked candidates by label. Hidden entries and plain files are skipped silently;
/// broken links, invalid names and duplicates get a warning, and the first wins.
fn scan_parked(parked: &[(PathBuf, Vec<DirEntry>)], warnings: &mut Vec<String>) -> BTreeMap<String, PathBuf> {
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (_, entries) in parked {
        let mut entries: Vec<&DirEntry> = entries.iter().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for e in entries {
            if e.name.starts_with('.') {
                continue;
            }
            match e.kind {
                EntryKind::File => continue,
                EntryKind::BrokenLink => {
                    warnings.push(format!("skipping {}: broken symlink", e.path.display()));
                    continue;
                }
                EntryKind::Dir => {}
            }
            let Some(label) = host_label(&e.name) else {
                warnings.push(format!(
                    "skipping {}: {:?} is not a valid host name",
                    e.path.display(),
                    e.name
                ));
                continue;
            };
            if let Some(first) = found.get(&label) {
                warnings.push(format!(
                    "{label}: {} and {} both map to this name; {} wins",
                    first.display(),
                    e.path.display(),
                    first.display()
                ));
                continue;
            }
            found.insert(label, e.path.clone());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SiteEntry, SitesConfig, YardConfig};
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

    fn dir(parent: &str, name: &str) -> DirEntry {
        DirEntry { name: name.into(), path: Path::new(parent).join(name), kind: EntryKind::Dir }
    }

    fn cfg(parked: &[&str]) -> YardConfig {
        YardConfig {
            parked: parked.iter().map(PathBuf::from).collect(),
            default_php: crate::PhpVersion::DEFAULT,
            tld: None,
        }
    }

    fn sites(entries: &[(&str, SiteEntry)]) -> SitesConfig {
        SitesConfig {
            sites: entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>(),
        }
    }

    fn php_parts(site: &yard_proto::SiteInfo) -> (&str, &str) {
        match &site.kind {
            SiteKind::Php { path, docroot, .. } => (path, docroot),
            other => panic!("expected php site, got {other:?}"),
        }
    }

    #[test]
    fn parked_dir_becomes_site_with_public_docroot() {
        let fs = Fs::new(&["/h/Herd/blog", "/h/Herd/blog/public"]);
        let parked = vec![(PathBuf::from("/h/Herd"), vec![dir("/h/Herd", "blog")])];
        let r = resolve(&cfg(&["/h/Herd"]), &SitesConfig::default(), &parked, "yard", 8080, &fs.is_dir());
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let s = &r.sites[0];
        assert_eq!(
            (s.name.as_str(), s.host.as_str(), s.url.as_str()),
            ("blog", "blog.yard", "http://blog.yard:8080/")
        );
        assert_eq!(s.source, SiteSource::Parked);
        assert_eq!(php_parts(s), ("/h/Herd/blog", "/h/Herd/blog/public"));
        match &s.kind {
            SiteKind::Php { php, .. } => assert_eq!(php, "8.4"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn port_80_is_omitted_from_url() {
        let fs = Fs::new(&["/p/a"]);
        let parked = vec![(PathBuf::from("/p"), vec![dir("/p", "a")])];
        let r = resolve(&cfg(&["/p"]), &SitesConfig::default(), &parked, "test", 80, &fs.is_dir());
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
    fn linked_overrides_parked() {
        let fs = Fs::new(&["/h/Herd/blog", "/code/blog"]);
        let parked = vec![(PathBuf::from("/h/Herd"), vec![dir("/h/Herd", "blog")])];
        let s = sites(&[("blog", SiteEntry { path: Some("/code/blog".into()), ..Default::default() })]);
        let r = resolve(&cfg(&["/h/Herd"]), &s, &parked, "yard", 8080, &fs.is_dir());
        assert_eq!(r.sites.len(), 1);
        assert_eq!(r.sites[0].source, SiteSource::Linked);
        assert_eq!(php_parts(&r.sites[0]).0, "/code/blog");
    }

    #[test]
    fn missing_linked_path_is_skipped_with_warning() {
        let s = sites(&[("gone", SiteEntry { path: Some("/nowhere".into()), ..Default::default() })]);
        let r = resolve(&cfg(&[]), &s, &[], "yard", 8080, &Fs::new(&[]).is_dir());
        assert!(r.sites.is_empty());
        assert!(r.warnings[0].contains("gone"), "{:?}", r.warnings);
    }

    #[test]
    fn proxy_site() {
        let s = sites(&[(
            "api",
            SiteEntry { proxy: Some("http://127.0.0.1:3000".into()), ..Default::default() },
        )]);
        let r = resolve(&cfg(&[]), &s, &[], "yard", 8080, &Fs::new(&[]).is_dir());
        assert_eq!(r.sites[0].kind, SiteKind::Proxy { target: "http://127.0.0.1:3000".into() });
        assert_eq!(r.sites[0].source, SiteSource::Proxy);
        assert_eq!(r.sites[0].host, "api.yard");
    }

    #[test]
    fn docroot_override_relative_and_absolute() {
        let fs = Fs::new(&["/p/a", "/p/b", "/p/a/public"]);
        let parked = vec![(PathBuf::from("/p"), vec![dir("/p", "a"), dir("/p", "b")])];
        let s = sites(&[
            ("a", SiteEntry { docroot: Some("dist".into()), ..Default::default() }),
            ("b", SiteEntry { docroot: Some("/elsewhere/www".into()), ..Default::default() }),
        ]);
        let r = resolve(&cfg(&["/p"]), &s, &parked, "yard", 8080, &fs.is_dir());
        assert_eq!(php_parts(&r.sites[0]).1, "/p/a/dist");
        assert_eq!(php_parts(&r.sites[1]).1, "/elsewhere/www");
        assert_eq!(r.sites[0].source, SiteSource::Parked);
    }

    #[test]
    fn docroot_only_entry_without_parked_site_warns() {
        let s = sites(&[("ghost", SiteEntry { docroot: Some("public".into()), ..Default::default() })]);
        let r = resolve(&cfg(&[]), &s, &[], "yard", 8080, &Fs::new(&[]).is_dir());
        assert!(r.sites.is_empty());
        assert!(r.warnings[0].contains("ghost"));
    }

    #[test]
    fn duplicate_parked_name_warns_first_wins() {
        let fs = Fs::new(&["/one/app", "/two/app"]);
        let parked = vec![
            (PathBuf::from("/one"), vec![dir("/one", "app")]),
            (PathBuf::from("/two"), vec![dir("/two", "app")]),
        ];
        let r =
            resolve(&cfg(&["/one", "/two"]), &SitesConfig::default(), &parked, "yard", 8080, &fs.is_dir());
        assert_eq!(r.sites.len(), 1);
        assert_eq!(php_parts(&r.sites[0]).0, "/one/app");
        assert!(r.warnings[0].contains("/one/app") && r.warnings[0].contains("/two/app"), "{:?}", r.warnings);
    }

    #[test]
    fn entry_with_path_and_proxy_is_skipped_with_warning() {
        let s = sites(&[(
            "both",
            SiteEntry {
                path: Some("/x".into()),
                proxy: Some("http://127.0.0.1:1".into()),
                ..Default::default()
            },
        )]);
        let r = resolve(&cfg(&[]), &s, &[], "yard", 8080, &Fs::new(&["/x"]).is_dir());
        assert!(r.sites.is_empty());
        assert!(r.warnings[0].contains("both"));
    }

    #[test]
    fn normalizes_and_skips_odd_entries() {
        let p = "/p";
        let mut entries: Vec<DirEntry> = ["MyApp", "my app", "my_app", "jane.doe", ".git", "émoji"]
            .iter()
            .map(|n| dir(p, n))
            .collect();
        entries.push(DirEntry {
            name: "notes.txt".into(),
            path: "/p/notes.txt".into(),
            kind: EntryKind::File,
        });
        entries.push(DirEntry {
            name: "dangling".into(),
            path: "/p/dangling".into(),
            kind: EntryKind::BrokenLink,
        });
        let fs = Fs::new(&["/p/MyApp", "/p/my app", "/p/my_app", "/p/jane.doe", "/p/.git", "/p/émoji"]);
        let r = resolve(
            &cfg(&[p]),
            &SitesConfig::default(),
            &[(PathBuf::from(p), entries)],
            "yard",
            8080,
            &fs.is_dir(),
        );
        let names: Vec<&str> = r.sites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["jane.doe", "my-app", "myapp"]);
        let my_app = r.sites.iter().find(|s| s.name == "my-app").unwrap();
        assert_eq!(php_parts(my_app).0, "/p/my app", "\"my app\" sorts before \"my_app\" and wins");
        let all = r.warnings.join("\n");
        assert!(all.contains("my_app"), "{all}");
        assert!(all.contains("dangling"), "{all}");
        assert!(all.contains("émoji"), "{all}");
        assert!(!all.contains(".git") && !all.contains("notes.txt"), "{all}");
        assert_eq!(r.warnings.len(), 3, "{all}");
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
