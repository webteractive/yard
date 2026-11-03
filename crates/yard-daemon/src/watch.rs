//! FSEvents on the four TOML files and each linked project's `.yard`; any change
//! reconciles. The project directories are read from `sites.toml` at start and again
//! whenever it changes. An edit
//! in `nginx/overrides/` or `nginx/tunnel-overrides/`, or of an instance's `overrides.conf` that nginx includes (MinIO's,
//! in `minio.<tld>`'s block), is tested with `nginx -t` and, if it passes, reloaded. Those
//! instance directories are read from `services.toml` the same way. The other engines'
//! `overrides.conf` are not watched: a restart of the instance applies them. Every
//! `rescan`, sites whose docroot appeared or vanished since are reconciled too.
//!
//! Only those paths count. Yard's own writes under `nginx/`, `php/` and `state/` must not
//! trigger a reconcile, or every reconcile would schedule the next one.

use crate::daemon::Context;
use crate::describe;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Returns once every path is watched, so yardd serves no request before an edit can be
/// seen: FSEvents reports nothing from before its stream started.
pub async fn spawn(ctx: Arc<Context>, debounce: Duration, rescan: Duration) -> JoinHandle<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<DebounceEventResult>();
    let debouncer: Watcher = match new_debouncer(debounce, move |result| {
        let _ = tx.send(result);
    }) {
        Ok(d) => Arc::new(Mutex::new(d)),
        Err(e) => {
            tracing::error!("cannot watch for changes: {e}; edits need `yard restart`");
            return tokio::spawn(async {});
        }
    };
    let overrides = canonical(&ctx.paths.nginx_dir().join("overrides"));
    let tunnel_overrides = canonical(&ctx.paths.tunnel_overrides_dir());
    let tomls: BTreeSet<PathBuf> =
        [ctx.paths.yard_toml(), ctx.paths.sites_toml(), ctx.paths.ports_toml(), ctx.paths.services_toml()]
            .iter()
            .map(|p| canonical(p))
            .collect();
    let sites_toml = canonical(&ctx.paths.sites_toml());
    let services_toml = canonical(&ctx.paths.services_toml());
    let mut projects = project_dirs(&ctx.paths);
    let managed_root = canonical(&ctx.paths.managed_sites_dir());
    let mut managed = managed_dirs(&ctx.paths);
    let mut includes = nginx_include_dirs(&ctx.paths);
    let mut watch = vec![canonical(ctx.paths.root()), overrides.clone(), tunnel_overrides.clone()];
    watch.extend(projects.iter().cloned());
    watch.extend(managed.iter().cloned());
    watch.extend(includes.iter().cloned());
    change_watches(&debouncer, watch, vec![]).await;

    tokio::spawn(async move {
        // Dropping the debouncer stops FSEvents, so the task owns it for as long as it runs.
        let mut tick = tokio::time::interval(rescan);
        loop {
            let result = tokio::select! {
                result = rx.recv() => match result {
                    Some(result) => result,
                    None => return,
                },
                _ = tick.tick() => {
                    if (ctx.reconciler.docroots_stale().await || ctx.reconciler.certs_due().await)
                        && let Err(e) = ctx.reconciler.reconcile().await
                    {
                        tracing::warn!("reconcile after a docroot or certificate change: {}", describe(&e));
                    }
                    continue;
                }
            };
            let events = match result {
                Ok(events) => events,
                Err(e) => {
                    tracing::warn!("watch error: {e}");
                    continue;
                }
            };
            let changed: Vec<PathBuf> = events.iter().map(|e| canonical(&e.path)).collect();
            if changed.contains(&sites_toml) {
                let now = project_dirs(&ctx.paths);
                let add = now.difference(&projects).cloned().collect();
                let remove = projects.difference(&now).cloned().collect();
                change_watches(&debouncer, add, remove).await;
                projects = now;
            }
            // Before relevance, so a `sites/<name>/` made in this batch is watched; a file
            // already written into a directory only now watched, or one removed with its
            // directory, counts as a change.
            let mut managed_moved = false;
            if changed.contains(&sites_toml) || changed.iter().any(|p| p.starts_with(&managed_root)) {
                let now = managed_dirs(&ctx.paths);
                let add: Vec<PathBuf> = now.difference(&managed).cloned().collect();
                let remove = managed.difference(&now).cloned().collect();
                let remove: Vec<PathBuf> = remove;
                managed_moved = !add.is_empty() || !remove.is_empty();
                change_watches(&debouncer, add, remove).await;
                managed = now;
            }
            let relevant = managed_moved
                || changed.iter().any(|p| {
                    tomls.contains(p) || is_project_file(p, &projects) || is_project_file(p, &managed)
                });
            let overridden = changed.iter().any(|p| {
                p.parent() == Some(overrides.as_path())
                    || p.parent() == Some(tunnel_overrides.as_path())
                    || is_nginx_include(p, &includes)
            });
            if relevant {
                match ctx.reconciler.reconcile().await {
                    Ok(report) => report.warnings.iter().for_each(|w| tracing::warn!("{w}")),
                    Err(e) => tracing::warn!("reconcile after a change: {}", describe(&e)),
                }
            }
            // After the reconcile, which creates a new instance's directory and file. An edit
            // made before the new watch started is not reported, so test nginx once after it.
            let mut added = false;
            if changed.contains(&services_toml) {
                let now = nginx_include_dirs(&ctx.paths);
                let add: Vec<PathBuf> = now.difference(&includes).cloned().collect();
                let remove = includes.difference(&now).cloned().collect();
                added = !add.is_empty();
                change_watches(&debouncer, add, remove).await;
                includes = now;
            }
            if (overridden || added)
                && let Err(e) = ctx.reconciler.apply_overrides().await
            {
                tracing::warn!("applying an override edit: {}", describe(&e));
            }
        }
    })
}

type Watcher = Arc<Mutex<Debouncer<RecommendedWatcher>>>;

/// The directories of the linked sites in `sites.toml`, canonical; none while it does
/// not parse (the reconcile reports that).
fn project_dirs(paths: &yard_core::Paths) -> BTreeSet<PathBuf> {
    let Ok(sites) = yard_core::config::load_sites(paths) else { return BTreeSet::new() };
    sites.sites.values().filter_map(|e| e.path.as_deref()).filter(|p| p.is_dir()).map(canonical).collect()
}

/// The managed-file directory, always, and each `sites/<name>/` in it that exists for a
/// name in `sites.toml`, canonical (managed-file spec §4). yardd makes `sites/` at
/// startup and never stops watching it: FSEvents reported nothing more for a `sites/`
/// that was unwatched when removed and watched again once made anew.
fn managed_dirs(paths: &yard_core::Paths) -> BTreeSet<PathBuf> {
    let root = paths.managed_sites_dir();
    let names: Vec<String> =
        yard_core::config::load_sites(paths).map(|s| s.sites.into_keys().collect()).unwrap_or_default();
    let names = names.iter().filter(|name| yard_core::sites::is_site_name(name));
    let sites = names.map(|name| root.join(name)).filter(|dir| dir.is_dir());
    std::iter::once(root.clone()).chain(sites).map(|dir| canonical(&dir)).collect()
}

/// The directories of the instances whose `overrides.conf` nginx includes: MinIO's, which
/// `minio.<tld>`'s block ends with. Canonical, and only those that exist; none while
/// `services.toml` does not parse.
fn nginx_include_dirs(paths: &yard_core::Paths) -> BTreeSet<PathBuf> {
    let Ok(services) = yard_core::config::load_services(paths) else { return BTreeSet::new() };
    services
        .services
        .iter()
        .filter(|(_, entry)| entry.engine == yard_core::services::Engine::Minio)
        .map(|(instance, _)| paths.service_dir(instance))
        .filter(|dir| dir.is_dir())
        .map(|dir| canonical(&dir))
        .collect()
}

/// An instance's `overrides.conf` in one of `dirs` (`nginx_include_dirs`).
fn is_nginx_include(path: &Path, dirs: &BTreeSet<PathBuf>) -> bool {
    path.file_name().is_some_and(|n| n == "overrides.conf")
        && path.parent().is_some_and(|dir| dirs.contains(dir))
}

fn is_project_file(path: &Path, projects: &BTreeSet<PathBuf>) -> bool {
    path.file_name().is_some_and(|n| n == yard_core::project::FILE)
        && path.parent().is_some_and(|dir| projects.contains(dir))
}

/// Adds and removes the watches as one batch: FSEvents restarts its stream for each change, and each
/// restart waits on fseventsd, which can take seconds when the system is busy. The wait
/// runs on the blocking pool.
async fn change_watches(debouncer: &Watcher, watch: Vec<PathBuf>, unwatch: Vec<PathBuf>) {
    if watch.is_empty() && unwatch.is_empty() {
        return;
    }
    let debouncer = debouncer.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let mut debouncer = debouncer.lock().unwrap_or_else(PoisonError::into_inner);
        let mut paths = debouncer.watcher().paths_mut();
        for old in &unwatch {
            if let Err(e) = paths.remove(old) {
                tracing::warn!("cannot stop watching {}: {e}", old.display());
            }
        }
        for new in &watch {
            if let Err(e) = paths.add(new, RecursiveMode::NonRecursive) {
                tracing::warn!("cannot watch {}: {e}", new.display());
            }
        }
        if let Err(e) = paths.commit() {
            tracing::warn!("cannot start watching: {e}");
        }
    })
    .await;
}

/// FSEvents reports resolved paths (`/private/var/…`), so compare canonical forms. A
/// deleted file cannot be canonicalized; its parent still can.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    match (path.parent().and_then(|d| std::fs::canonicalize(d).ok()), path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_dirs_skip_a_name_that_is_not_a_site_name() {
        let root = tempfile::tempdir().unwrap();
        let paths = yard_core::Paths::new(root.path());
        std::fs::create_dir_all(root.path().join("escape")).unwrap();
        std::fs::create_dir_all(paths.managed_sites_dir().join("ee")).unwrap();
        let sites = "[sites.\"../escape\"]\npath = \"/code/x\"\n[sites.ee]\npath = \"/code/ee\"\n";
        std::fs::write(paths.sites_toml(), sites).unwrap();
        let dirs = managed_dirs(&paths);
        let sites_dir = canonical(&paths.managed_sites_dir());
        assert_eq!(dirs, BTreeSet::from([sites_dir.clone(), sites_dir.join("ee")]));
    }
}
