//! `sites.init`: write a project's `.yard`, link the project, and move its settings out
//! of `sites.toml` (project-file spec §4).

use super::handlers::{directory, reconcile, site, valid_name};
use crate::daemon::Context;
use crate::project::{read_project_file, read_small};
use std::io::Write;
use std::path::{Path, PathBuf};
use toml_edit::value;
use yard_core::config::{SiteEntry, TomlFile, load_sites, load_yard};
use yard_core::project::{self, Detected};
use yard_core::sites::host_label;
use yard_proto::methods::InitParams;
use yard_proto::{ErrorCode, InitResult, YardError};

/// The largest `composer.json` `init` reads for `require.php`.
const COMPOSER_JSON_MAX: u64 = 1 << 20;

/// Writes the file unless one is there (or rewrites it with `force`), then links the
/// project and drops `php`, `secure` and `docroot` from its `sites.toml` entry. A `.yard`
/// that does not parse is refused before anything is linked or moved.
pub async fn init(ctx: &Context, p: InitParams) -> Result<InitResult, YardError> {
    let dir = directory(&p.path)?;
    let sites = load_sites(&ctx.paths)?;
    // sites.toml may spell the project's path another way, through a symlink.
    let linked = sites
        .sites
        .iter()
        .find(|(_, e)| e.path.as_deref().and_then(|p| std::fs::canonicalize(p).ok()).as_ref() == Some(&dir));
    let name = match (linked, &p.name) {
        (Some((linked, _)), Some(name)) if name != linked => {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!("{} is already linked as {linked}", dir.display()),
            )
            .with_hint(format!("yard unlink {linked}")));
        }
        (Some((linked, _)), _) => linked.clone(),
        (None, Some(name)) => name.clone(),
        (None, None) => dir_label(&dir)?,
    };
    valid_name(&name)?;
    let entry = linked.map(|(_, e)| e);
    // Unlike `yard link`, init would also strip the other site's settings: refuse.
    if let (None, Some(other)) = (entry, sites.sites.get(&name)) {
        let serves = match (&other.path, &other.proxy) {
            (Some(path), _) => path.display().to_string(),
            (None, Some(url)) => url.clone(),
            (None, None) => "another project".to_string(),
        };
        return Err(YardError::new(ErrorCode::InvalidArgument, format!("{name} already serves {serves}"))
            .with_hint("yard init <another name>"));
    }

    let file = dir.join(project::FILE);
    let existing = read_project_file(&file).transpose()?;
    let (text, written) = match existing {
        Some(text) if !p.force => {
            project::parse(&text, &file)?;
            (text, false)
        }
        existing => {
            let detected = detect(ctx, entry, &dir)?;
            let text = match &existing {
                Some(old) => project::rewrite(old, &detected)?,
                None => project::render(&detected),
            };
            let written = existing.as_deref() != Some(text.as_str());
            if written {
                write_project_file(&file, &text)?;
            }
            (text, written)
        }
    };

    TomlFile::edit(&ctx.paths.sites_toml(), |toml| {
        for key in ["php", "secure", "docroot"] {
            toml.remove(&["sites", &name, key]);
        }
        if entry.is_none() {
            toml.remove(&["sites", &name, "proxy"]);
            toml.set(&["sites", &name, "path"], value(dir.display().to_string()));
        }
        Ok(())
    })?;
    reconcile(ctx).await?;
    Ok(InitResult { file: file.display().to_string(), text, written, site: site(ctx, &name).await? })
}

fn dir_label(dir: &Path) -> Result<String, YardError> {
    let base = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    host_label(&base).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, format!("{base:?} is not a valid site name"))
            .with_hint("yard init <name>")
    })
}

/// What the file should say, from the site's settings and the project as it is now.
fn detect(ctx: &Context, entry: Option<&SiteEntry>, dir: &Path) -> Result<Detected, YardError> {
    // An absolute docroot is compared with the canonical project directory.
    let entry = entry.map(|e| SiteEntry {
        docroot: e
            .docroot
            .clone()
            .map(|d| if d.is_absolute() { std::fs::canonicalize(&d).unwrap_or(d) } else { d }),
        ..e.clone()
    });
    let composer = read_small(&dir.join("composer.json"), COMPOSER_JSON_MAX);
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let installed = ctx.brew.installed_php(&exists, &read_link);
    let secure_by_default = load_yard(&ctx.paths)?.0.secure_by_default;
    project::detect(entry.as_ref(), dir, composer.as_deref(), &installed, secure_by_default, &|p| p.is_dir())
}

/// Writes a fresh temporary file, then renames it over `.yard`. Neither a symlinked
/// `.yard` nor a symlink left at the temporary name in a cloned project is written through.
fn write_project_file(file: &Path, text: &str) -> Result<(), YardError> {
    let failed = |e: std::io::Error| {
        YardError::new(ErrorCode::Internal, format!("cannot write {}: {e}", file.display()))
    };
    if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(YardError::new(ErrorCode::InvalidArgument, format!("{} is a symlink", file.display()))
            .with_hint(format!("rm {} && yard init", file.display())));
    }
    let mut tmp = file.as_os_str().to_os_string();
    tmp.push(".yard-tmp");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(failed)?;
    out.write_all(text.as_bytes()).map_err(failed)?;
    std::fs::rename(&tmp, file).map_err(failed)
}
