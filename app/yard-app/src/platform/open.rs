//! Opening URLs, folders and files, and the clipboard.

use gpui::{App, ClipboardItem};
use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::Path;
use yard_proto::YardError;

pub struct Opener;

impl Opener {
    pub fn open_url(cx: &App, url: &str) {
        cx.open_url(url);
    }

    /// Shows `path` in Finder.
    #[expect(dead_code, reason = "opening a log or data folder (M6 Task 15)")]
    pub fn reveal(cx: &App, path: &Path) {
        cx.reveal_path(path);
    }

    pub fn copy(cx: &App, text: &str) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
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
