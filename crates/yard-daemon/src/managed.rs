//! Editing a site's managed `.yard` for `yard isolate`, `secure` and `docroot`
//! (managed-file spec §4): Yard owns the file's place, so these commands change it
//! instead of being refused, keeping every other key and comment.

use crate::project::read_project_file;
use std::path::Path;
use yard_core::config::TomlFile;
use yard_core::project;
use yard_proto::{ErrorCode, YardError};

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
