//! What the `php`/`composer`/`pecl` shims and every PHP command Yard runs see (spec §4.5).
//! Herd puts its own `php` and `composer` on PATH, so nothing Yard starts may find them.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// The PATH for a child: `keg_bin` first, then `inherited` without Herd's directories
/// (any entry with a `Herd` component), empty entries or a second `keg_bin`.
pub fn child_path(keg_bin: Option<&Path>, inherited: &OsStr) -> OsString {
    let herd = |p: &Path| p.components().any(|c| c == Component::Normal(OsStr::new("Herd")));
    let rest = std::env::split_paths(inherited)
        .filter(|p| !p.as_os_str().is_empty() && !herd(p) && Some(p.as_path()) != keg_bin);
    let all: Vec<PathBuf> = keg_bin.map(Path::to_path_buf).into_iter().chain(rest).collect();
    std::env::join_paths(all).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_path_drops_herd_entries() {
        let inherited = "/Users/g/Library/Application Support/Herd/bin/:/opt/homebrew/bin:/usr/bin";
        let keg = Path::new("/opt/homebrew/opt/php@7.1/bin");
        assert_eq!(
            child_path(Some(keg), inherited.as_ref()),
            OsString::from("/opt/homebrew/opt/php@7.1/bin:/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn child_path_without_a_keg_only_filters() {
        let inherited = "/usr/bin::/x/Herd:/opt/php@7.1/bin";
        assert_eq!(child_path(None, inherited.as_ref()), OsString::from("/usr/bin:/opt/php@7.1/bin"));
        let keg = Path::new("/opt/php@7.1/bin");
        assert_eq!(child_path(Some(keg), inherited.as_ref()), OsString::from("/opt/php@7.1/bin:/usr/bin"));
    }

    #[test]
    fn herd_in_a_longer_name_is_kept() {
        assert_eq!(child_path(None, "/opt/Herdsman/bin".as_ref()), OsString::from("/opt/Herdsman/bin"));
    }
}
