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
use yard_core::config::ServicesConfig;
use yard_core::project::{Projects, ServiceWant};
use yard_core::services::instance_name;
use yard_core::services::sleep::{effective_sleep, sleeping_text};
use yard_proto::{SiteInfo, UnitState};

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

    /// Puts the instance to sleep if it has been unused for `after` at `now`; true when it fell
    /// asleep. One lock covers the check and the change, so a use in between keeps it awake. A
    /// use after `now` counts as no idle time at all.
    pub fn fall_asleep_if_idle(
        &self,
        instance: &str,
        now: Instant,
        after: Duration,
        since: OffsetDateTime,
    ) -> bool {
        let mut instances = self.lock();
        let entry = instances.entry(instance.to_string()).or_default();
        let idle = entry.used.map_or(Duration::ZERO, |used| now.saturating_duration_since(used));
        if idle < after {
            return false;
        }
        entry.asleep = Some(since);
        entry.waking = None;
        true
    }

    /// Undoes a sleep whose unit was never stopped: it is awake, not waking, and its idle
    /// clock starts again.
    pub fn stay_awake(&self, instance: &str) {
        let mut instances = self.lock();
        let entry = instances.entry(instance.to_string()).or_default();
        entry.asleep = None;
        entry.waking = None;
        entry.used = Some(Instant::now());
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
    instances_for(&snapshot.services, &snapshot.projects, site)
}

/// [`needed_by`] from the parts a reconcile has before it makes the snapshot.
pub fn instances_for(services: &ServicesConfig, projects: &Projects, site: &str) -> Vec<String> {
    let Some(project) = projects.get(site).and_then(|loaded| loaded.ok()) else { return vec![] };
    project
        .services
        .iter()
        .filter_map(|(&engine, want)| match want {
            ServiceWant::Line(line) => Some(instance_name(engine, Some(line))),
            ServiceWant::Default => default_instance(services, engine).map(str::to_string),
        })
        .filter(|name| services.services.contains_key(name))
        .collect()
}

/// `run/sleeping`: the linked sites that need an asleep instance, which the shims read. The
/// reconcile writes it before it stops or starts any unit, under its lock, so it is the file's
/// only writer once yardd runs, and a shim run while an instance stops asks for a wake, which
/// waits for the stop. An unchanged list is not rewritten.
pub fn write_sleeping(
    paths: &Paths,
    sites: &[SiteInfo],
    services: &ServicesConfig,
    projects: &Projects,
    sleeper: &Sleeper,
) {
    let sites: BTreeSet<String> = sites
        .iter()
        .filter(|site| instances_for(services, projects, &site.name).iter().any(|i| sleeper.is_asleep(i)))
        .map(|site| site.name.clone())
        .collect();
    let file = paths.sleeping_file();
    let text = sleeping_text(&sites);
    if std::fs::read_to_string(&file).is_ok_and(|old| old == text) {
        return;
    }
    if let Err(e) = yard_core::generated::write_atomic(&file, &text) {
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
    let units = ctx.supervisor().list();
    let mut fell = vec![];
    for (name, entry) in &snapshot.services.services {
        if entry.stopped || sleeper.is_asleep(name) {
            continue;
        }
        let Some(pid) =
            units.iter().find(|u| u.name == *name && u.state == UnitState::Running).and_then(|u| u.pid)
        else {
            continue;
        };
        // Running, so it is no longer waking, whether or not it may still sleep.
        sleeper.clear_waking(name);
        let (Some(secs), _) = effective_sleep(entry.sleep_after, snapshot.yard.service_sleep_after) else {
            continue;
        };
        sleeper.seen(name);
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
        if sleeper.fall_asleep_if_idle(name, Instant::now(), cfg.lasts(secs), OffsetDateTime::now_utc()) {
            fell.push(name.clone());
        }
    }
    if fell.is_empty() {
        return;
    }
    tracing::info!("{} unused for their sleep_after: putting them to sleep", fell.join(", "));
    // The reconcile lists their sites in `run/sleeping`, then leaves them out, which stops
    // their units gracefully.
    if let Err(e) = ctx.reconciler.reconcile().await {
        tracing::warn!("putting {} to sleep: {}", fell.join(", "), crate::describe(&e));
        // A reconcile that failed before it stopped a unit leaves that instance running: it is
        // awake, and says so, and its idle clock starts again.
        let units = ctx.supervisor().list();
        for name in fell.iter().filter(|name| units.iter().any(|u| u.name == **name)) {
            sleeper.stay_awake(name);
        }
    }
}

/// How often a wake checks whether the instances it woke run yet.
const WAKE_POLL: Duration = Duration::from_millis(20);

/// Counts as use of each of `instances` and wakes the asleep ones, then waits up to `wait`
/// until each one woken or still waking runs, has failed, or has no unit coming. An instance
/// the user stopped is never woken. It returns either way, within `wait` even while the
/// reconcile that starts the units queues behind another (a stop in progress holds it): a
/// wake that fails is the unit's own `failed` state (spec §6.1).
pub async fn wake(ctx: &Context, instances: &[String], wait: Duration) {
    let deadline = Instant::now() + wait;
    let Some(snapshot) = ctx.reconciler.snapshot().await else { return };
    let sleeper = &ctx.sleeper;
    let instances: Vec<&String> =
        instances.iter().filter(|i| snapshot.services.services.get(*i).is_some_and(|e| !e.stopped)).collect();
    let mut woke = false;
    for instance in &instances {
        sleeper.touch(instance);
        woke |= sleeper.wake(instance);
    }
    if woke {
        tracing::info!("waking {}", instances.iter().map(|i| i.as_str()).collect::<Vec<_>>().join(", "));
        // Spawned, so giving up at the deadline never cancels a reconcile halfway through.
        let reconciler = ctx.reconciler.clone();
        let mut run = tokio::spawn(async move { reconciler.reconcile().await });
        tokio::select! {
            done = &mut run => {
                if let Ok(Err(e)) = done {
                    tracing::warn!("waking: {}", crate::describe(&e));
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => return,
        }
    }
    let waiting: Vec<&String> = instances.into_iter().filter(|i| sleeper.waking_since(i).is_some()).collect();
    loop {
        let snapshot = ctx.reconciler.snapshot().await;
        let units = ctx.supervisor().list();
        let pending = waiting.iter().filter(|i| {
            let left_out = snapshot.as_ref().is_some_and(|s| s.left_out.contains_key(i.as_str()));
            match units.iter().find(|u| u.name == ***i).map(|u| &u.state) {
                Some(UnitState::Running) => {
                    sleeper.clear_waking(i);
                    false
                }
                Some(UnitState::Failed { .. }) => false,
                Some(_) => true,
                // Another wake's reconcile may not have spawned it yet.
                None => !left_out && sleeper.waking_since(i).is_some(),
            }
        });
        if pending.count() == 0 || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(WAKE_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_falls_asleep_only_after_the_whole_sleep_after() {
        let sleeper = Sleeper::default();
        let since = OffsetDateTime::UNIX_EPOCH;
        sleeper.touch("redis");
        let after = Duration::from_secs(60);
        assert!(!sleeper.fall_asleep_if_idle(
            "redis",
            Instant::now() + Duration::from_secs(30),
            after,
            since
        ));
        assert!(!sleeper.is_asleep("redis"));
        assert!(sleeper.fall_asleep_if_idle("redis", Instant::now() + Duration::from_secs(61), after, since));
        assert_eq!(sleeper.asleep().get("redis"), Some(&since));
    }

    #[test]
    fn a_touch_after_the_check_began_keeps_it_awake() {
        let sleeper = Sleeper::default();
        let since = OffsetDateTime::UNIX_EPOCH;
        let now = Instant::now();
        sleeper.touch("redis");
        // The check began before the touch, so the instance was not idle at all.
        assert!(!sleeper.fall_asleep_if_idle("redis", now, Duration::from_nanos(1), since));
    }

    #[test]
    fn stay_awake_undoes_a_sleep() {
        let sleeper = Sleeper::default();
        let since = OffsetDateTime::UNIX_EPOCH;
        sleeper.touch("redis");
        assert!(sleeper.fall_asleep_if_idle(
            "redis",
            Instant::now() + Duration::from_secs(61),
            Duration::from_secs(60),
            since
        ));
        sleeper.stay_awake("redis");
        assert!(!sleeper.is_asleep("redis"));
        assert_eq!(sleeper.waking_since("redis"), None, "it never stopped, so it is not waking");
        assert!(!sleeper.fall_asleep_if_idle(
            "redis",
            Instant::now() + Duration::from_secs(30),
            Duration::from_secs(60),
            since
        ));
    }
}
