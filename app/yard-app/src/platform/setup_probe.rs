//! The one onboarding fact the app checks on disk: Homebrew at its Apple Silicon prefix.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const BREW: &str = "/opt/homebrew/bin/brew";

pub fn brew_installed() -> bool {
    std::fs::metadata(Path::new(BREW)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
