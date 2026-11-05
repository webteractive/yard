//! Settings in NSUserDefaults (M6 design §8.3).

use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSString, NSUserDefaults};
use std::path::PathBuf;

const YARD_PATH: &str = "YardCLIPath";
const EDITOR_APP: &str = "EditorApp";
const TERMINAL_APP: &str = "TerminalApp";
const RECENT_SITES: &str = "RecentSites";
const DID_ONBOARD: &str = "DidOnboard";

/// The app's own settings, in its `NSUserDefaults` domain (`dev.more.yard.app`); nothing of
/// Yard's data root.
pub struct Settings {
    defaults: Retained<NSUserDefaults>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { defaults: NSUserDefaults::standardUserDefaults() }
    }
}

impl Settings {
    fn string(&self, key: &str) -> Option<String> {
        self.defaults.stringForKey(&NSString::from_str(key)).map(|s| s.to_string()).filter(|s| !s.is_empty())
    }

    fn set_string(&self, key: &str, value: Option<&str>) {
        let key = NSString::from_str(key);
        match value {
            // SAFETY: an NSString is a property-list object, which is what NSUserDefaults stores.
            Some(v) => unsafe { self.defaults.setObject_forKey(Some(&NSString::from_str(v)), &key) },
            None => self.defaults.removeObjectForKey(&key),
        }
    }

    pub fn yard_path(&self) -> Option<PathBuf> {
        self.string(YARD_PATH).map(PathBuf::from)
    }

    pub fn set_yard_path(&self, path: Option<&std::path::Path>) {
        self.set_string(YARD_PATH, path.map(|p| p.display().to_string()).as_deref());
    }

    pub fn editor_app(&self) -> Option<PathBuf> {
        self.string(EDITOR_APP).map(PathBuf::from)
    }

    pub fn set_editor_app(&self, app: Option<&std::path::Path>) {
        self.set_string(EDITOR_APP, app.map(|p| p.display().to_string()).as_deref());
    }

    pub fn terminal_app(&self) -> Option<PathBuf> {
        self.string(TERMINAL_APP).map(PathBuf::from)
    }

    pub fn set_terminal_app(&self, app: Option<&std::path::Path>) {
        self.set_string(TERMINAL_APP, app.map(|p| p.display().to_string()).as_deref());
    }

    /// The five sites opened last, newest first.
    pub fn recent_sites(&self) -> Vec<String> {
        self.defaults
            .stringArrayForKey(&NSString::from_str(RECENT_SITES))
            .map(|a| a.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn note_recent_site(&self, site: &str) {
        let mut sites = self.recent_sites();
        sites.retain(|s| s != site);
        sites.insert(0, site.to_string());
        sites.truncate(5);
        let strings: Vec<Retained<NSString>> = sites.iter().map(|s| NSString::from_str(s)).collect();
        let array = NSArray::from_retained_slice(&strings);
        // SAFETY: an array of NSStrings is a property-list object.
        unsafe { self.defaults.setObject_forKey(Some(&array), &NSString::from_str(RECENT_SITES)) };
    }

    pub fn did_onboard(&self) -> bool {
        self.defaults.boolForKey(&NSString::from_str(DID_ONBOARD))
    }

    pub fn set_did_onboard(&self) {
        self.defaults.setBool_forKey(true, &NSString::from_str(DID_ONBOARD));
    }
}
