//! On-demand services (M4e spec): an instance that opts in falls asleep after `sleep_after`
//! without a client, and wakes when a site needs it. Sleep is runtime state, never written to
//! `services.toml`: the reconcile leaves an asleep instance out of the desired units, so its
//! unit stops with its own graceful signal, and every instance starts awake with yardd.

use crate::conns::client_connections;
use crate::daemon::Context;
use crate::services::default_instance;
use crate::state::Snapshot;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use yard_core::Paths;
use yard_core::project::ServiceWant;
use yard_core::services::instance_name;
use yard_core::services::sleep::{effective_sleep, sleeping_text};
use yard_proto::UnitState;

/// How the idle loop keeps time.
#[derive(Debug, Clone, Copy)]
pub struct SleepConfig {
    /// How often each instance's clients are counted.
    pub sample_interval: Duration,
    /// How long one second of a `sleep_after` lasts: a second in production, a millisecond
    /// in tests, so `"60s"` lasts 60 ms there.
    pub second: Duration,
}

impl SleepConfig {
    /// A sample every 30 s (spec §7).
    pub fn production() -> Self {
        Self { sample_interval: Duration::from_secs(30), second: Duration::from_secs(1) }
    }

    /// How long `secs` of `sleep_after` lasts on this clock.
    fn lasts(&self, secs: u64) -> Duration {
        self.second.saturating_mul(u32::try_from(secs).unwrap_or(u32::MAX))
    }
}

/// Each instance's idle clock and sleep, shared by the reconcile, the idle loop and the wakes.
#[derive(Debug, Default)]
pub struct Sleeper {
    instances: Mutex<BTreeMap<String, Instance>>,
}

#[derive(Debug, Default)]
struct Instance {
    /// When it was last used; `None` until the idle loop first sees it running.
    used: Option<Instant>,
    /// When it fell asleep, while it sleeps.
    asleep: Option<OffsetDateTime>,
    /// When it fell asleep, from its wake until the idle loop sees it running again.
    waking: Option<OffsetDateTime>,
}

impl Sleeper {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Instance>> {
        self.instances.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts the idle clock if it has none.
    pub fn seen(&self, instance: &str) {
        self.lock().entry(instance.to_string()).or_default().used.get_or_insert_with(Instant::now);
    }

    /// The instance was used now.
    pub fn touch(&self, instance: &str) {
        self.lock().entry(instance.to_string()).or_default().used = Some(Instant::now());
    }

    pub fn idle_for(&self, instance: &str, now: Instant) -> Duration {
        self.lock().get(instance).and_then(|i| i.used).map_or(Duration::ZERO, |used| now.duration_since(used))
    }

    pub fn fall_asleep(&self, instance: &str, since: OffsetDateTime) {
        let mut instances = self.lock();
        let entry = instances.entry(instance.to_string()).or_default();
        entry.asleep = Some(since);
        entry.waking = None;
    }

    /// Wakes the instance; true when it was asleep. It is then waking, with the same
    /// `since`, until `clear_waking`, and its idle clock starts again.
    pub fn wake(&self, instance: &str) -> bool {
        let mut instances = self.lock();
        let Some(entry) = instances.get_mut(instance) else { return false };
        let Some(since) = entry.asleep.take() else { return false };
        entry.waking = Some(since);
        entry.used = Some(Instant::now());
        true
    }

    pub fn is_asleep(&self, instance: &str) -> bool {
        self.lock().get(instance).is_some_and(|i| i.asleep.is_some())
    }

    /// Every asleep instance and when it fell asleep.
    pub fn asleep(&self) -> BTreeMap<String, OffsetDateTime> {
        self.lock().iter().filter_map(|(name, i)| Some((name.clone(), i.asleep?))).collect()
    }

    pub fn waking_since(&self, instance: &str) -> Option<OffsetDateTime> {
        self.lock().get(instance).and_then(|i| i.waking)
    }

    pub fn clear_waking(&self, instance: &str) {
        if let Some(entry) = self.lock().get_mut(instance) {
            entry.waking = None;
        }
    }

    /// Drops everything known about the instance: it was stopped or removed.
    pub fn forget(&self, instance: &str) {
        self.lock().remove(instance);
    }
}

/// The instances in `services.toml` that `site`'s `.yard` asks for: by line, or the one
/// `true` means. The inverse of `services::used_by`.
pub fn needed_by(snapshot: &Snapshot, site: &str) -> Vec<String> {
    let Some(project) = snapshot.projects.get(site).and_then(|loaded| loaded.ok()) else { return vec![] };
    project
        .services
        .iter()
        .filter_map(|(&engine, want)| match want {
            ServiceWant::Line(line) => Some(instance_name(engine, Some(line))),
            ServiceWant::Default => default_instance(&snapshot.services, engine).map(str::to_string),
        })
        .filter(|name| snapshot.services.services.contains_key(name))
        .collect()
}

/// `run/sleeping`: the linked sites that need an asleep instance, which the shims read.
pub fn write_sleeping(paths: &Paths, snapshot: &Snapshot, sleeper: &Sleeper) {
    let sites: BTreeSet<String> = snapshot
        .resolution
        .sites
        .iter()
        .filter(|site| needed_by(snapshot, &site.name).iter().any(|i| sleeper.is_asleep(i)))
        .map(|site| site.name.clone())
        .collect();
    let file = paths.sleeping_file();
    if let Err(e) = yard_core::generated::write_atomic(&file, &sleeping_text(&sites)) {
        tracing::warn!(
            "cannot write {}: {e}; php and composer will not wake sleeping services",
            file.display()
        );
    }
}

/// The idle loop: every `sample_interval`, counts the clients of each running instance that
/// may sleep, and puts the ones idle for their `sleep_after` to sleep.
pub fn spawn(ctx: Arc<Context>, cfg: SleepConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.sample_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            sample(&ctx, &cfg).await;
        }
    })
}

async fn sample(ctx: &Context, cfg: &SleepConfig) {
    let Some(snapshot) = ctx.reconciler.snapshot().await else { return };
    let sleeper = &ctx.sleeper;
    // An instance stopped or removed by editing the file wakes with its next start.
    for name in sleeper.asleep().keys() {
        if snapshot.services.services.get(name).is_none_or(|e| e.stopped) {
            sleeper.forget(name);
        }
    }
    let units = ctx.supervisor().list();
    let mut fell = vec![];
    for (name, entry) in &snapshot.services.services {
        if entry.stopped || sleeper.is_asleep(name) {
            continue;
        }
        let (Some(secs), _) = effective_sleep(entry.sleep_after, snapshot.yard.service_sleep_after) else {
            continue;
        };
        let Some(pid) =
            units.iter().find(|u| u.name == *name && u.state == UnitState::Running).and_then(|u| u.pid)
        else {
            continue;
        };
        sleeper.seen(name);
        sleeper.clear_waking(name);
        match tokio::task::spawn_blocking(move || client_connections(pid)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(_)) => sleeper.touch(name),
            // An instance whose clients cannot be counted stays awake.
            Ok(Err(e)) => {
                tracing::warn!("cannot count {name}'s connections: {e}; it stays awake");
                sleeper.touch(name);
            }
            Err(e) => {
                tracing::warn!("cannot count {name}'s connections: {e}; it stays awake");
                sleeper.touch(name);
            }
        }
        if sleeper.idle_for(name, Instant::now()) >= cfg.lasts(secs) {
            sleeper.fall_asleep(name, OffsetDateTime::now_utc());
            fell.push(name.clone());
        }
    }
    if fell.is_empty() {
        return;
    }
    tracing::info!("{} unused for their sleep_after: putting them to sleep", fell.join(", "));
    // The reconcile leaves them out, which stops their units gracefully.
    if let Err(e) = ctx.reconciler.reconcile().await {
        tracing::warn!("putting {} to sleep: {}", fell.join(", "), crate::describe(&e));
    }
    let snapshot = ctx.reconciler.snapshot().await.unwrap_or(snapshot);
    write_sleeping(&ctx.paths, &snapshot, sleeper);
}
