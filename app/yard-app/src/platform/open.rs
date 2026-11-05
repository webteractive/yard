//! Opening URLs, folders and files, and the clipboard.

use gpui::{App, ClipboardItem};
use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::Path;
use yard_proto::YardError;

/// Where a hinted file opens when no editor is set: a text editor, never the default handler.
const TEXT_EDIT: &str = "/System/Applications/TextEdit.app";

/// Types macOS launches rather than shows, even in an editor's place.
const LAUNCHABLE: &[&str] = &[
    "app",
    "command",
    "tool",
    "terminal",
    "workflow",
    "scpt",
    "applescript",
    "sh",
    "zsh",
    "bash",
    "pkg",
    "mpkg",
    "dmg",
    "webloc",
    "inetloc",
    "fileloc",
    "prefpane",
    "action",
    "shortcut",
];

/// The file a hint asks to edit, if it is safe to hand to an editor: a regular file, not a
/// symlink, not executable, and not a type macOS would launch. Returns its canonical path.
pub fn checked_text_file(file: &Path) -> Result<std::path::PathBuf, YardError> {
    use std::os::unix::fs::PermissionsExt;
    let refuse = |why: &str| {
        YardError::new(
            yard_proto::ErrorCode::InvalidArgument,
            format!("{} is not opened: {why}", file.display()),
        )
    };
    let link = std::fs::symlink_metadata(file).map_err(|e| refuse(&e.to_string()))?;
    if link.file_type().is_symlink() {
        return Err(refuse("it is a symlink"));
    }
    let path = std::fs::canonicalize(file).map_err(|e| refuse(&e.to_string()))?;
    let meta = std::fs::metadata(&path).map_err(|e| refuse(&e.to_string()))?;
    if !meta.is_file() {
        return Err(refuse("it is not a file"));
    }
    if meta.permissions().mode() & 0o111 != 0 {
        return Err(refuse("it is executable"));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if LAUNCHABLE.contains(&ext.as_str()) {
        return Err(refuse("macOS would run it"));
    }
    Ok(path)
}

pub struct Opener;

impl Opener {
    pub fn open_url(cx: &App, url: &str) {
        cx.open_url(url);
    }

    /// Shows `path` in Finder.
    pub fn reveal(cx: &App, path: &Path) {
        cx.reveal_path(path);
    }

    pub fn copy(cx: &App, text: &str) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    }

    /// Opens a file a hint names, as text: checked by [`checked_text_file`], then the editor app,
    /// else TextEdit, never the file's default handler.
    pub fn open_text(file: &Path, editor: Option<&Path>) -> Result<(), YardError> {
        let file = checked_text_file(file)?;
        Self::open_with(&file, Some(editor.unwrap_or(Path::new(TEXT_EDIT))))
    }

    /// Opens `file` with `app` (an `.app` bundle path), or with its default app.
    pub fn open_with(file: &Path, app: Option<&Path>) -> Result<(), YardError> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&file.display().to_string()));
        let workspace = NSWorkspace::sharedWorkspace();
        match app {
            Some(app) => {
                let app_url = NSURL::fileURLWithPath(&NSString::from_str(&app.display().to_string()));
                let urls = NSArray::from_retained_slice(&[url]);
                let config = NSWorkspaceOpenConfiguration::configuration();
                workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                    &urls, &app_url, &config, None,
                );
                Ok(())
            }
            None if workspace.openURL(&url) => Ok(()),
            None => Err(YardError::internal(format!("macOS would not open {}", file.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_hinted_file_opens_only_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let yard = dir.path().join(".yard");
        std::fs::write(&yard, "php = \"8.4\"\n").unwrap();
        assert_eq!(checked_text_file(&yard).unwrap(), std::fs::canonicalize(&yard).unwrap());

        let command = dir.path().join("x.command");
        std::fs::write(&command, "#!/bin/sh\n").unwrap();
        assert!(checked_text_file(&command).is_err(), "a .command runs in Terminal");

        let script = dir.path().join("run");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(checked_text_file(&script).is_err(), "an executable is not text to edit");

        assert!(checked_text_file(dir.path()).is_err(), "a folder (or an .app bundle) is not a file");
        let link = dir.path().join("link.conf");
        std::os::unix::fs::symlink(&yard, &link).unwrap();
        assert!(checked_text_file(&link).is_err(), "a symlink is refused");
        assert!(checked_text_file(&dir.path().join("missing.conf")).is_err());
    }
}
