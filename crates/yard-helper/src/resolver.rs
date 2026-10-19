//! `/etc/resolver/<tld>`: written and removed only when it carries Yard's marker.

use std::io;
use std::path::{Path, PathBuf};
use yard_core::paths::RESOLVER_MARKER;
use yard_core::render::resolver::resolver_file;
use yard_proto::{ErrorCode, YardError};

/// `^[a-z][a-z0-9-]{0,62}$`, so a TLD can never name a path outside the resolver dir.
fn valid_tld(tld: &str) -> bool {
    let mut bytes = tld.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && tld.len() <= 63
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn checked(dir: &Path, tld: &str) -> Result<PathBuf, YardError> {
    if valid_tld(tld) {
        Ok(dir.join(tld))
    } else {
        Err(YardError::new(ErrorCode::InvalidArgument, format!("{tld:?} is not a valid TLD"))
            .with_hint("use lowercase letters, digits and dashes, starting with a letter"))
    }
}

/// `None` when the file is absent, else whether its first line is the marker.
fn is_ours(file: &Path) -> Result<Option<bool>, YardError> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text.lines().next() == Some(RESOLVER_MARKER))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(YardError::internal(format!("cannot read {}: {e}", file.display()))),
    }
}

pub fn install(dir: &Path, tld: &str, port: u16) -> Result<(), YardError> {
    let file = checked(dir, tld)?;
    if port < 1024 {
        return Err(YardError::new(ErrorCode::InvalidArgument, format!("port {port} is below 1024"))
            .with_hint("the resolver port must be in 1024–65535"));
    }
    if is_ours(&file)? == Some(false) {
        return Err(YardError::new(
            ErrorCode::ResolverForeign,
            format!("{} belongs to another tool; Yard left it alone", file.display()),
        )
        .with_hint(format!(
            "set `tld` in yard.toml to another name, or remove {} if nothing uses it",
            file.display()
        )));
    }
    let write = || -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{tld}.yard-tmp"));
        std::fs::write(&tmp, resolver_file(port))?;
        std::fs::rename(&tmp, &file)
    };
    write().map_err(|e| YardError::internal(format!("cannot write {}: {e}", file.display())))
}

/// Removes Yard's file; a foreign or missing one is left as is.
pub fn remove(dir: &Path, tld: &str) -> Result<(), YardError> {
    let file = checked(dir, tld)?;
    if is_ours(&file)? == Some(true) {
        std::fs::remove_file(&file)
            .map_err(|e| YardError::internal(format!("cannot remove {}: {e}", file.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tld_pattern() {
        for ok in ["yard", "test", "a", "dev-1", &"a".repeat(63)] {
            assert!(valid_tld(ok), "{ok}");
        }
        for bad in ["", "Yard", "1yard", "-yard", "ya.rd", "../etc", "ya/rd", &"a".repeat(64)] {
            assert!(!valid_tld(bad), "{bad}");
        }
    }
}
