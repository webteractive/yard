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
use yard_core::render::php::pecl_user_config;
use yard_core::shim::child_path;
use yard_core::{Paths, PhpVersion, generated};
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
        let keg_path = keg_path(&self.brew, &self.opt);
        // pecl builds against PHP's headers, which include brew's (pcre2.h).
        let prefix = self.brew.prefix.display();
        let keg_env = [("PATH".to_string(), keg_path.clone())];
        let build_env = [
            ("PATH".to_string(), keg_path.clone()),
            ("CPPFLAGS".to_string(), format!("-I{prefix}/include")),
            ("LDFLAGS".to_string(), format!("-L{prefix}/lib")),
        ];

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
                let formula = crate::sources::ext_formula(module, v);
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
                    let pecl = Brew::php_bin(&self.opt, "pecl");
                    let conf = pecl_conf(&self.paths, v)?;
                    job.run(&pecl, &["-c", &conf, "install", package], &build_env).await?;
                    entry.insert("source", "pecl".into());
                    // pecl installs into its own ext_dir, not PHP's extension_dir, so the
                    // ini Yard writes loads it by absolute path.
                    let (_, out) =
                        job.capture(&pecl, &["-c", &conf, "config-get", "ext_dir"], &keg_env).await?;
                    let so = out
                        .iter()
                        .rev()
                        .find(|l| l.starts_with('/'))
                        .map(|dir| Path::new(dir.trim()).join(format!("{module}.so")));
                    match so {
                        Some(so) if so.is_file() => {
                            entry.insert("so", so.display().to_string().into());
                        }
                        _ => {
                            job.line(format!("pecl did not say where it put {module}.so; loading it by name"))
                        }
                    }
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
        // A cancel during the check rolls back as a failed check does, then reports itself.
        let (output, interrupted) = match &reconciled {
            Ok(_) => match job.capture(&Brew::php_bin(&self.opt, "php"), &["-m"], &env).await {
                Ok((_, lines)) => (lines.join("\n"), None),
                Err(e) => (e.message.clone(), Some(e)),
            },
            Err(e) => (e.message.clone(), None),
        };
        if interrupted.is_none()
            && reconciled.is_ok()
            && loaded_modules(&output).contains(&module.to_lowercase())
        {
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
        if let Some(e) = interrupted {
            return Err(e);
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
                let env = [("PATH".to_string(), keg_path(&self.brew, &self.opt))];
                job.run(
                    &Brew::php_bin(&self.opt, "pecl"),
                    &["-c", &pecl_conf(&self.paths, v)?, "uninstall", module],
                    &env,
                )
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
fn keg_path(brew: &Brew, opt: &Path) -> String {
    // Then brew's own bin: phpize runs autoconf, and launchd gives yardd no /opt/homebrew/bin.
    let brew_bin = brew.prefix.join("bin");
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let rest = std::env::split_paths(&inherited).filter(|p| *p != brew_bin);
    let with_brew = std::env::join_paths(std::iter::once(brew_bin.clone()).chain(rest)).unwrap_or_default();
    child_path(Some(&opt.join("bin")), &with_brew).to_string_lossy().into_owned()
}

/// Writes `php/<v>/pecl.conf`, the PEAR user config pecl runs with (`-c`), and returns
/// its path. It points `php_ini` at `php/<v>/pecl-php.ini`, which is never created, so pecl
/// only notes that it cannot enable the extension instead of editing brew's php.ini;
/// Yard writes the ini that loads it (`render::php::pecl_user_config` says why `-d`/`-D`
/// cannot do this).
fn pecl_conf(paths: &Paths, v: PhpVersion) -> Result<String, YardError> {
    let conf = paths.php_dir(v).join("pecl.conf");
    let text = pecl_user_config(&paths.php_dir(v).join("pecl-php.ini"));
    generated::write_atomic(&conf, &text)
        .map_err(|e| YardError::internal(format!("cannot write {}: {e}", conf.display())))?;
    Ok(conf.display().to_string())
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
