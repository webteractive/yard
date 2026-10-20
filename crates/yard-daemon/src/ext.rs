//! PHP extensions (spec §4.3): installing one from brew, pecl or a `.so`, recording it in
//! `yard.toml`, and proving it loads with `php -m`, rolling back when it does not; and
//! removing one.

use crate::jobs::JobCtx;
use crate::reconcile::Reconciler;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use toml_edit::{InlineTable, value};
use yard_core::brew::Brew;
use yard_core::config::TomlFile;
use yard_core::config::yard::{ExtEntry, ExtSource};
use yard_core::shim::child_path;
use yard_core::{Paths, PhpVersion};
use yard_proto::{ErrorCode, YardError};

/// The modules `php -m` lists under `[PHP Modules]` and `[Zend Modules]`, lowercased.
/// Startup warnings printed before the lists are not modules.
pub fn loaded_modules(output: &str) -> BTreeSet<String> {
    let mut listing = false;
    let mut modules = BTreeSet::new();
    for line in output.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            listing = line == "[PHP Modules]" || line == "[Zend Modules]";
        } else if listing && !line.is_empty() {
            modules.insert(line.to_lowercase());
        }
    }
    modules
}

/// One `ext.add`, checked by the handler: `module` is valid and not yet managed for `v`,
/// and `so` is an existing `.so` when the extension comes from a file.
pub struct Add {
    pub paths: Paths,
    pub brew: Brew,
    pub reconciler: Arc<Reconciler>,
    pub v: PhpVersion,
    /// The version's keg, whose `pecl` and `php` are used.
    pub opt: PathBuf,
    pub module: String,
    pub so: Option<PathBuf>,
    /// `<module>-<version>`: a pinned pecl release, installed with pecl even when brew
    /// has a formula.
    pub release: Option<String>,
}

impl Add {
    /// The job body: install, record, reconcile, verify.
    pub async fn run(self, job: JobCtx) -> Result<(), YardError> {
        let (v, module) = (self.v, self.module.as_str());
        let keg_path = keg_path(&self.opt);
        let keg_env = [("PATH".to_string(), keg_path.clone())];

        let mut entry = InlineTable::new();
        let mut copied = None;
        match &self.so {
            Some(src) => {
                let dst = self.copy(src)?;
                job.line(format!("copied {} to {}", src.display(), dst.display()));
                entry.insert("source", "path".into());
                entry.insert("so", dst.display().to_string().into());
                copied = Some(dst);
            }
            None => {
                let brew = self.brew.brew_bin();
                let formula = format!("shivammathur/extensions/{module}@{v}");
                let found = match &self.release {
                    Some(_) => false,
                    None => job.capture(&brew, &["info", "--formula", &formula], &Brew::job_env()).await?.0,
                };
                if found {
                    job.run(&brew, &["install", &formula], &Brew::job_env()).await?;
                    entry.insert("source", "brew".into());
                } else {
                    let package = self.release.as_deref().unwrap_or(module);
                    if self.release.is_none() {
                        job.line(format!("brew has no {formula}; trying pecl"));
                    }
                    // `php_ini=` keeps pecl from adding `extension=` to brew's php.ini:
                    // Yard writes the ini that loads it.
                    job.run(
                        &Brew::php_bin(&self.opt, "pecl"),
                        &["-d", "php_ini=", "install", package],
                        &keg_env,
                    )
                    .await?;
                    entry.insert("source", "pecl".into());
                }
            }
        }

        let yard_toml = self.paths.yard_toml();
        let existed = yard_toml.exists();
        let keys = ["php", &v.to_string(), "extensions", module];
        TomlFile::edit(&yard_toml, |file| {
            file.set(&keys, value(entry.clone()));
            Ok(())
        })?;
        // Reconcile writes 50-<module>.ini and restarts the version's pool if it runs.
        let reconciled = self.reconciler.reconcile().await;

        let php_dir = self.paths.php_dir(v);
        let env = [
            ("PHPRC".to_string(), php_dir.display().to_string()),
            ("PHP_INI_SCAN_DIR".to_string(), self.paths.php_conf_d(v).display().to_string()),
            ("PATH".to_string(), keg_path),
        ];
        let output = match &reconciled {
            Ok(_) => job.capture(&Brew::php_bin(&self.opt, "php"), &["-m"], &env).await?.1.join("\n"),
            Err(e) => e.message.clone(),
        };
        if reconciled.is_ok() && loaded_modules(&output).contains(&module.to_lowercase()) {
            job.line(format!("{module} is loaded in PHP {v}"));
            return Ok(());
        }

        // Roll back. A file Yard copied goes, with its entry. Something brew or pecl
        // installed stays installed but disabled: with no entry, brew's own ini would
        // load it again.
        // Only this entry is undone: other edits made meanwhile stay.
        let installed = copied.is_none();
        let emptied = TomlFile::edit(&yard_toml, |file| {
            if installed {
                entry.insert("enabled", false.into());
                file.set(&keys, value(entry));
            } else {
                file.remove(&keys);
            }
            Ok(file.doc().is_empty())
        })?;
        if emptied && !existed {
            remove_created(&yard_toml)?;
        }
        if let Some(copy) = &copied {
            let _ = std::fs::remove_file(copy);
        }
        if let Err(e) = self.reconciler.reconcile().await {
            job.line(format!("reconcile after the rollback failed: {}", e.message));
        }
        let error = YardError::new(ErrorCode::ExtVerifyFailed, format!("{module} did not load in PHP {v}"))
            .with_data(serde_json::json!({ "output": output }));
        Err(if installed { error.with_hint(format!("yard ext remove {module} --php {v}")) } else { error })
    }

    /// Copies a `.so` into `php/<v>/ext/`, so the user's file can move or change.
    fn copy(&self, src: &Path) -> Result<PathBuf, YardError> {
        let dir = self.paths.php_dir(self.v).join("ext");
        let dst = dir.join(src.file_name().unwrap_or_default());
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(src, &dst)).map_err(|e| {
            YardError::internal(format!("cannot copy {} to {}: {e}", src.display(), dst.display()))
        })?;
        Ok(dst)
    }
}

/// One `ext.remove` of an extension `yard.toml` manages (not `bundled`): uninstall or
/// delete it, then forget it.
pub struct Remove {
    pub paths: Paths,
    pub brew: Brew,
    pub reconciler: Arc<Reconciler>,
    pub v: PhpVersion,
    pub opt: PathBuf,
    pub module: String,
    pub entry: ExtEntry,
}

impl Remove {
    pub async fn run(self, job: JobCtx) -> Result<(), YardError> {
        let (v, module) = (self.v, self.module.as_str());
        match self.entry.source {
            ExtSource::Brew => {
                let formula = format!("shivammathur/extensions/{module}@{v}");
                job.run(&self.brew.brew_bin(), &["uninstall", &formula], &Brew::job_env()).await?;
            }
            ExtSource::Pecl => {
                let env = [("PATH".to_string(), keg_path(&self.opt))];
                job.run(&Brew::php_bin(&self.opt, "pecl"), &["-d", "php_ini=", "uninstall", module], &env)
                    .await?;
            }
            ExtSource::Path | ExtSource::Bundled => {
                if let Some(so) = &self.entry.so {
                    match std::fs::remove_file(so) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                            return Err(YardError::internal(format!("cannot delete {}: {e}", so.display())));
                        }
                        _ => job.line(format!("deleted {}", so.display())),
                    }
                }
            }
        }
        TomlFile::edit(&self.paths.yard_toml(), |file| {
            Ok(file.remove(&["php", &v.to_string(), "extensions", module]))
        })?;
        self.reconciler.reconcile().await.map(|_| ())
    }
}

/// PATH for a version's tools: pecl runs phpize and php-config through it, so the
/// keg's `bin` comes first and Herd's directories are gone.
fn keg_path(opt: &Path) -> String {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    child_path(Some(&opt.join("bin")), &inherited).to_string_lossy().into_owned()
}

/// Deletes the `yard.toml` an add created, once the rollback has emptied it.
fn remove_created(file: &Path) -> Result<(), YardError> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(YardError::internal(format!("cannot delete {}: {e}", file.display())))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_modules_parses_php_m() {
        let out = "PHP Warning:  Unable to load dynamic library 'x'\n[PHP Modules]\nCore\nredis\n\n[Zend Modules]\nXdebug\n";
        let expected: BTreeSet<String> = ["core", "redis", "xdebug"].map(String::from).into();
        assert_eq!(loaded_modules(out), expected);
    }
}
