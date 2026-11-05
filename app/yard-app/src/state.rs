//! `AppState` (M6 design §7.1): the app's view of yardd, one `Resource` per list, kept fresh by
//! the refresh policy in yard-app-model and the update stream from the session.

use crate::daemon::{BoxFuture, Daemon, call};
use gpui::{App, AppContext, Context, Entity, Task};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::sync::oneshot;
use yard_app_model::jobs::{JobKey, JobPhase, JobsStore};
use yard_app_model::refresh::{RefreshQueue, ResourceKey, invalidation};
use yard_client::api;
use yard_client::session::{ConnectionState, Update};
use yard_proto::methods::DoctorParams;
use yard_proto::{
    CaInfo, DoctorReport, ExtInfo, NodeList, PhpInfo, PortInfo, ServiceInfo, SiteDetail, SiteInfo,
    SourceInfo, StatusInfo, TunnelDomain, TunnelInfo, YardError,
};

/// The last value of a list, the last error fetching it, whether a fetch is in flight, and when it
/// last succeeded. A failed fetch keeps the value it had.
#[derive(Debug, Clone)]
pub struct Resource<T> {
    pub value: Option<T>,
    pub error: Option<YardError>,
    pub loading: bool,
    pub updated_at: Option<OffsetDateTime>,
}

impl<T> Default for Resource<T> {
    fn default() -> Self {
        Resource { value: None, error: None, loading: false, updated_at: None }
    }
}

impl<T: DeserializeOwned> Resource<T> {
    fn put(&mut self, answer: Result<Value, YardError>) {
        self.loading = false;
        let decoded = answer.and_then(|v| serde_json::from_value(v).map_err(YardError::internal));
        match decoded {
            Ok(v) => {
                self.value = Some(v);
                self.error = None;
                self.updated_at = Some(OffsetDateTime::now_utc());
            }
            Err(e) => self.error = Some(e),
        }
    }
}

pub struct AppState {
    daemon: Arc<dyn Daemon>,
    debounce: Duration,
    pub connection: ConnectionState,
    pub epoch: u64,
    pub status: Resource<StatusInfo>,
    pub sites: Resource<Vec<SiteInfo>>,
    pub services: Resource<Vec<ServiceInfo>>,
    pub php: Resource<Vec<PhpInfo>>,
    pub node: Resource<NodeList>,
    pub tunnels: Resource<Vec<TunnelInfo>>,
    pub tunnel_domains: Resource<Vec<TunnelDomain>>,
    pub ports: Resource<Vec<PortInfo>>,
    pub sources: Resource<Vec<SourceInfo>>,
    pub ca: Resource<CaInfo>,
    pub extensions: HashMap<String, Resource<Vec<ExtInfo>>>,
    pub site_detail: HashMap<String, Resource<SiteDetail>>,
    pub doctor: Resource<DoctorReport>,
    pub jobs: JobsStore,
    queue: RefreshQueue,
    flush: Option<Task<()>>,
    waiters: Vec<(JobKey, oneshot::Sender<JobPhase>)>,
    _updates: Task<()>,
}

impl AppState {
    /// Starts reading the daemon's updates; fetches are coalesced over `debounce`.
    pub fn new(daemon: Arc<dyn Daemon>, debounce: Duration, cx: &mut App) -> Entity<AppState> {
        cx.new(|cx| {
            let mut updates = daemon.take_updates();
            let reader = cx.spawn(async move |this, cx| {
                while let Some(update) = updates.recv().await {
                    if this.update(cx, |s: &mut AppState, cx| s.on_update(update, cx)).is_err() {
                        return;
                    }
                }
            });
            AppState {
                daemon,
                debounce,
                connection: ConnectionState::Connecting,
                epoch: 0,
                status: Resource::default(),
                sites: Resource::default(),
                services: Resource::default(),
                php: Resource::default(),
                node: Resource::default(),
                tunnels: Resource::default(),
                tunnel_domains: Resource::default(),
                ports: Resource::default(),
                sources: Resource::default(),
                ca: Resource::default(),
                extensions: HashMap::new(),
                site_detail: HashMap::new(),
                doctor: Resource::default(),
                jobs: JobsStore::default(),
                queue: RefreshQueue::default(),
                flush: None,
                waiters: vec![],
                _updates: reader,
            }
        })
    }

    #[expect(dead_code, reason = "the screens reach yardd through AppState (M6 Tasks 13-16)")]
    pub fn daemon(&self) -> Arc<dyn Daemon> {
        self.daemon.clone()
    }

    pub fn refresh(&mut self, keys: impl IntoIterator<Item = ResourceKey>, cx: &mut Context<Self>) {
        self.queue.invalidate(keys);
        self.schedule(cx);
    }

    /// The window or menu became active: `status`, and what is on screen.
    pub fn became_active(&mut self, visible: &[ResourceKey], cx: &mut Context<Self>) {
        self.refresh(std::iter::once(ResourceKey::Status).chain(visible.iter().cloned()), cx);
    }

    /// `doctor` is slow and runs brew, so it is fetched only when asked.
    pub fn run_doctor(&mut self, cx: &mut Context<Self>) {
        self.doctor.loading = true;
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            let answer = call(&*daemon, api::DOCTOR, DoctorParams::default()).await.map(|r| json!(r));
            let _ = this.update(cx, |s, cx| {
                s.doctor.put(answer);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Records a job this app started, so its events and the answer make one record.
    pub fn claim(&mut self, job_id: &str, title: String) -> JobKey {
        self.jobs.claim(job_id, self.epoch, title, OffsetDateTime::now_utc());
        JobKey { epoch: self.epoch, id: job_id.to_string() }
    }

    /// Resolves with the job's end, at once if it has already ended.
    pub fn wait_job(&mut self, key: JobKey) -> oneshot::Receiver<JobPhase> {
        let (tx, rx) = oneshot::channel();
        match self.jobs.phase(&key) {
            Some(p) if p.is_finished() => {
                let _ = tx.send(p.clone());
            }
            _ => self.waiters.push((key, tx)),
        }
        rx
    }

    fn on_update(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::State(state) => {
                if let ConnectionState::Connected(info) = &state {
                    if info.epoch != self.epoch {
                        self.epoch = info.epoch;
                        self.jobs.epoch_changed(info.epoch, OffsetDateTime::now_utc());
                        self.resolve_waiters();
                    }
                    let mut keys = self.queue.loaded();
                    keys.insert(ResourceKey::Status);
                    self.refresh(keys, cx);
                }
                self.connection = state;
            }
            Update::Event(event) => {
                self.jobs.apply(&event, self.epoch, OffsetDateTime::now_utc());
                self.resolve_waiters();
                let services: HashSet<String> =
                    self.services.value.iter().flatten().map(|s| s.name.clone()).collect();
                let i = invalidation::keys(&event, &services);
                if let Some((unit, state)) = i.patch_unit
                    && let Some(u) =
                        self.status.value.as_mut().and_then(|s| s.units.iter_mut().find(|u| u.name == unit))
                {
                    u.state = state;
                }
                if let Some(t) = i.upsert_tunnel
                    && let Some(list) = self.tunnels.value.as_mut()
                {
                    match list.iter_mut().find(|x| x.site == t.site) {
                        Some(x) => *x = t,
                        None => list.push(t),
                    }
                }
                let mut keys = i.keys;
                if i.all_extensions {
                    keys.extend(self.extensions.keys().map(|v| ResourceKey::Extensions(v.clone())));
                }
                if i.all_site_details {
                    keys.extend(self.site_detail.keys().map(|n| ResourceKey::SiteDetail(n.clone())));
                }
                if !keys.is_empty() {
                    self.refresh(keys, cx);
                }
            }
        }
        cx.notify();
    }

    fn resolve_waiters(&mut self) {
        let jobs = &self.jobs;
        let waiters = std::mem::take(&mut self.waiters);
        for (key, tx) in waiters {
            match jobs.phase(&key) {
                Some(p) if p.is_finished() => {
                    let _ = tx.send(p.clone());
                }
                _ => self.waiters.push((key, tx)),
            }
        }
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        if self.flush.is_some() {
            return;
        }
        let wait = self.debounce;
        self.flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |s, cx| {
                s.flush = None;
                s.fetch_due(cx);
            });
        }));
    }

    fn fetch_due(&mut self, cx: &mut Context<Self>) {
        for key in self.queue.due() {
            self.queue.started(key.clone());
            let fetch = self.fetch(&key);
            cx.spawn(async move |this, cx| {
                let answer = fetch.await;
                let _ = this.update(cx, |s, cx| {
                    s.store(&key, answer);
                    if s.queue.finished(key) {
                        s.schedule(cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn fetch(&mut self, key: &ResourceKey) -> BoxFuture<'static, Result<Value, YardError>> {
        let (name, params) = match key {
            ResourceKey::Status => (api::STATUS.name, Value::Null),
            ResourceKey::Sites => (api::SITES_LIST.name, Value::Null),
            ResourceKey::Services => (api::SERVICES_LIST.name, Value::Null),
            ResourceKey::Php => (api::PHP_LIST.name, json!({"available": true})),
            ResourceKey::Node => (api::NODE_LIST.name, Value::Null),
            ResourceKey::Tunnels => (api::TUNNELS_LIST.name, Value::Null),
            ResourceKey::TunnelDomains => (api::TUNNELS_DOMAINS.name, Value::Null),
            ResourceKey::Ports => (api::PORTS_LIST.name, Value::Null),
            ResourceKey::Sources => (api::SOURCES_LIST.name, Value::Null),
            ResourceKey::Ca => (api::CA_INFO.name, Value::Null),
            ResourceKey::Extensions(v) => {
                self.extensions.entry(v.clone()).or_default().loading = true;
                (api::EXT_LIST.name, json!({"php": v}))
            }
            ResourceKey::SiteDetail(n) => {
                self.site_detail.entry(n.clone()).or_default().loading = true;
                (api::SITES_INFO.name, json!({"site": n}))
            }
        };
        self.daemon.call_value(name, false, params)
    }

    fn store(&mut self, key: &ResourceKey, answer: Result<Value, YardError>) {
        match key {
            ResourceKey::Status => self.status.put(answer),
            ResourceKey::Sites => self.sites.put(answer),
            ResourceKey::Services => self.services.put(answer),
            ResourceKey::Php => self.php.put(answer),
            ResourceKey::Node => self.node.put(answer),
            ResourceKey::Tunnels => self.tunnels.put(answer),
            ResourceKey::TunnelDomains => self.tunnel_domains.put(answer),
            ResourceKey::Ports => self.ports.put(answer),
            ResourceKey::Sources => self.sources.put(answer),
            ResourceKey::Ca => self.ca.put(answer),
            ResourceKey::Extensions(v) => self.extensions.entry(v.clone()).or_default().put(answer),
            ResourceKey::SiteDetail(n) => self.site_detail.entry(n.clone()).or_default().put(answer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDaemon, connected, site, status};
    use gpui::TestAppContext;
    use serde_json::json;
    use std::time::Duration;
    use yard_app_model::refresh::ResourceKey;
    use yard_proto::{ErrorCode, Event, YardError};

    const DEBOUNCE: Duration = Duration::from_millis(50);

    fn settle(cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(DEBOUNCE * 2);
        cx.run_until_parked();
    }

    fn started(cx: &mut TestAppContext, fake: &Arc<FakeDaemon>) -> Entity<AppState> {
        fake.answer("status", status());
        fake.answer("sites.list", json!([site("blog", json!([]))]));
        let state = cx.update(|cx| AppState::new(fake.clone(), DEBOUNCE, cx));
        fake.push(connected(1));
        settle(cx);
        state
    }

    #[gpui::test]
    async fn connect_fetches_status_and_loaded_resources(cx: &mut TestAppContext) {
        let fake = FakeDaemon::new();
        let state = started(cx, &fake);
        assert_eq!(
            (fake.count("status"), fake.count("sites.list")),
            (1, 0),
            "only status until a list is shown"
        );
        assert!(state.read_with(cx, |s, _| s.status.value.is_some()));
        state.update(cx, |s, cx| s.refresh([ResourceKey::Sites], cx));
        settle(cx);
        assert_eq!(fake.count("sites.list"), 1);
        fake.push(connected(2));
        settle(cx);
        assert_eq!(
            (fake.count("status"), fake.count("sites.list")),
            (2, 2),
            "a new epoch fetches what was loaded"
        );
    }

    #[gpui::test]
    async fn reconciled_refetches_once_after_debounce(cx: &mut TestAppContext) {
        let fake = FakeDaemon::new();
        let state = started(cx, &fake);
        for _ in 0..3 {
            fake.event(Event::Reconciled { config_error: None });
        }
        cx.run_until_parked();
        assert_eq!(fake.count("sites.list"), 0, "nothing before the debounce");
        settle(cx);
        assert_eq!(fake.count("sites.list"), 1);
        assert_eq!(state.read_with(cx, |s, _| s.sites.value.as_ref().map(Vec::len)), Some(1));
    }

    #[gpui::test]
    async fn a_failed_fetch_keeps_the_last_value(cx: &mut TestAppContext) {
        let fake = FakeDaemon::new();
        let state = started(cx, &fake);
        state.update(cx, |s, cx| s.refresh([ResourceKey::Sites], cx));
        settle(cx);
        fake.fail("sites.list", YardError::new(ErrorCode::Internal, "lost"));
        state.update(cx, |s, cx| s.refresh([ResourceKey::Sites], cx));
        settle(cx);
        state.read_with(cx, |s, _| {
            assert_eq!(s.sites.value.as_ref().map(Vec::len), Some(1));
            assert_eq!(s.sites.error.as_ref().map(|e| e.message.as_str()), Some("lost"));
        });
    }

    #[gpui::test]
    async fn doctor_runs_only_when_asked(cx: &mut TestAppContext) {
        let fake = FakeDaemon::new();
        fake.answer("doctor", json!({"checks": []}));
        let state = started(cx, &fake);
        fake.event(Event::Reconciled { config_error: None });
        settle(cx);
        assert_eq!(fake.count("doctor"), 0);
        state.update(cx, |s, cx| s.run_doctor(cx));
        settle(cx);
        assert_eq!(fake.count("doctor"), 1);
        assert!(state.read_with(cx, |s, _| s.doctor.value.is_some()));
    }

    #[gpui::test]
    async fn became_active_refetches_the_visible_lists(cx: &mut TestAppContext) {
        let fake = FakeDaemon::new();
        let state = started(cx, &fake);
        state.update(cx, |s, cx| s.became_active(&[ResourceKey::Sites], cx));
        settle(cx);
        assert_eq!((fake.count("status"), fake.count("sites.list")), (2, 1));
    }
}
