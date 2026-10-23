//! Ports that other database apps use, kept free while they are installed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// DBngin's MySQL, PostgreSQL and Redis defaults.
const DBNGIN_PORTS: [u16; 3] = [3306, 5432, 6379];
/// Postgres.app's default.
const POSTGRES_APP_PORTS: [u16; 1] = [5432];

/// The ports to keep free while DBngin.app or Postgres.app is installed, so a Yard service
/// never pins a port the app takes when it starts (a probe only sees live listeners).
/// `apps` are the directories to look in (`/Applications` and `~/Applications`); `exists`
/// is injected so this stays pure. Only the app bundle's presence is read.
pub fn reserved_ports(apps: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> BTreeSet<u16> {
    let installed = |bundle: &str| apps.iter().any(|dir| exists(&dir.join(bundle)));
    let mut ports = BTreeSet::new();
    if installed("DBngin.app") {
        ports.extend(DBNGIN_PORTS);
    }
    if installed("Postgres.app") {
        ports.extend(POSTGRES_APP_PORTS);
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(present: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p: &Path| present.iter().any(|name| p == Path::new(name))
    }

    fn apps() -> Vec<PathBuf> {
        vec![PathBuf::from("/Applications"), PathBuf::from("/Users/me/Applications")]
    }

    #[test]
    fn reserved_ports_for_installed_apps() {
        let dbngin = reserved_ports(&apps(), &installed(&["/Applications/DBngin.app"]));
        assert_eq!(dbngin, BTreeSet::from([3306, 5432, 6379]));
        let postgres = reserved_ports(&apps(), &installed(&["/Applications/Postgres.app"]));
        assert_eq!(postgres, BTreeSet::from([5432]));
        assert!(reserved_ports(&apps(), &installed(&[])).is_empty());
    }

    #[test]
    fn reserved_ports_see_the_user_applications_directory_and_both_apps() {
        let both = reserved_ports(
            &apps(),
            &installed(&["/Users/me/Applications/DBngin.app", "/Applications/Postgres.app"]),
        );
        assert_eq!(both, BTreeSet::from([3306, 5432, 6379]));
        assert!(reserved_ports(&[], &installed(&["/Applications/DBngin.app"])).is_empty());
    }
}
