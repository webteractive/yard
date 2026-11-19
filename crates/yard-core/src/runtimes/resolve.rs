//! Which version of a runtime a directory gets (runtimes spec §7): M4c's Node walk, for any
//! runtime. PHP keeps its own walk (`shim::resolve_version`).

use super::version::{VersionSpec, spec_line};
use crate::Paths;
use crate::config::{SiteEntry, SitesConfig};
use crate::project::{self, ProjectFile};
use crate::shim::{innermost, naming};
use std::fmt;
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

/// Why a runtime's spec was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinSource {
    /// The nearest `.yard` that sets the key.
    Project(PathBuf),
    /// The managed `.yard` of the linked site containing the cwd.
    Managed(PathBuf),
    /// `yard runtime use`, in `sites.toml`.
    Site(String),
    /// One of the recipe's `pin_files`.
    PinFile(PathBuf),
    Default,
}

impl fmt::Display for PinSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PinSource::Project(file) | PinSource::Managed(file) | PinSource::PinFile(file) => {
                write!(f, "{}", file.display())
            }
            PinSource::Site(name) => write!(f, "site {name} uses it"),
            PinSource::Default => write!(f, "the default"),
        }
    }
}

/// A Reader as the shims and yardd give it: `None` for no file, an error for one that
/// cannot be read.
pub type Read<'a> = &'a dyn Fn(&Path) -> Option<Result<String, YardError>>;

/// The pin `cwd` resolves to, walking up (M4c spec §5, runtimes spec §7): the nearest
/// directory with any pin wins. In one directory a `.yard` that sets it (`from_file`) comes
/// first, then, in the innermost linked site's root, the site's own setting (its managed
/// `.yard` when that is for the site's path, else `sites.toml` through `from_site`), then
/// each of `pin_files` in order, read with `from_pin_file`. A committed `.yard` at the
/// site's root skips the site's setting even without the key; a managed file for the site
/// without it gives nothing, so `sites.toml` is not consulted behind it. `None`: nothing
/// pins it, and the caller applies the default.
///
/// A file that cannot be used stops the search, with the file in the error's data: a shim
/// never falls through a pin it cannot meet.
#[allow(clippy::too_many_arguments)]
pub fn resolve_pin<S>(
    cwd: &Path,
    pin_files: &[String],
    sites: &SitesConfig,
    paths: &Paths,
    read: Read,
    canonical: &dyn Fn(&Path) -> PathBuf,
    from_file: &dyn Fn(&ProjectFile) -> Option<S>,
    from_site: &dyn Fn(&SiteEntry) -> Option<S>,
    from_pin_file: &dyn Fn(&str, &Path) -> Result<S, YardError>,
) -> Result<Option<(S, PinSource)>, YardError> {
    let innermost = innermost(cwd, sites, canonical);
    let site = || site_setting(&innermost, paths, read, from_file, from_site);
    let mut site_root = innermost.last().map(|(.., path)| (*path, canonical(path)));
    for dir in cwd.ancestors() {
        let file = dir.join(project::FILE);
        if let Some(text) = read(&file)
            && let Some(spec) =
                from_file(&project::parse(&text.map_err(naming(&file))?, &file).map_err(naming(&file))?)
        {
            return Ok(Some((spec, PinSource::Project(file))));
        }
        if site_root.as_ref().is_some_and(|(path, real)| dir == *path || canonical(dir) == *real) {
            site_root = None;
            if let Some(found) = site()? {
                return Ok(Some(found));
            }
        }
        for name in pin_files {
            let file = dir.join(name);
            if let Some(text) = read(&file) {
                let spec = text.and_then(|text| from_pin_file(&text, &file)).map_err(naming(&file))?;
                return Ok(Some((spec, PinSource::PinFile(file))));
            }
        }
    }
    // The walk never met the site's root (a path only its canonical form contains).
    if site_root.is_some() {
        return site();
    }
    Ok(None)
}

/// The site's own setting in [`resolve_pin`]: `None` when the innermost site sets nothing.
fn site_setting<S>(
    innermost: &[(&String, &SiteEntry, &Path)],
    paths: &Paths,
    read: Read,
    from_file: &dyn Fn(&ProjectFile) -> Option<S>,
    from_site: &dyn Fn(&SiteEntry) -> Option<S>,
) -> Result<Option<(S, PinSource)>, YardError> {
    let Some((name, entry, path)) = innermost.last() else { return Ok(None) };
    if read(&path.join(project::FILE)).is_some() {
        return Ok(None);
    }
    for (name, _, path) in innermost.iter().rev() {
        let file = paths.managed_project_file(name);
        if let Some(text) = read(&file) {
            let (managed, project) = project::parse_managed(&text?, &file)?;
            if project.as_deref() == Some(*path) {
                return Ok(from_file(&managed).map(|spec| (spec, PinSource::Managed(file))));
            }
        }
    }
    Ok(from_site(entry).map(|spec| (spec, PinSource::Site((*name).clone()))))
}

/// The pin a [`resolve_pin`] error is about, as the source it would have been.
pub fn failed_source(e: &YardError, paths: &Paths) -> Option<PinSource> {
    let file = PathBuf::from(e.data.as_ref()?.get("file")?.as_str()?);
    Some(match file.file_name()?.to_str()? {
        project::FILE if file.starts_with(paths.managed_sites_dir()) => PinSource::Managed(file),
        project::FILE => PinSource::Project(file),
        _ => PinSource::PinFile(file),
    })
}

/// The spec in a recipe's pin file (`.bun-version`): its first line that is not blank or a
/// comment. `INVALID_ARGUMENT` naming the file when it has none Yard accepts.
pub fn pin_file_version(text: &str, file: &Path) -> Result<VersionSpec, YardError> {
    let message = match spec_line(text) {
        None => format!("{} names no version", file.display()),
        Some((line, spec)) => match VersionSpec::parse(spec) {
            Ok(spec) => return Ok(spec),
            Err(why) => format!("{} line {line}: {why}", file.display()),
        },
    };
    Err(YardError::new(ErrorCode::InvalidArgument, message)
        .with_hint(format!("put a version such as 1.2 in {}", file.display())))
}

/// Whether the site at `root` shows the runtime: one of its `uses_when` files, its own
/// setting, or a pin in its root. A pin above the root still picks the version, but alone
/// does not make the site use the runtime.
pub fn uses_runtime(source: &PinSource, root: &Path, has_uses_file: bool) -> bool {
    has_uses_file
        || match source {
            PinSource::Site(_) | PinSource::Managed(_) => true,
            PinSource::Project(f) | PinSource::PinFile(f) => f.parent() == Some(root),
            PinSource::Default => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::sites::parse_sites;
    use crate::runtimes::version::VersionSpec;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use yard_proto::ErrorCode;

    const SITES: &str = r#"
[sites.shop]
path = "/code/shop"
runtimes = { composer = "2.2" }
[sites.plain]
path = "/code/plain"
"#;

    fn same(p: &Path) -> PathBuf {
        p.to_path_buf()
    }

    fn spec(s: &str) -> VersionSpec {
        VersionSpec::parse(s).unwrap()
    }

    fn composer_at(
        files: &BTreeMap<PathBuf, String>,
        pin_files: &[String],
        cwd: &str,
    ) -> Result<Option<(VersionSpec, PinSource)>, YardError> {
        let sites = parse_sites(Some(SITES), Path::new("/r/sites.toml")).unwrap();
        let read = |p: &Path| files.get(p).cloned().map(Ok);
        resolve_pin(
            Path::new(cwd),
            pin_files,
            &sites,
            &Paths::new("/r"),
            &read,
            &same,
            &|f: &ProjectFile| f.runtimes.get("composer").cloned(),
            &|s: &SiteEntry| s.runtimes.get("composer").cloned(),
            &|text: &str, file: &Path| pin_file_version(text, file),
        )
    }

    fn files(list: &[(&str, &str)]) -> BTreeMap<PathBuf, String> {
        list.iter().map(|(p, t)| (PathBuf::from(p), t.to_string())).collect()
    }

    #[test]
    fn the_nearest_yard_with_the_key_wins() {
        let f = files(&[
            ("/code/.yard", "[runtimes]\ncomposer = \"1\"\n"),
            ("/code/x/.yard", "[runtimes]\ncomposer = \"2\"\n"),
        ]);
        assert_eq!(
            composer_at(&f, &[], "/code/x/y").unwrap(),
            Some((spec("2"), PinSource::Project("/code/x/.yard".into())))
        );
    }

    #[test]
    fn a_nearer_yard_without_the_key_passes_over() {
        let f = files(&[
            ("/r/sites/shop/.yard", "project = \"/code/shop\"\n[runtimes]\ncomposer = \"2.2\"\n"),
            ("/code/shop/packages/a/.yard", "php = \"8.4\"\n"),
        ]);
        let got = composer_at(&f, &[], "/code/shop/packages/a").unwrap();
        assert_eq!(got, Some((spec("2.2"), PinSource::Managed("/r/sites/shop/.yard".into()))));
        assert_eq!(got.unwrap().1.to_string(), "/r/sites/shop/.yard");
    }

    #[test]
    fn the_site_setting_beats_a_yard_above_the_site() {
        let f = files(&[("/code/.yard", "[runtimes]\ncomposer = \"1\"\n")]);
        let got = composer_at(&f, &[], "/code/shop/src").unwrap();
        assert_eq!(got, Some((spec("2.2"), PinSource::Site("shop".into()))));
        assert_eq!(got.unwrap().1.to_string(), "site shop uses it");
    }

    #[test]
    fn a_managed_file_without_the_key_hides_sites_toml() {
        let f = files(&[("/r/sites/shop/.yard", "project = \"/code/shop\"\nphp = \"8.4\"\n")]);
        assert_eq!(composer_at(&f, &[], "/code/shop/src").unwrap(), None);
    }

    #[test]
    fn a_committed_yard_skips_the_site_setting() {
        let f = files(&[("/code/shop/.yard", "php = \"8.4\"\n")]);
        assert_eq!(composer_at(&f, &[], "/code/shop/src").unwrap(), None);
    }

    #[test]
    fn an_unusable_file_stops_the_walk() {
        let sites = parse_sites(Some(SITES), Path::new("/r/sites.toml")).unwrap();
        let f = files(&[("/.yard", "[runtimes]\ncomposer = \"2\"\n")]);
        let read = |p: &Path| {
            if p == Path::new("/code/.yard") {
                Some(Err(YardError::new(ErrorCode::InvalidArgument, "/code/.yard is a FIFO")))
            } else {
                f.get(p).cloned().map(Ok)
            }
        };
        let e = resolve_pin(
            Path::new("/code/x"),
            &[],
            &sites,
            &Paths::new("/r"),
            &read,
            &same,
            &|f: &ProjectFile| f.runtimes.get("composer").cloned(),
            &|s: &SiteEntry| s.runtimes.get("composer").cloned(),
            &|text: &str, file: &Path| pin_file_version(text, file),
        )
        .unwrap_err();
        assert_eq!(e.message, "/code/.yard is a FIFO");
        assert_eq!(e.data.unwrap()["file"], "/code/.yard");
        let bad = files(&[("/code/.yard", "[runtimes]\ncomposer = \"^2\"\n")]);
        assert_eq!(composer_at(&bad, &[], "/code/x").unwrap_err().code, ErrorCode::ProjectFileInvalid);
    }

    #[test]
    fn nothing_pinned_is_none() {
        assert_eq!(composer_at(&files(&[]), &[], "/elsewhere/x").unwrap(), None);
        assert_eq!(composer_at(&files(&[]), &[], "/code/plain").unwrap(), None);
    }

    #[test]
    fn a_pin_file_is_read_with_its_parser() {
        let pins = [".bun-version".to_string()];
        let f = files(&[
            ("/code/plain/.bun-version", "# pinned\nv1.2\n"),
            ("/code/.yard", "[runtimes]\ncomposer = \"2\"\n"),
        ]);
        assert_eq!(
            composer_at(&f, &pins, "/code/plain/src").unwrap(),
            Some((spec("1.2"), PinSource::PinFile("/code/plain/.bun-version".into())))
        );
        let bad = files(&[("/code/plain/.bun-version", "latest\n")]);
        let e = composer_at(&bad, &pins, "/code/plain").unwrap_err();
        assert!(e.message.contains("/code/plain/.bun-version line 1"), "{}", e.message);
    }

    #[test]
    fn uses_follows_nodes_rule() {
        let root = Path::new("/code/shop");
        assert!(uses_runtime(&PinSource::Default, root, true));
        assert!(!uses_runtime(&PinSource::Default, root, false));
        assert!(uses_runtime(&PinSource::Site("shop".into()), root, false));
        assert!(uses_runtime(&PinSource::Project("/code/shop/.yard".into()), root, false));
        assert!(!uses_runtime(&PinSource::Project("/code/.yard".into()), root, false));
    }
}
