//! An import file's format from its first bytes (db spec §6.6).

use yard_proto::{ErrorCode, ImportKind, YardError};

/// gzip's magic number.
pub fn is_gzip(head: &[u8]) -> bool {
    head.starts_with(&[0x1f, 0x8b])
}

/// `head` is the file's first bytes (after gunzip); `is_dir` and `dir_has_toc` describe a
/// directory, which only `pg_dump -Fd` makes. Content decides, never the name.
pub fn kind(head: &[u8], is_dir: bool, dir_has_toc: bool) -> Result<ImportKind, YardError> {
    if is_dir {
        return if dir_has_toc {
            Ok(ImportKind::Directory)
        } else {
            Err(YardError::new(
                ErrorCode::InvalidArgument,
                "a directory imports only when pg_dump -Fd made it, and this one has no toc.dat",
            ))
        };
    }
    if head.starts_with(b"PGDMP") || head.get(257..262) == Some(b"ustar".as_slice()) {
        return Ok(ImportKind::Archive);
    }
    Ok(ImportKind::Plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::{ErrorCode, ImportKind};

    #[test]
    fn detect_kinds() {
        assert!(is_gzip(&[0x1f, 0x8b, 8, 0]));
        assert!(!is_gzip(b"-- MySQL dump"));
        assert_eq!(kind(b"PGDMP\x01\x0e\x00", false, false).unwrap(), ImportKind::Archive);
        let mut tar = vec![0u8; 512];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(kind(&tar, false, false).unwrap(), ImportKind::Archive);
        assert_eq!(kind(b"-- MySQL dump 10.13", false, false).unwrap(), ImportKind::Plain);
        assert_eq!(kind(b"", true, true).unwrap(), ImportKind::Directory);
        assert_eq!(kind(b"", true, false).unwrap_err().code, ErrorCode::InvalidArgument);
    }
}
