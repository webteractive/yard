//! `yard install-shims`: the PATH line for the shims in `<root>/bin`. Offline, and it
//! edits no dotfile; the user pastes the line.

use super::{Ctx, Outcome};
use serde::Serialize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct ShimsInfo {
    bin: String,
    line: String,
    /// The `php` the current PATH finds first, if any.
    first_php: Option<String>,
}

/// A double-quoted shell word that still expands `$PATH` after it.
fn export_line(bin: &Path) -> String {
    let quoted: String = bin
        .display()
        .to_string()
        .chars()
        .flat_map(|c| if matches!(c, '"' | '\\' | '$' | '`') { vec!['\\', c] } else { vec![c] })
        .collect();
    format!("export PATH=\"{quoted}:$PATH\"")
}

fn first_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

pub fn install(ctx: &Ctx) -> Outcome {
    let bin = ctx.paths.bin_dir();
    let line = export_line(&bin);
    let first = first_on_path("php");
    let info = ShimsInfo {
        bin: bin.display().to_string(),
        line: line.clone(),
        first_php: first.as_ref().map(|p| p.display().to_string()),
    };
    ctx.emit(&info, || {
        format!("{line}\n\nAdd that line to your shell profile (~/.zshrc), then open a new terminal.\n")
    });
    if !ctx.json
        && let Some(php) = first.filter(|p| *p != bin.join("php"))
    {
        let herd = if php.to_string_lossy().contains("/Herd/") { " (Herd's)" } else { "" };
        eprintln!("warning: php on your PATH is {}{herd} — Yard's bin must come before it", php.display());
    }
    Ok(0)
}
