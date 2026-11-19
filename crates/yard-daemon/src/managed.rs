//! Editing a site's managed `.yard` for `yard isolate`, `secure` and `docroot`
//! (managed-file spec §4): Yard owns the file's place, so these commands change it
//! instead of being refused, keeping every other key and comment.

use crate::project::read_project_file;
use std::path::{Path, PathBuf};
use yard_core::Paths;
use yard_core::config::{TomlFile, load_sites};
use yard_core::project;
use yard_proto::{ErrorCode, YardError};

/// The file that holds a linked site's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteFile {
    /// The project's own `.yard`: the user's, never edited.
    Committed(PathBuf),
    /// The managed `.yard` written for the site's path (managed-file spec §4).
    Managed(PathBuf),
    /// Its `sites.toml` entry: a proxy, a site with no path, or one with no file of its own.
    SitesToml,
}

/// Which file holds `site`'s settings. A managed file written for another directory is not
/// the site's; one that does not parse is its error.
pub fn site_file(paths: &Paths, site: &str) -> Result<SiteFile, YardError> {
    let sites = load_sites(paths)?;
    let Some(path) = sites.sites.get(site).and_then(|e| e.path.as_ref()) else {
        return Ok(SiteFile::SitesToml);
    };
    let file = path.join(project::FILE);
    if file.symlink_metadata().is_ok() {
        return Ok(SiteFile::Committed(file));
    }
    let managed = paths.managed_project_file(site);
    let Some(text) = read_project_file(&managed) else { return Ok(SiteFile::SitesToml) };
    match project::parse_managed(&text?, &managed)? {
        (_, Some(project)) if project == *path => Ok(SiteFile::Managed(managed)),
        _ => Ok(SiteFile::SitesToml),
    }
}

/// Sets (or with `None` removes) `[runtimes] <key>` for a linked PHP site: in its managed
/// `.yard`, else in `[sites.<site>.runtimes]`. A committed `.yard` is the user's to edit:
/// `SITE_HAS_PROJECT_FILE`, naming the line to add.
pub fn set_site_runtime(paths: &Paths, site: &str, key: &str, spec: Option<&str>) -> Result<(), YardError> {
    let set = |toml: &mut TomlFile, keys: &[&str]| match spec {
        Some(spec) => toml.set(keys, toml_edit::value(spec)),
        None => {
            toml.remove(keys);
        }
    };
    match site_file(paths, site)? {
        SiteFile::Committed(file) => {
            let change = match spec {
                Some(spec) => format!("[runtimes] {key} = \"{spec}\""),
                None => format!("remove {key} from [runtimes]"),
            };
            Err(YardError::new(
                ErrorCode::SiteHasProjectFile,
                format!("{site} is configured by {}", file.display()),
            )
            .with_hint(format!("edit {}: {change}", file.display())))
        }
        SiteFile::Managed(file) => edit_managed(&file, |toml| {
            set(toml, &["runtimes", key]);
            Ok(())
        }),
        SiteFile::SitesToml => TomlFile::edit(&paths.sites_toml(), |toml| {
            let entry = toml.doc().get("sites").and_then(|s| s.get(site));
            if entry.is_none() {
                return Err(YardError::new(ErrorCode::SiteNotFound, format!("no site named {site:?}"))
                    .with_hint("yard sites lists them"));
            }
            if entry.and_then(|e| e.get("path")).is_none() {
                return Err(YardError::new(
                    ErrorCode::InvalidArgument,
                    format!("{site} is a proxy, so no {key} runs for it"),
                )
                .with_hint("yard sites shows which sites run PHP"));
            }
            set(toml, &["sites", site, "runtimes", key]);
            Ok(())
        }),
    }
}

/// Applies `f` to the managed `.yard` at `file` under `TomlFile`'s lock, and saves it
/// only if the result is still a valid managed file. A symlinked file is never written
/// through, and one that does not parse is never edited.
pub fn edit_managed(
    file: &Path,
    f: impl FnOnce(&mut TomlFile) -> Result<(), YardError>,
) -> Result<(), YardError> {
    if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        // The link's target as written, joined to its directory when relative.
        let target = std::fs::read_link(file)
            .map(|t| file.parent().map_or(t.clone(), |dir| dir.join(&t)))
            .unwrap_or_else(|_| file.to_path_buf());
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            format!("{} is a symlink, so Yard does not edit it", file.display()),
        )
        .with_hint(format!("edit {}", target.display())));
    }
    let text = read_project_file(file).unwrap_or_else(|| {
        Err(YardError::new(ErrorCode::ProjectFileInvalid, format!("{} is gone", file.display()))
            .with_hint("yard init"))
    })?;
    project::parse_managed(&text, file)?;
    TomlFile::edit(file, |toml| {
        f(toml)?;
        project::parse_managed(&toml.doc().to_string(), file)?;
        Ok(())
    })
}

/// Runs `write` under `TomlFile`'s lock while the managed `.yard` at `file` still holds
/// `read`, the text `yard init` decided on (`None`: there was none). An edit made since,
/// such as `yard isolate`'s, is never written over: init fails and is run again.
pub fn write_if_unchanged(
    file: &Path,
    read: Option<&str>,
    write: impl FnOnce() -> Result<(), YardError>,
) -> Result<(), YardError> {
    TomlFile::locked(|| {
        let now = read_project_file(file).transpose()?;
        if now.as_deref() != read {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!("{} changed while yard init ran", file.display()),
            )
            .with_hint("yard init"));
        }
        write()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_only_while_the_file_holds_what_was_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".yard");
        std::fs::write(&file, "php = \"7.3\"\n").unwrap();
        let wrote = std::cell::Cell::new(false);
        write_if_unchanged(&file, Some("php = \"7.3\"\n"), || {
            wrote.set(true);
            Ok(())
        })
        .unwrap();
        assert!(wrote.get());

        // `yard isolate` changed it after init read it: init writes nothing.
        wrote.set(false);
        let e = write_if_unchanged(&file, Some("php = \"7.1\"\n"), || {
            wrote.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(!wrote.get());
        assert!(e.message.contains("changed while yard init ran"), "{e:?}");
        assert_eq!(e.hint.as_deref(), Some("yard init"));
        // Made since init found none.
        assert!(write_if_unchanged(&file, None, || Ok(())).is_err());
    }
}
