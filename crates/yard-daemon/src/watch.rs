//! FSEvents on the three TOML files and each linked project's `.yard`; any change
//! reconciles. The project directories are read from `sites.toml` at start and again
//! whenever it changes. An edit
//! in `nginx/overrides/` is tested with `nginx -t` and, if it passes, reloaded. Every
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
    let tomls: BTreeSet<PathBuf> = [ctx.paths.yard_toml(), ctx.paths.sites_toml(), ctx.paths.ports_toml()]
        .iter()
        .map(|p| canonical(p))
        .collect();
    let sites_toml = canonical(&ctx.paths.sites_toml());
    let mut projects = project_dirs(&ctx.paths);
    let mut watch = vec![canonical(ctx.paths.root()), overrides.clone()];
    watch.extend(projects.iter().cloned());
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
            let relevant = changed.iter().any(|p| tomls.contains(p) || is_project_file(p, &projects));
            let overridden = events.iter().any(|e| canonical(&e.path).parent() == Some(overrides.as_path()));
            if relevant {
                match ctx.reconciler.reconcile().await {
                    Ok(report) => report.warnings.iter().for_each(|w| tracing::warn!("{w}")),
                    Err(e) => tracing::warn!("reconcile after a change: {}", describe(&e)),
                }
            }
            if overridden && let Err(e) = ctx.reconciler.apply_overrides().await {
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
