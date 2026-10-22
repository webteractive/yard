//! The three user-owned TOML files: `yard.toml`, `sites.toml`, `ports.toml`.

pub mod edit;
pub mod sites;
pub mod yard;

pub use edit::TomlFile;
pub use sites::{SiteEntry, SitesConfig, load_sites};
pub use yard::{YardConfig, load_yard};

use std::ops::Range;
use std::path::Path;
use yard_proto::{ErrorCode, YardError};

/// `CONFIG_INVALID` pointing at the offending line and column, so a typo in a
/// hand-edited file is found without guessing.
pub(crate) fn config_error(file: &Path, text: &str, span: Option<Range<usize>>, message: &str) -> YardError {
    let hint = format!("fix {}; Yard keeps serving the last good configuration until then", file.display());
    located_error(ErrorCode::ConfigInvalid, file, text, span, message, hint)
}

/// An error with `code` naming `file`, line and column, with `data { file, line, column }`.
pub(crate) fn located_error(
    code: ErrorCode,
    file: &Path,
    text: &str,
    span: Option<Range<usize>>,
    message: &str,
    hint: String,
) -> YardError {
    let (line, column) = span.map(|s| line_col(text, s.start)).unwrap_or((1, 1));
    YardError::new(code, format!("{} line {line}, column {column}: {}", file.display(), message.trim()))
        .with_hint(hint)
        .with_data(serde_json::json!({ "file": file.display().to_string(), "line": line, "column": column }))
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

pub(crate) fn read_optional(file: &Path) -> Result<Option<String>, YardError> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(YardError::new(ErrorCode::Internal, format!("cannot read {}: {e}", file.display()))),
    }
}
