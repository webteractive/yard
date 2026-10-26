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
    /// The `node` the current PATH finds first, if any.
    first_node: Option<String>,
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

/// Whose `php` or `node` a path is, as ` (Herd's)`, or nothing when it is not a manager's.
fn whose(path: &str) -> &'static str {
    if path.contains("/Herd/") {
        " (Herd's)"
    } else if path.contains("/.nvm/") {
        " (nvm's)"
    } else if path.contains("/mise/") {
        " (mise's)"
    } else if path.contains("fnm_multishells") {
        " (fnm's)"
    } else if path.starts_with("/opt/homebrew/") {
        " (brew's)"
    } else {
        ""
    }
}

pub fn install(ctx: &Ctx) -> Outcome {
    let bin = ctx.paths.bin_dir();
    let line = export_line(&bin);
    let (php, node) = (first_on_path("php"), first_on_path("node"));
    let info = ShimsInfo {
        bin: bin.display().to_string(),
        line: line.clone(),
        first_php: php.as_ref().map(|p| p.display().to_string()),
        first_node: node.as_ref().map(|p| p.display().to_string()),
    };
    ctx.emit(&info, || {
        format!("{line}\n\nAdd that line to your shell profile (~/.zshrc), then open a new terminal.\n")
    });
    if !ctx.json {
        for (name, first) in [("php", php), ("node", node)] {
            if let Some(path) = first.filter(|p| *p != bin.join(name)) {
                let path = path.display().to_string();
                eprintln!(
                    "warning: {name} on your PATH is {path}{} — Yard's bin must come before it",
                    whose(&path)
                );
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::whose;

    #[test]
    fn whose_names_the_managers() {
        assert_eq!(whose("/Users/me/Library/Application Support/Herd/bin/node"), " (Herd's)");
        assert_eq!(whose("/Users/me/.nvm/versions/node/v20.11.0/bin/node"), " (nvm's)");
        assert_eq!(whose("/Users/me/.local/share/mise/installs/node/22/bin/node"), " (mise's)");
        assert_eq!(whose("/Users/me/.local/state/fnm_multishells/123_456/bin/node"), " (fnm's)");
        assert_eq!(whose("/opt/homebrew/bin/node"), " (brew's)");
        assert_eq!(whose("/usr/local/bin/node"), "");
    }
}
