//! FSEvents on the three TOML files; any change reconciles. An edit
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
    change_watches(&debouncer, vec![canonical(ctx.paths.root()), overrides.clone()]).await;

    tokio::spawn(async move {
        // Dropping the debouncer stops FSEvents, so the task owns it for as long as it runs.
        let _debouncer = debouncer;
        let mut tick = tokio::time::interval(rescan);
        loop {
            let result = tokio::select! {
                result = rx.recv() => match result {
                    Some(result) => result,
                    None => return,
                },
                _ = tick.tick() => {
                    if ctx.reconciler.docroots_stale().await
                        && let Err(e) = ctx.reconciler.reconcile().await
                    {
                        tracing::warn!("reconcile after a docroot change: {}", describe(&e));
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
            let relevant = events.iter().any(|e| tomls.contains(&canonical(&e.path)));
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

/// Adds the watches as one batch: FSEvents restarts its stream for each change, and each
/// restart waits on fseventsd, which can take seconds when the system is busy. The wait
/// runs on the blocking pool.
async fn change_watches(debouncer: &Watcher, watch: Vec<PathBuf>) {
    let debouncer = debouncer.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let mut debouncer = debouncer.lock().unwrap_or_else(PoisonError::into_inner);
        let mut paths = debouncer.watcher().paths_mut();
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
