//! What the app offers when a newer Yard is released (M7 spec §6.3). The check itself is `yard
//! self-update --check --json`, run by yard-app; this decides what to show and whether to install.

use serde::Deserialize;
use std::time::Duration;

/// How often the app asks for a newer release, besides at launch.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// What a Homebrew install updates with; the app copies it rather than running brew.
pub const BREW_UPGRADE: &str = "brew upgrade --cask yard";

/// `yard self-update --check --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Check {
    pub installed: String,
    pub latest: String,
    pub available: bool,
    /// How Yard was installed: `bundle` (install.sh), `brew` or `dev`.
    pub kind: String,
    pub notes_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAction {
    /// Run `yard self-update --yes`, then relaunch.
    Install,
    /// Copy [`BREW_UPGRADE`]: brew owns this install.
    CopyBrewCommand,
}

/// The menu item and Overview button for an available update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateItem {
    pub title: String,
    pub action: UpdateAction,
}

/// None when nothing newer is out, or for a development build, which updates by rebuilding.
pub fn item(check: &Check) -> Option<UpdateItem> {
    if !check.available {
        return None;
    }
    match check.kind.as_str() {
        "bundle" => Some(UpdateItem {
            title: format!("Update to {}…", check.latest),
            action: UpdateAction::Install,
        }),
        "brew" => {
            Some(UpdateItem { title: "Update with Homebrew".into(), action: UpdateAction::CopyBrewCommand })
        }
        _ => None,
    }
}

/// Installs without asking only an install.sh bundle, only with Settings' switch on.
pub fn auto_install(check: &Check, auto_update: bool) -> bool {
    auto_update && check.available && check.kind == "bundle"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(available: bool, kind: &str) -> Check {
        Check {
            installed: "0.1.0".into(),
            latest: "0.2.0".into(),
            available,
            kind: kind.into(),
            notes_url: "https://example.invalid/n".into(),
        }
    }

    #[test]
    fn the_item_follows_the_check() {
        assert_eq!(
            item(&check(true, "bundle")),
            Some(UpdateItem { title: "Update to 0.2.0…".into(), action: UpdateAction::Install })
        );
        assert_eq!(
            item(&check(true, "brew")),
            Some(UpdateItem { title: "Update with Homebrew".into(), action: UpdateAction::CopyBrewCommand })
        );
        assert_eq!(item(&check(true, "dev")), None);
        assert_eq!(item(&check(false, "bundle")), None);
    }

    #[test]
    fn only_a_bundle_installs_by_itself_and_only_when_allowed() {
        assert!(auto_install(&check(true, "bundle"), true));
        assert!(!auto_install(&check(true, "bundle"), false));
        assert!(!auto_install(&check(true, "brew"), true));
        assert!(!auto_install(&check(false, "bundle"), true));
    }

    #[test]
    fn the_check_reads_what_the_cli_prints() {
        let json =
            r#"{"installed":"0.1.0","latest":"0.2.0","available":true,"kind":"bundle","notes_url":"x"}"#;
        assert_eq!(
            serde_json::from_str::<Check>(json).unwrap(),
            Check { notes_url: "x".into(), ..check(true, "bundle") }
        );
    }
}
