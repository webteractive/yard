//! FSEvents on the parked paths and the three TOML files; any change reconciles. An edit
//! in `nginx/overrides/` is tested with `nginx -t` and, if it passes, reloaded. Every
//! `rescan`, sites whose docroot appeared or vanished since are reconciled too.
//!
//! Only those paths count. Yard's own writes under `nginx/`, `php/` and `state/` must not
//! trigger a reconcile, or every reconcile would schedule the next one.

use crate::daemon::Context;
use crate::describe;
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub fn spawn(ctx: Arc<Context>, debounce: Duration, rescan: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<DebounceEventResult>();
        let mut debouncer = match new_debouncer(debounce, move |result| {
            let _ = tx.send(result);
        }) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("cannot watch for changes: {e}; edits need `yard restart`");
                return;
            }
        };
        let root = canonical(ctx.paths.root());
        if let Err(e) = debouncer.watcher().watch(&root, RecursiveMode::NonRecursive) {
            tracing::warn!("cannot watch {}: {e}", root.display());
        }
        let overrides = canonical(&ctx.paths.nginx_dir().join("overrides"));
        if let Err(e) = debouncer.watcher().watch(&overrides, RecursiveMode::NonRecursive) {
            tracing::warn!("cannot watch {}: {e}", overrides.display());
        }
        let tomls: BTreeSet<PathBuf> =
            [ctx.paths.yard_toml(), ctx.paths.sites_toml(), ctx.paths.ports_toml()]
                .iter()
                .map(|p| canonical(p))
                .collect();
        let mut parked = BTreeSet::new();
        rewatch(&ctx, &mut debouncer, &mut parked).await;

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
            let relevant = events.iter().any(|e| {
                let path = canonical(&e.path);
                tomls.contains(&path)
                    || parked.contains(&path)
                    || path.parent().is_some_and(|p| parked.contains(p))
            });
            let overridden = events.iter().any(|e| canonical(&e.path).parent() == Some(overrides.as_path()));
            if relevant {
                match ctx.reconciler.reconcile().await {
                    Ok(report) => report.warnings.iter().for_each(|w| tracing::warn!("{w}")),
                    Err(e) => tracing::warn!("reconcile after a change: {}", describe(&e)),
                }
                rewatch(&ctx, &mut debouncer, &mut parked).await;
            }
            if overridden && let Err(e) = ctx.reconciler.apply_overrides().await {
                tracing::warn!("applying an override edit: {}", describe(&e));
            }
        }
    })
}

/// Watches the parked paths of the current snapshot, dropping ones no longer parked.
async fn rewatch(
    ctx: &Context,
    debouncer: &mut notify_debouncer_mini::Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>,
    watched: &mut BTreeSet<PathBuf>,
) {
    let Some(snapshot) = ctx.reconciler.snapshot().await else { return };
    let wanted: BTreeSet<PathBuf> = snapshot.yard.parked.iter().map(|p| canonical(p)).collect();
    for gone in watched.difference(&wanted) {
        let _ = debouncer.watcher().unwatch(gone);
    }
    for new in wanted.difference(watched) {
        if let Err(e) = debouncer.watcher().watch(new, RecursiveMode::NonRecursive) {
            tracing::warn!("cannot watch parked path {}: {e}", new.display());
        }
    }
    *watched = wanted;
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
