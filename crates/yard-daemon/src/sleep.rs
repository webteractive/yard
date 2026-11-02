//! On-demand services (M4e spec): an instance that opts in falls asleep after `sleep_after`
//! without a client, and wakes when a site needs it. Sleep is runtime state, never written to
//! `services.toml`: the reconcile leaves an asleep instance out of the desired units, so its
//! unit stops with its own graceful signal, and every instance starts awake with yardd.

use crate::conns::{Closed, client_connections};
use crate::daemon::Context;
use crate::reconcile::port_owner;
use crate::services::default_instance;
use crate::state::Snapshot;
use crate::supervisor::conflict_of;
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
    /// How long an idle instance's sites stay listed in `run/sleeping` before its clients are
    /// counted again and it stops: long enough for a client that came in just after the first
    /// count to show.
    pub drain: Duration,
}

impl SleepConfig {
    /// A sample every 20 s, within the 30 s a closed connection stays in `TIME_WAIT`, so
    /// every one is seen (spec §7).
    pub fn production() -> Self {
        Self {
            sample_interval: Duration::from_secs(20),
            second: Duration::from_secs(1),
            drain: Duration::from_secs(1),
        }
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
    /// The closed connections the idle loop has counted.
    closed: Closed,
}

#[derive(Debug, Default)]
struct Instance {
    /// When it was last used; `None` until the idle loop first sees it running.
    used: Option<Instant>,
    /// When it fell asleep, while it sleeps.
    asleep: Option<OffsetDateTime>,
    /// When it fell asleep, from its wake until the idle loop sees it running again.
    waking: Option<OffsetDateTime>,
    /// Since when its sites are listed in `run/sleeping` before it stops, while it still runs.
    draining: Option<Instant>,
    /// The process the reconcile is stopping for its sleep, which shows as running until it
    /// exits.
    stopping: Option<u32>,
    /// A wake found another program on its port: its sites stay listed, so every `php` asks
    /// (and is refused) until it runs again.
    refused: bool,
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

    /// Starts draining the instance if it has been unused for `after` at `now`; true when it
    /// began. Its sites are then listed in `run/sleeping` while it still runs (spec §6.2).
    /// One lock covers the check and the change, so a use in between keeps it awake. A use
    /// after `now` counts as no idle time at all.
    pub fn drain_if_idle(&self, instance: &str, now: Instant, after: Duration) -> bool {
        let mut instances = self.lock();
        let entry = instances.entry(instance.to_string()).or_default();
        let idle = entry.used.map_or(Duration::ZERO, |used| now.saturating_duration_since(used));
        if idle < after {
            return false;
        }
        entry.draining = Some(now);
        true
    }

    /// Puts a draining instance to sleep unless it was used since it began to drain; true
    /// when it fell asleep.
    pub fn sleep_if_unused(&self, instance: &str, since: OffsetDateTime) -> bool {
        let mut instances = self.lock();
        let Some(entry) = instances.get_mut(instance) else { return false };
        let Some(began) = entry.draining.take() else { return false };
        if entry.used.is_some_and(|used| used > began) {
            return false;
        }
        entry.asleep = Some(since);
        entry.waking = None;
        true
    }

    /// The reconcile, as it hands the supervisor its units: `pid` is the process it is about
    /// to stop for the instance's sleep, `None` when it keeps the instance running. Recorded
    /// there rather than when the sleep is decided, since a wake in between keeps the unit.
    pub fn set_stopping(&self, instance: &str, pid: Option<u32>) {
        if let Some(entry) = self.lock().get_mut(instance) {
            entry.stopping = pid;
        }
    }

    /// A wake found another program on the instance's port.
    pub fn refuse(&self, instance: &str) -> bool {
        let mut instances = self.lock();
        let entry = instances.entry(instance.to_string()).or_default();
        !std::mem::replace(&mut entry.refused, true)
    }

    /// Undoes a sleep whose unit was never stopped, or a drain: it is awake, not waking, and
    /// its idle clock starts again.
    pub fn stay_awake(&self, instance: &str) {
        let mut instances = self.lock();
        let entry = instances.entry(instance.to_string()).or_default();
        entry.asleep = None;
        entry.waking = None;
        entry.draining = None;
        entry.stopping = None;
        entry.refused = false;
        entry.used = Some(Instant::now());
    }

    /// Whether the instance, whose unit reports running as `pid`, is up: false while that is
    /// still the process its sleep is stopping. Once up, it is no longer waking.
    pub fn running(&self, instance: &str, pid: Option<u32>) -> bool {
        let mut instances = self.lock();
        let Some(entry) = instances.get_mut(instance) else { return true };
        if entry.stopping.is_some() && entry.stopping == pid {
            return false;
        }
        entry.stopping = None;
        entry.waking = None;
        entry.refused = false;
        true
    }

    /// Wakes the instance; true when it was asleep. It is then waking, with the same
    /// `since`, until it is seen running, and its idle clock starts again.
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

    /// Whether the sites that need the instance belong in `run/sleeping`: it is asleep,
    /// draining on its way there, or refused because another program holds its port.
    pub fn is_listed(&self, instance: &str) -> bool {
        self.lock().get(instance).is_some_and(|i| i.asleep.is_some() || i.draining.is_some() || i.refused)
    }

    /// Every asleep instance and when it fell asleep.
    pub fn asleep(&self) -> BTreeMap<String, OffsetDateTime> {
        self.lock().iter().filter_map(|(name, i)| Some((name.clone(), i.asleep?))).collect()
    }

    pub fn is_refused(&self, instance: &str) -> bool {
        self.lock().get(instance).is_some_and(|i| i.refused)
    }

    pub fn waking_since(&self, instance: &str) -> Option<OffsetDateTime> {
        self.lock().get(instance).and_then(|i| i.waking)
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

/// `run/sleeping`: the linked sites that need an asleep or draining instance, which the shims
/// read. The reconcile writes it before it stops or starts any unit, under its lock, so it is
/// the file's only writer once yardd runs, and a shim run while an instance drains or stops
/// asks for a wake, which keeps it awake or waits for the stop. An unchanged list is not
/// rewritten.
pub fn write_sleeping(
    paths: &Paths,
    sites: &[SiteInfo],
    services: &ServicesConfig,
    projects: &Projects,
    sleeper: &Sleeper,
) {
    let sites: BTreeSet<String> = sites
        .iter()
        .filter(|site| instances_for(services, projects, &site.name).iter().any(|i| sleeper.is_listed(i)))
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
    let mut candidates = vec![];
    for (name, entry) in &snapshot.services.services {
        if entry.stopped || sleeper.is_asleep(name) {
            continue;
        }
        let Some(pid) =
            units.iter().find(|u| u.name == *name && u.state == UnitState::Running).and_then(|u| u.pid)
        else {
            continue;
        };
        // Running, and not the process a sleep is still stopping: it is no longer waking,
        // whether or not it may still sleep.
        if !sleeper.running(name, Some(pid)) {
            continue;
        }
        let (Some(secs), _) = effective_sleep(entry.sleep_after, snapshot.yard.service_sleep_after) else {
            continue;
        };
        sleeper.seen(name);
        candidates.push((name.clone(), pid, cfg.lasts(secs)));
    }
    if candidates.is_empty() {
        return;
    }
    touch_closed(ctx, &snapshot).await;
    let mut draining = vec![];
    for (name, pid, after) in candidates {
        if in_use(&name, pid).await {
            sleeper.touch(&name);
        }
        if sleeper.drain_if_idle(&name, Instant::now(), after) {
            draining.push((name, pid));
        }
    }
    if draining.is_empty() {
        return;
    }
    let names = draining.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ");
    // 1. Their sites are listed in `run/sleeping` while they still run, so a `php` or
    // `composer` from now on asks for a wake, which counts as use.
    if let Err(e) = ctx.reconciler.reconcile().await {
        tracing::warn!("putting {names} to sleep: {}", crate::describe(&e));
        for (name, _) in &draining {
            sleeper.stay_awake(name);
        }
        return;
    }
    tokio::time::sleep(cfg.drain).await;
    // 2. Counted again: a client that came in after the first count, or a `php` that read
    // `run/sleeping` a moment before it listed them, keeps an instance awake rather than being
    // cut off by its stop.
    let snapshot = ctx.reconciler.snapshot().await;
    if let Some(snapshot) = &snapshot {
        touch_closed(ctx, snapshot).await;
    }
    let mut fell = vec![];
    for (name, pid) in &draining {
        let may_sleep = snapshot.as_ref().and_then(|s| {
            let entry = s.services.services.get(name).filter(|e| !e.stopped)?;
            effective_sleep(entry.sleep_after, s.yard.service_sleep_after).0
        });
        if may_sleep.is_none() {
            sleeper.stay_awake(name);
            continue;
        }
        if in_use(name, *pid).await {
            sleeper.touch(name);
        }
        if sleeper.sleep_if_unused(name, OffsetDateTime::now_utc()) {
            fell.push(name.clone());
        }
    }
    if fell.is_empty() {
        // Nothing stops: this pass takes their sites out of `run/sleeping` again.
        if let Err(e) = ctx.reconciler.reconcile().await {
            tracing::warn!("keeping {names} awake: {}", crate::describe(&e));
        }
        return;
    }
    tracing::info!("{} unused for their sleep_after: putting them to sleep", fell.join(", "));
    // The reconcile leaves them out, which stops their units gracefully.
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

/// Whether the instance has a client now. One whose clients cannot be counted is in use.
async fn in_use(name: &str, pid: u32) -> bool {
    match tokio::task::spawn_blocking(move || client_connections(pid)).await {
        Ok(Ok(0)) => {}
        Ok(Ok(_)) => return true,
        Ok(Err(e)) => {
            tracing::warn!("cannot count {name}'s connections: {e}; it stays awake");
            return true;
        }
        Err(e) => {
            tracing::warn!("cannot count {name}'s connections: {e}; it stays awake");
            return true;
        }
    }
    false
}

/// Counts as use every instance a connection closed on since the last call, whichever
/// instance this pass is about, since each closed connection is counted only once. When they
/// cannot be read, every instance counts as used.
async fn touch_closed(ctx: &Context, snapshot: &Snapshot) {
    let ports: BTreeSet<u16> =
        snapshot.services.services.keys().flat_map(|instance| instance_ports(snapshot, instance)).collect();
    let sleeper = ctx.sleeper.clone();
    let closed = tokio::task::spawn_blocking(move || sleeper.closed.new_on(&ports))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    for instance in snapshot.services.services.keys() {
        let used = match &closed {
            Ok(closed) => instance_ports(snapshot, instance).iter().any(|port| closed.contains(port)),
            Err(_) => true,
        };
        if used {
            ctx.sleeper.touch(instance);
        }
    }
    if let Err(e) = &closed {
        tracing::warn!("cannot read closed connections: {e}; instances that may sleep stay awake");
    }
}

/// The ports `ports.toml` records for the instance.
fn instance_ports(snapshot: &Snapshot, instance: &str) -> Vec<u16> {
    snapshot
        .registry
        .entries
        .iter()
        .filter(|(unit, _)| port_owner(unit) == instance)
        .map(|(_, e)| e.port)
        .collect()
}

/// How often a wake checks whether the instances it woke run yet.
const WAKE_POLL: Duration = Duration::from_millis(20);

/// Counts as use of each of `instances` and wakes the asleep ones, then waits up to `wait`
/// until each one woken or still waking runs, has failed, or has no unit coming. An instance
/// the user stopped is never woken. It returns within `wait` even while the reconcile that
/// starts the units queues behind another (a stop in progress holds it): a wake that fails is
/// the unit's own `failed` state (spec §6.1). It answers the instances that cannot start
/// because another program holds their port, each with what holds it (empty when none).
pub async fn wake(ctx: &Context, instances: &[String], wait: Duration) -> Vec<String> {
    let deadline = Instant::now() + wait;
    let sleeper = &ctx.sleeper;
    let instances: Vec<String> = ctx
        .reconciler
        .read(|s| {
            instances
                .iter()
                .filter(|i| s.services.services.get(*i).is_some_and(|e| !e.stopped))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut woke = false;
    for instance in &instances {
        sleeper.touch(instance);
        woke |= sleeper.wake(instance);
    }
    let refused: Vec<&String> = instances.iter().filter(|i| sleeper.is_refused(i)).collect();
    // One that failed because another program held its port starts afresh once that program
    // has let go, so the site is not refused until someone restarts it.
    let mut retried = vec![];
    for instance in &instances {
        let Some(port) = conflict(ctx, instance) else { continue };
        let free = tokio::task::spawn_blocking(move || crate::probe::is_free(port, crate::probe::Proto::Tcp));
        if free.await.unwrap_or(false) && ctx.supervisor().restart_and_wait(instance).await.is_ok() {
            tracing::info!("{instance}'s port {port} is free again: starting it");
            retried.push(instance);
        }
    }
    if woke {
        tracing::info!("waking {}", instances.join(", "));
        // Spawned, so giving up at the deadline never cancels a reconcile halfway through.
        let reconciler = ctx.reconciler.clone();
        let mut run = tokio::spawn(async move {
            let result = reconciler.reconcile().await;
            if result.is_err() {
                // A config that no longer parses keeps the last good one serving, and that
                // includes starting what a site needs.
                reconciler.wake_from_last_good().await;
            }
            result
        });
        tokio::select! {
            done = &mut run => {
                if let Ok(Err(e)) = done {
                    tracing::warn!("waking: {}", crate::describe(&e));
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => return blocked(ctx, &instances, &refused),
        }
    }
    let waiting: Vec<&String> =
        instances.iter().filter(|i| sleeper.waking_since(i).is_some() || retried.contains(i)).collect();
    loop {
        let units = ctx.supervisor().list();
        let pending = waiting.iter().filter(|i| {
            match units.iter().find(|u| u.name == ***i).map(|u| (&u.state, u.pid)) {
                // Not yet when that is the process its sleep is still stopping.
                Some((UnitState::Running, pid)) => !sleeper.running(i, pid),
                Some((UnitState::Failed { .. }, _)) => false,
                Some(_) => true,
                // Another wake's reconcile may not have spawned it yet.
                None => !ctx.reconciler.is_left_out(i) && sleeper.waking_since(i).is_some(),
            }
        });
        if pending.count() == 0 || Instant::now() >= deadline {
            return blocked(ctx, &instances, &refused);
        }
        tokio::time::sleep(WAKE_POLL).await;
    }
}

/// Of `instances`, the ones that failed because another program holds their port, as
/// `<instance>: port 3307 is held by …`. Nothing should talk to that program in their place:
/// it could pose as the database. Each stays refused, its sites listed in `run/sleeping` so
/// every `php` asks, until it runs; `was_refused` are the ones that were before this wake, and
/// a change either way has a reconcile rewrite the file.
fn blocked(ctx: &Context, instances: &[String], was_refused: &[&String]) -> Vec<String> {
    let units = ctx.supervisor().list();
    let held: Vec<String> = instances
        .iter()
        .filter_map(|instance| {
            let unit = units.iter().find(|u| u.name == *instance)?;
            if !matches!(unit.state, UnitState::Failed { .. }) {
                return None;
            }
            let (_, held) = conflict_of(unit.last_error.as_deref()?)?;
            Some(format!("{instance}: {held}"))
        })
        .collect();
    let mut changed = false;
    for instance in instances.iter().filter(|i| held.iter().any(|h| h.starts_with(&format!("{i}: ")))) {
        changed |= ctx.sleeper.refuse(instance);
    }
    changed |= was_refused.iter().any(|i| !ctx.sleeper.is_refused(i));
    if changed {
        let reconciler = ctx.reconciler.clone();
        tokio::spawn(async move {
            if let Err(e) = reconciler.reconcile().await {
                tracing::warn!("listing refused sites: {}", crate::describe(&e));
            }
        });
    }
    held
}

/// The port another program held when the instance's unit failed, while it is `failed` so.
fn conflict(ctx: &Context, instance: &str) -> Option<u16> {
    let unit = ctx.supervisor().list().into_iter().find(|u| u.name == instance)?;
    if !matches!(unit.state, UnitState::Failed { .. }) {
        return None;
    }
    conflict_of(unit.last_error.as_deref()?).map(|(port, _)| port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_drains_only_after_the_whole_sleep_after() {
        let sleeper = Sleeper::default();
        sleeper.touch("redis");
        let after = Duration::from_secs(60);
        assert!(!sleeper.drain_if_idle("redis", Instant::now() + Duration::from_secs(30), after));
        assert!(!sleeper.is_listed("redis"));
        assert!(sleeper.drain_if_idle("redis", Instant::now() + Duration::from_secs(61), after));
        assert!(sleeper.is_listed("redis"), "a draining instance's sites are listed");
        assert!(!sleeper.is_asleep("redis"), "and it still runs");
        assert!(sleeper.sleep_if_unused("redis", OffsetDateTime::UNIX_EPOCH));
        assert_eq!(sleeper.asleep().get("redis"), Some(&OffsetDateTime::UNIX_EPOCH));
    }

    #[test]
    fn a_touch_after_the_check_began_keeps_it_awake() {
        let sleeper = Sleeper::default();
        let now = Instant::now();
        sleeper.touch("redis");
        // The check began before the touch, so the instance was not idle at all.
        assert!(!sleeper.drain_if_idle("redis", now, Duration::from_nanos(1)));
    }

    #[test]
    fn a_use_while_it_drains_keeps_it_awake() {
        let sleeper = Sleeper::default();
        sleeper.touch("redis");
        assert!(sleeper.drain_if_idle("redis", Instant::now(), Duration::ZERO));
        assert!(sleeper.sleep_if_unused("redis", OffsetDateTime::UNIX_EPOCH), "no use since it began");

        let sleeper = Sleeper::default();
        sleeper.touch("redis");
        assert!(sleeper.drain_if_idle("redis", Instant::now(), Duration::ZERO));
        std::thread::sleep(Duration::from_millis(2));
        sleeper.touch("redis");
        assert!(!sleeper.sleep_if_unused("redis", OffsetDateTime::UNIX_EPOCH), "used while it drained");
        assert!(!sleeper.is_listed("redis"), "and no longer listed");
    }

    #[test]
    fn the_stopping_process_is_not_the_instance_running() {
        let sleeper = Sleeper::default();
        sleeper.touch("redis");
        assert!(sleeper.drain_if_idle("redis", Instant::now(), Duration::ZERO));
        assert!(sleeper.sleep_if_unused("redis", OffsetDateTime::UNIX_EPOCH));
        sleeper.set_stopping("redis", Some(7));
        assert!(sleeper.wake("redis"));
        assert!(!sleeper.running("redis", Some(7)), "pid 7 is the one still stopping");
        assert!(sleeper.waking_since("redis").is_some());
        assert!(sleeper.running("redis", Some(8)), "the new process");
        assert_eq!(sleeper.waking_since("redis"), None);
    }

    #[test]
    fn a_wake_before_the_stop_leaves_its_process_running() {
        // Woken between the decision and the reconcile that would stop it: that reconcile keeps
        // the unit, so its process is the instance, now and later.
        let sleeper = Sleeper::default();
        sleeper.touch("redis");
        assert!(sleeper.drain_if_idle("redis", Instant::now(), Duration::ZERO));
        assert!(sleeper.sleep_if_unused("redis", OffsetDateTime::UNIX_EPOCH));
        assert!(sleeper.wake("redis"));
        assert!(sleeper.running("redis", Some(7)), "pid 7 was never stopped");
        assert_eq!(sleeper.waking_since("redis"), None);
    }

    #[test]
    fn stay_awake_undoes_a_sleep() {
        let sleeper = Sleeper::default();
        sleeper.touch("redis");
        assert!(sleeper.drain_if_idle("redis", Instant::now(), Duration::ZERO));
        assert!(sleeper.sleep_if_unused("redis", OffsetDateTime::UNIX_EPOCH));
        sleeper.stay_awake("redis");
        assert!(!sleeper.is_asleep("redis"));
        assert_eq!(sleeper.waking_since("redis"), None, "it never stopped, so it is not waking");
        assert!(sleeper.running("redis", Some(7)), "and its process is the instance");
        assert!(!sleeper.drain_if_idle(
            "redis",
            Instant::now() + Duration::from_secs(30),
            Duration::from_secs(60)
        ));
    }
}
