//! Opening URLs, folders and files, and the clipboard.

use gpui::{App, ClipboardItem};
use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::Path;
use yard_proto::{ErrorCode, YardError};

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

fn refusal(file: &Path, why: &str) -> YardError {
    YardError::new(yard_proto::ErrorCode::InvalidArgument, format!("{} is not opened: {why}", file.display()))
}

fn launchable(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    LAUNCHABLE.contains(&ext.as_str())
}

/// The file a hint asks to edit, if it is safe to hand to an editor: a regular file, not a
/// symlink, not executable, and not a type macOS would launch. Returns its canonical path.
///
/// The file is opened once, without following a final symlink and without blocking (a FIFO),
/// and every check reads that descriptor, so nothing swapped in between two lookups can pass.
/// The editor then opens the path itself, a moment later; what reaches it is always an explicit
/// editor app (TextEdit by default), never the file's default handler, so a file swapped in that
/// moment is at worst shown as text.
pub fn checked_text_file(file: &Path) -> Result<std::path::PathBuf, YardError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let refuse = |why: &str| refusal(file, why);
    let name = file.file_name().ok_or_else(|| refuse("it names no file"))?;
    let dir = file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let path = std::fs::canonicalize(dir).map_err(|e| refuse(&e.to_string()))?.join(name);
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => refuse("it is a symlink"),
            _ => refuse(&e.to_string()),
        })?;
    let meta = opened.metadata().map_err(|e| refuse(&e.to_string()))?;
    if !meta.is_file() {
        return Err(refuse("it is not a file"));
    }
    if meta.permissions().mode() & 0o111 != 0 {
        return Err(refuse("it is executable"));
    }
    if launchable(&path) {
        return Err(refuse("macOS would run it"));
    }
    Ok(path)
}

/// How "Open in editor" opens a project folder.
#[derive(Debug, PartialEq, Eq)]
pub enum FolderOpen {
    /// Handed to the editor app.
    With(std::path::PathBuf, std::path::PathBuf),
    /// No editor: opened in Finder, the default for a folder.
    Default(std::path::PathBuf),
    /// No editor, and macOS would launch the folder rather than show it (an `.app` bundle, or one
    /// it calls a package): selected in Finder instead.
    Reveal(std::path::PathBuf),
}

/// What "Open in editor" does with `dir`: it must be a folder; with no editor, a folder macOS
/// would launch is revealed, never opened. `package` is macOS's own answer for the folder.
pub fn folder_target(dir: &Path, editor: Option<&Path>, package: bool) -> Result<FolderOpen, YardError> {
    let path = std::fs::canonicalize(dir).map_err(|e| refusal(dir, &e.to_string()))?;
    if !path.is_dir() {
        return Err(refusal(dir, "it is not a folder"));
    }
    Ok(match editor {
        Some(app) => FolderOpen::With(path, app.to_path_buf()),
        None if package || launchable(&path) => FolderOpen::Reveal(path),
        None => FolderOpen::Default(path),
    })
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

    /// Opens a project folder in the editor app, else in Finder; see [`folder_target`].
    pub fn open_folder(cx: &App, dir: &Path, editor: Option<&Path>) -> Result<(), YardError> {
        let package = NSWorkspace::sharedWorkspace()
            .isFilePackageAtPath(&NSString::from_str(&dir.display().to_string()));
        match folder_target(dir, editor, package)? {
            FolderOpen::With(path, app) => Self::open_with(&path, Some(&app)),
            FolderOpen::Default(path) => Self::open_with(&path, None),
            FolderOpen::Reveal(path) => {
                Self::reveal(cx, &path);
                Ok(())
            }
        }
    }

    /// Opens `file` with `app` (an `.app` bundle path), or with its default app. An app that is
    /// not there is an error here: macOS reports that only to a completion handler.
    pub fn open_with(file: &Path, app: Option<&Path>) -> Result<(), YardError> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&file.display().to_string()));
        let workspace = NSWorkspace::sharedWorkspace();
        match app {
            Some(app) if !app.is_dir() => {
                Err(YardError::new(ErrorCode::InvalidArgument, format!("{} is not there", app.display()))
                    .with_hint("choose another app in Yard's Settings"))
            }
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

    /// An app chosen in Settings that is gone says so; macOS would fail without a word (M6 final
    /// review: a missing editor or terminal app did nothing).
    #[test]
    fn a_missing_app_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.conf");
        std::fs::write(&file, "x").unwrap();
        let e = Opener::open_with(&file, Some(Path::new("/Applications/Gone Editor.app"))).unwrap_err();
        assert_eq!(e.message, "/Applications/Gone Editor.app is not there");
        assert_eq!(e.hint.as_deref(), Some("choose another app in Yard's Settings"));
    }

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
        let refused = checked_text_file(&link).unwrap_err();
        assert!(refused.message.contains("symlink"), "{}", refused.message);
        assert!(checked_text_file(&dir.path().join("missing.conf")).is_err());
    }

    /// The check opens the file once, without following a link and without blocking: a FIFO in a
    /// hint's place is refused at once rather than hanging the app on its open.
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.conf");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let refused = checked_text_file(&fifo).unwrap_err();
        assert!(refused.message.contains("not a file"), "{}", refused.message);
    }

    /// "Open in editor" with no editor set shows the project folder in Finder; a folder macOS
    /// would launch (an `.app` bundle) is revealed instead, never opened.
    #[test]
    fn a_project_folder_is_never_launched() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("site");
        let bundle = dir.path().join("Trap.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::create_dir(&plain).unwrap();
        let editor = Path::new("/Applications/Editor.app");
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap();

        assert_eq!(folder_target(&plain, None, false).unwrap(), FolderOpen::Default(canon(&plain)));
        assert_eq!(folder_target(&bundle, None, false).unwrap(), FolderOpen::Reveal(canon(&bundle)));
        assert_eq!(
            folder_target(&plain, None, true).unwrap(),
            FolderOpen::Reveal(canon(&plain)),
            "a folder macOS calls a package is revealed too"
        );
        assert_eq!(
            folder_target(&bundle, Some(editor), false).unwrap(),
            FolderOpen::With(canon(&bundle), editor.to_path_buf()),
            "an editor is handed the folder whatever its name"
        );
        let file = dir.path().join("index.php");
        std::fs::write(&file, "<?php").unwrap();
        assert!(folder_target(&file, None, false).is_err(), "a file is not a project folder");
        assert!(folder_target(&dir.path().join("gone"), None, false).is_err());
    }
}
