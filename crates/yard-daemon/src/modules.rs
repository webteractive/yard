//! The modules a PHP keg has compiled in, from `php -n -m`, cached in
//! `state/php-modules.json` by the keg's canonical path, so it runs once per keg version
//! and again only after a `brew upgrade` moves the keg (project-file spec §4).

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::shim::child_path;

type Cache = BTreeMap<String, BTreeSet<String>>;

/// Lowercased module names; empty when the keg's `php` cannot run, which is not cached.
pub fn compiled(paths: &Paths, opt: &Path) -> BTreeSet<String> {
    let Ok(keg) = std::fs::canonicalize(opt) else { return BTreeSet::new() };
    let key = keg.display().to_string();
    let file = paths.php_modules_cache();
    let mut cache: Cache =
        std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Some(modules) = cache.get(&key) {
        return modules.clone();
    }
    let Some(modules) = run(opt) else { return BTreeSet::new() };
    cache.insert(key, modules.clone());
    if let Err(e) = save(&file, &cache) {
        tracing::warn!("cannot save {}: {e}", file.display());
    }
    modules
}

fn run(opt: &Path) -> Option<BTreeSet<String>> {
    let mut cmd = Command::new(Brew::php_bin(opt, "php"));
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    cmd.args(["-n", "-m"])
        .env("PATH", child_path(Some(&opt.join("bin")), &inherited))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let out = cmd.output().ok()?;
    out.status.success().then(|| crate::ext::loaded_modules(&String::from_utf8_lossy(&out.stdout)))
}

fn save(file: &Path, cache: &Cache) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.yard-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cache).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, file)
}
