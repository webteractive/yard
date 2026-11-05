//! Which of the app's resources an event makes stale (M6 design §7.2), and the queue that fetches
//! each at most once at a time. The 150 ms coalescing is the caller's timer.

use std::collections::HashSet;

/// A list or detail the app fetches and shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceKey {
    Status,
    Sites,
    Services,
    Php,
    Node,
    Tunnels,
    TunnelDomains,
    Ports,
    Sources,
    Ca,
    /// `ext.list` for one PHP version.
    Extensions(String),
    /// `sites.info` for one site.
    SiteDetail(String),
}

pub mod invalidation {
    use super::ResourceKey;
    use std::collections::HashSet;
    use yard_proto::{Event, TunnelInfo, UnitState};

    /// What one event asks of the app: lists to fetch again, and changes it can apply at once.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Invalidation {
        pub keys: HashSet<ResourceKey>,
        /// Every `Extensions(_)` loaded so far.
        pub all_extensions: bool,
        /// Every `SiteDetail(_)` loaded so far.
        pub all_site_details: bool,
        /// `status.units`, patched in place before anything is fetched.
        pub patch_unit: Option<(String, UnitState)>,
        pub upsert_tunnel: Option<TunnelInfo>,
    }

    /// What a reconcile may have changed: every list the config files decide.
    pub const AFTER_RECONCILE: &[ResourceKey] = &[
        ResourceKey::Status,
        ResourceKey::Sites,
        ResourceKey::Services,
        ResourceKey::Php,
        ResourceKey::Node,
        ResourceKey::Tunnels,
        ResourceKey::Ports,
    ];

    /// What a finished job may have changed: the same, plus what only jobs change.
    pub const AFTER_JOB: &[ResourceKey] = &[
        ResourceKey::Status,
        ResourceKey::Sites,
        ResourceKey::Services,
        ResourceKey::Php,
        ResourceKey::Node,
        ResourceKey::Tunnels,
        ResourceKey::Ports,
        ResourceKey::Sources,
        ResourceKey::TunnelDomains,
        ResourceKey::Ca,
    ];

    /// The table in design §7.2. `services` names the service instances, whose unit changes
    /// refetch the services list.
    pub fn keys(event: &Event, services: &HashSet<String>) -> Invalidation {
        let mut i = Invalidation::default();
        let all = |i: &mut Invalidation, keys: &[ResourceKey]| {
            i.keys.extend(keys.iter().cloned());
            i.all_extensions = true;
            i.all_site_details = true;
        };
        match event {
            Event::Reconciled { .. } => all(&mut i, AFTER_RECONCILE),
            Event::JobFinished { .. } => all(&mut i, AFTER_JOB),
            Event::UnitStateChanged { unit, state } => {
                i.patch_unit = Some((unit.clone(), state.clone()));
                if services.contains(unit) {
                    i.keys.insert(ResourceKey::Services);
                } else if unit.starts_with("php-fpm-") {
                    i.keys.insert(ResourceKey::Php);
                } else if unit.starts_with("tunnel-") {
                    i.keys.insert(ResourceKey::Tunnels);
                }
            }
            Event::SiteAdded { .. } | Event::SiteRemoved { .. } => {
                i.keys.extend([ResourceKey::Sites, ResourceKey::Status]);
            }
            Event::PortConflict { .. } => {
                i.keys.insert(ResourceKey::Ports);
            }
            Event::TunnelChanged { tunnel } => i.upsert_tunnel = Some((**tunnel).clone()),
            Event::JobProgress { .. } | Event::LogLine { .. } | Event::Unknown => {}
        }
        i
    }
}

/// At most one fetch per resource in flight, and one more if it was invalidated meanwhile.
#[derive(Debug, Default)]
pub struct RefreshQueue {
    invalidated: HashSet<ResourceKey>,
    in_flight: HashSet<ResourceKey>,
    again: HashSet<ResourceKey>,
    loaded: HashSet<ResourceKey>,
}

impl RefreshQueue {
    pub fn invalidate(&mut self, keys: impl IntoIterator<Item = ResourceKey>) {
        for key in keys {
            if self.in_flight.contains(&key) {
                self.again.insert(key);
            } else {
                self.invalidated.insert(key);
            }
        }
    }

    /// The keys to fetch now, in a stable order; each is then `started`.
    pub fn due(&mut self) -> Vec<ResourceKey> {
        let mut due: Vec<ResourceKey> =
            self.invalidated.iter().filter(|k| !self.in_flight.contains(*k)).cloned().collect();
        due.sort();
        for k in &due {
            self.invalidated.remove(k);
        }
        due
    }

    pub fn started(&mut self, key: ResourceKey) {
        self.in_flight.insert(key);
    }

    /// Ends a fetch; `true` when the key was invalidated during it, so it is due again.
    pub fn finished(&mut self, key: ResourceKey) -> bool {
        self.in_flight.remove(&key);
        self.loaded.insert(key.clone());
        if self.again.remove(&key) {
            self.invalidated.insert(key);
            return true;
        }
        false
    }

    /// Every key fetched at least once: what a new epoch fetches again.
    pub fn loaded(&self) -> HashSet<ResourceKey> {
        self.loaded.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::invalidation::{AFTER_JOB, AFTER_RECONCILE, keys};
    use super::*;
    use serde_json::json;
    use yard_proto::{Event, TunnelInfo, UnitState};

    fn services() -> HashSet<String> {
        HashSet::from(["redis".to_string(), "mysql-8.4".to_string()])
    }

    fn set(keys: &[ResourceKey]) -> HashSet<ResourceKey> {
        keys.iter().cloned().collect()
    }

    #[test]
    fn reconciled_refetches_config_lists() {
        let i = keys(&Event::Reconciled { config_error: None }, &services());
        assert_eq!(i.keys, set(AFTER_RECONCILE));
        assert!(i.all_extensions && i.all_site_details);
        assert_eq!(
            set(AFTER_RECONCILE),
            set(&[
                ResourceKey::Status,
                ResourceKey::Sites,
                ResourceKey::Services,
                ResourceKey::Php,
                ResourceKey::Node,
                ResourceKey::Tunnels,
                ResourceKey::Ports
            ])
        );
    }

    #[test]
    fn job_finished_refetches_everything() {
        let e = Event::JobFinished { job_id: "php-install-1".into(), error: None, result: None };
        let i = keys(&e, &services());
        assert_eq!(i.keys, set(AFTER_JOB));
        assert!(i.all_extensions && i.all_site_details);
        for extra in [ResourceKey::Sources, ResourceKey::TunnelDomains, ResourceKey::Ca] {
            assert!(i.keys.contains(&extra), "{extra:?}");
        }
        assert!(set(AFTER_RECONCILE).is_subset(&i.keys));
    }

    #[test]
    fn unit_state_change_patches_then_refetches_its_list() {
        let change = |unit: &str| {
            keys(&Event::UnitStateChanged { unit: unit.into(), state: UnitState::Running }, &services())
        };
        let redis = change("redis");
        assert_eq!(redis.patch_unit, Some(("redis".to_string(), UnitState::Running)));
        assert_eq!(redis.keys, set(&[ResourceKey::Services]));
        assert_eq!(change("php-fpm-8.4").keys, set(&[ResourceKey::Php]));
        assert_eq!(change("tunnel-blog").keys, set(&[ResourceKey::Tunnels]));
        assert!(change("nginx").keys.is_empty(), "the patch is enough");
    }

    #[test]
    fn site_added_refetches_sites() {
        let i = keys(&Event::SiteRemoved { name: "blog".into() }, &services());
        assert_eq!(i.keys, set(&[ResourceKey::Sites, ResourceKey::Status]));
    }

    #[test]
    fn port_conflict_refetches_ports() {
        let e = Event::PortConflict { unit: "redis".into(), port: 6379, holder: None };
        assert_eq!(keys(&e, &services()).keys, set(&[ResourceKey::Ports]));
    }

    #[test]
    fn tunnel_changed_upserts() {
        let tunnel: TunnelInfo = serde_json::from_value(json!({
            "site": "blog", "provider": "cloudflare", "kind": "quick", "url": null, "host": null,
            "tunnel": null, "state": "connecting", "unit": "tunnel-blog", "origin_port": 33001,
            "metrics_port": 33002, "last_error": null
        }))
        .unwrap();
        let i = keys(&Event::TunnelChanged { tunnel: Box::new(tunnel.clone()) }, &services());
        assert_eq!(i.upsert_tunnel, Some(tunnel));
        assert!(i.keys.is_empty());
    }

    #[test]
    fn unknown_event_changes_nothing() {
        let i = keys(&Event::Unknown, &services());
        assert!(i.keys.is_empty() && !i.all_extensions && !i.all_site_details);
        assert!(i.patch_unit.is_none() && i.upsert_tunnel.is_none());
    }

    #[test]
    fn invalidations_coalesce_while_in_flight() {
        let mut q = RefreshQueue::default();
        q.invalidate([ResourceKey::Sites]);
        assert_eq!(q.due(), vec![ResourceKey::Sites]);
        q.started(ResourceKey::Sites);
        for _ in 0..3 {
            q.invalidate([ResourceKey::Sites]);
        }
        assert!(q.due().is_empty(), "one fetch in flight at a time");
        assert!(q.finished(ResourceKey::Sites), "invalidated meanwhile, so one more");
        assert_eq!(q.due(), vec![ResourceKey::Sites]);
        q.started(ResourceKey::Sites);
        assert!(!q.finished(ResourceKey::Sites));
        assert!(q.due().is_empty());
    }

    #[test]
    fn a_new_epoch_refetches_what_was_loaded() {
        let mut q = RefreshQueue::default();
        for k in [ResourceKey::Sites, ResourceKey::Extensions("8.4".into())] {
            q.invalidate([k.clone()]);
            q.started(k.clone());
            q.finished(k);
        }
        assert_eq!(q.loaded(), set(&[ResourceKey::Sites, ResourceKey::Extensions("8.4".into())]));
    }
}
