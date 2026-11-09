//! `ActionRunner` (M6 design §9, §10, §11): runs what a person asks for, keeps each action's last
//! error, follows jobs to their end, and asks the trust question before an install brew would refuse.

use crate::daemon::{Daemon, call};
use crate::state::AppState;
use futures::channel::oneshot;
use gpui::{App, AppContext, Context, Entity, Task};
use serde::Serialize;
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
pub use yard_app_model::actions::{ActionErrors, ActionId, Prompt, trust_prompt};
use yard_app_model::jobs::JobPhase;
use yard_app_model::settle::{SettleStage, next_stage};
use yard_client::api::{self, Method};
use yard_proto::methods::{
    ExtAddParams, JobParams, LinkParams, NodeInstallParams, PhpParams, ServiceAddParams, SourceParams,
};
use yard_proto::{ErrorCode, JobStarted, SiteInfo, YardError};

pub struct ActionRunner {
    daemon: Arc<dyn Daemon>,
    state: Entity<AppState>,
    pub errors: ActionErrors,
    /// Questions waiting for the user, oldest first, each with the work waiting on its answer.
    questions: VecDeque<(Prompt, oneshot::Sender<usize>)>,
}

impl ActionRunner {
    pub fn new(daemon: Arc<dyn Daemon>, state: Entity<AppState>, cx: &mut App) -> Entity<ActionRunner> {
        cx.new(|_| ActionRunner {
            daemon,
            state,
            errors: ActionErrors::default(),
            questions: VecDeque::new(),
        })
    }

    /// The question to ask now; the next waits until it is answered.
    pub fn prompt(&self) -> Option<Prompt> {
        self.questions.front().map(|(p, _)| p.clone())
    }

    /// Runs `work`; its error stays beside the action's control until it succeeds.
    pub fn perform<R: 'static>(
        &mut self,
        id: ActionId,
        work: impl Future<Output = Result<R, YardError>> + 'static,
        cx: &mut Context<Self>,
    ) -> Task<Option<R>> {
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |s, cx| {
                cx.notify();
                match result {
                    Ok(v) => {
                        s.errors.clear(&id);
                        Some(v)
                    }
                    Err(e) => {
                        s.errors.record(id, e);
                        None
                    }
                }
            })
            .ok()
            .flatten()
        })
    }

    /// Starts a job and resolves with its end. Without yardd's events its end could not be known,
    /// so it is refused; a `FORMULA_UNTRUSTED` asks the trust question and remembers the job.
    pub fn run_job<P: Serialize + Clone + 'static>(
        &mut self,
        id: ActionId,
        m: Method<P, JobStarted>,
        params: P,
        title: String,
        cx: &mut Context<Self>,
    ) -> Task<JobPhase> {
        if !self.daemon.can_follow_jobs() {
            let e = YardError::new(
                ErrorCode::Internal,
                "cannot follow the job's progress without yardd's events",
            )
            .with_hint("yard start");
            self.errors.record(id, e.clone());
            cx.notify();
            return Task::ready(JobPhase::Failed(e));
        }
        let daemon = self.daemon.clone();
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let started = call(&*daemon, m, params.clone()).await;
            let phase = match started {
                // brew refuses the formula until it is trusted: ask, and on yes trust it and run
                // the job again, so whatever waits on this job (a settle) carries on.
                Err(e) if let Some(Prompt::Trust { formula, tap }) = trust_prompt(&e) => {
                    let (tx, answer) = oneshot::channel();
                    let prompt = Prompt::Trust { formula: formula.clone(), tap };
                    let asked = this.update(cx, |s, cx| {
                        s.questions.push_back((prompt, tx));
                        cx.notify();
                    });
                    if asked.is_err() || answer.await != Ok(0) {
                        JobPhase::Failed(e)
                    } else {
                        let trust = this.update(cx, |s, cx| {
                            s.run_job(
                                ActionId(format!("source:{formula}:trust")),
                                api::SOURCES_TRUST,
                                SourceParams { target: formula.clone(), tap: false },
                                format!("Trust {formula}"),
                                cx,
                            )
                        });
                        let trusted = match trust {
                            Ok(trust) => matches!(trust.await, JobPhase::Succeeded(_)),
                            Err(_) => false,
                        };
                        let again = trusted
                            .then(|| {
                                this.update(cx, |s, cx| s.run_job(id.clone(), m, params, title, cx)).ok()
                            })
                            .flatten();
                        match again {
                            Some(again) => again.await,
                            None => JobPhase::Failed(e),
                        }
                    }
                }
                Err(e) => JobPhase::Failed(e),
                Ok(started) => {
                    let ended = state.update(cx, |st, _| {
                        let key = st.claim(&started.job_id, title);
                        st.wait_job(key)
                    });
                    ended.await.unwrap_or(JobPhase::Lost)
                }
            };
            let _ = this.update(cx, |s, cx| {
                match &phase {
                    JobPhase::Succeeded(_) => s.errors.clear(&id),
                    JobPhase::Failed(e) => s.errors.record(id.clone(), e.clone()),
                    _ => {}
                }
                cx.notify();
            });
            phase
        })
    }

    /// Cancels a job; one that already ended (`JOB_NOT_FOUND`) is not an error.
    pub fn cancel_job(&mut self, job_id: String, cx: &mut Context<Self>) -> Task<Option<()>> {
        let daemon = self.daemon.clone();
        let id = ActionId(format!("job:{job_id}:cancel"));
        self.perform(
            id,
            async move {
                match call(&*daemon, api::JOBS_CANCEL, JobParams { job_id }).await {
                    Ok(_) => Ok(()),
                    Err(e) if e.code == ErrorCode::JobNotFound => Ok(()),
                    Err(e) => Err(e),
                }
            },
            cx,
        )
    }

    /// The answer to the question asked now: button 0 is yes. Yes to the trust question trusts
    /// the formula as a job, then repeats the work that asked, once.
    pub fn answer(&mut self, choice: usize, cx: &mut Context<Self>) {
        if let Some((_, waiting)) = self.questions.pop_front() {
            let _ = waiting.send(choice);
        }
        cx.notify();
    }

    /// Links `folder` as `name`, sending the folder's canonical path.
    pub fn link(&mut self, folder: PathBuf, name: String, cx: &mut Context<Self>) -> Task<Option<SiteInfo>> {
        let daemon = self.daemon.clone();
        self.perform(
            ActionId("site:link".into()),
            async move {
                let path = std::fs::canonicalize(&folder).map_err(|e| {
                    YardError::new(ErrorCode::InvalidArgument, format!("{}: {e}", folder.display()))
                })?;
                call(&*daemon, api::SITES_LINK, LinkParams { name, path: path.display().to_string() }).await
            },
            cx,
        )
    }

    /// Installs what the site's `.yard` asks for, stage by stage, refetching the site between
    /// stages; stops at the first job that does not succeed.
    pub fn settle(&mut self, site: SiteInfo, cx: &mut Context<Self>) -> Task<Option<SiteInfo>> {
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            let mut site = site;
            let mut after: Option<SettleStage> = None;
            while let Some(stage) = next_stage(&site, after.as_ref()) {
                let jobs: Vec<Task<JobPhase>> =
                    this.update(cx, |s, cx| s.stage_jobs(&site.name, &stage, cx)).ok()?;
                for job in jobs {
                    if !matches!(job.await, JobPhase::Succeeded(_)) {
                        return None;
                    }
                }
                let sites = call(&*daemon, api::SITES_LIST, ()).await.ok()?;
                site = sites.into_iter().find(|s| s.name == site.name)?;
                after = Some(stage);
            }
            Some(site)
        })
    }

    fn stage_jobs(&mut self, site: &str, stage: &SettleStage, cx: &mut Context<Self>) -> Vec<Task<JobPhase>> {
        let id = |what: &str| ActionId(format!("site:{site}:settle:{what}"));
        match stage {
            SettleStage::Php(versions) => versions
                .iter()
                .map(|v| {
                    self.run_job(
                        id(v),
                        api::PHP_INSTALL,
                        PhpParams { php: v.clone() },
                        format!("Install PHP {v}"),
                        cx,
                    )
                })
                .collect(),
            SettleStage::Extensions(exts) => exts
                .iter()
                .map(|(name, php)| {
                    let params = ExtAddParams { name: name.clone(), php: Some(php.clone()) };
                    self.run_job(
                        id(name),
                        api::EXT_ADD,
                        params,
                        format!("Add the {name} extension to PHP {php}"),
                        cx,
                    )
                })
                .collect(),
            SettleStage::Services(services) => services
                .iter()
                .map(|(engine, version)| {
                    let title = match version {
                        Some(v) => format!("Add {} {v}", yard_proto::engine_display(engine)),
                        None => format!("Add {}", yard_proto::engine_display(engine)),
                    };
                    let params = ServiceAddParams {
                        engine: engine.clone(),
                        version: version.clone(),
                        port: None,
                        path: None,
                    };
                    self.run_job(id(engine), api::SERVICES_ADD, params, title, cx)
                })
                .collect(),
            SettleStage::Node(spec) => {
                let params = NodeInstallParams { spec: Some(spec.clone()), path: None };
                vec![self.run_job(id("node"), api::NODE_INSTALL, params, format!("Install Node {spec}"), cx)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::testing::{FakeDaemon, connected, finished, job, site, status};
    use gpui::TestAppContext;
    use serde_json::{Value, json};
    use std::time::Duration;
    use yard_app_model::jobs::JobPhase;
    use yard_proto::methods::PhpParams;
    use yard_proto::{ErrorCode, YardError};

    fn setup(cx: &mut TestAppContext) -> (Arc<FakeDaemon>, Entity<ActionRunner>) {
        let fake = FakeDaemon::new();
        fake.answer("status", status());
        let state = cx.update(|cx| AppState::new(fake.clone(), Duration::from_millis(50), cx));
        fake.push(connected(1));
        cx.run_until_parked();
        let runner = cx.update(|cx| ActionRunner::new(fake.clone(), state, cx));
        (fake, runner)
    }

    fn install(runner: &Entity<ActionRunner>, cx: &mut TestAppContext, v: &str) -> gpui::Task<JobPhase> {
        runner.update(cx, |r, cx| {
            r.run_job(
                ActionId(format!("php:{v}:install")),
                api::PHP_INSTALL,
                PhpParams { php: v.into() },
                format!("Install PHP {v}"),
                cx,
            )
        })
    }

    #[gpui::test]
    async fn finished_before_the_answer_returns(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        fake.hold("php.install");
        let task = install(&runner, cx, "8.3");
        cx.run_until_parked();
        let refused = YardError::new(ErrorCode::FormulaMissing, "no such formula");
        fake.event(finished("php-install-1", Some(refused.clone())));
        cx.run_until_parked();
        fake.release("php.install", Ok(job("php-install-1")));
        assert_eq!(task.await, JobPhase::Failed(refused));
    }

    /// The design's Lost: the connection dropped while the job ran. Waiting for a new connection
    /// left it running forever while yardd stayed down (found in the final review).
    #[gpui::test]
    async fn a_job_running_when_yardd_goes_is_lost(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        fake.answer("php.install", job("php-install-1"));
        let task = install(&runner, cx, "8.3");
        cx.run_until_parked();
        fake.push(yard_client::session::Update::State(yard_client::session::ConnectionState::Disconnected(
            YardError::new(ErrorCode::Internal, "yardd closed the connection"),
        )));
        cx.run_until_parked();
        assert_eq!(task.await, JobPhase::Lost);
    }

    /// A job whose answer arrives after the connection dropped is Lost too: claimed under the
    /// dead connection after the sweep, it stayed running until yardd came back (M6 final review).
    #[gpui::test]
    async fn a_job_answered_after_yardd_went_is_lost(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        fake.hold("php.install");
        let task = install(&runner, cx, "8.3");
        cx.run_until_parked();
        fake.push(yard_client::session::Update::State(yard_client::session::ConnectionState::Disconnected(
            YardError::new(ErrorCode::Internal, "yardd closed the connection"),
        )));
        cx.run_until_parked();
        fake.release("php.install", Ok(job("php-install-1")));
        cx.run_until_parked();
        assert_eq!(task.await, JobPhase::Lost);
    }

    #[gpui::test]
    async fn cancel_of_a_finished_job_is_ignored(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        fake.fail("jobs.cancel", YardError::new(ErrorCode::JobNotFound, "php-install-1 is not running"));
        runner.update(cx, |r, cx| r.cancel_job("php-install-1".into(), cx)).await;
        assert!(
            runner.read_with(cx, |r, _| r.errors.get(&ActionId("job:php-install-1:cancel".into())).is_none())
        );
    }

    #[gpui::test]
    async fn untrusted_formula_asks_then_retries(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        let untrusted = YardError::new(ErrorCode::FormulaUntrusted, "not trusted")
            .with_data(json!({"formula": "shivammathur/php/php@8.3", "tap": "shivammathur/php"}));
        fake.answers("php.install", vec![Err(untrusted), Ok(job("php-install-2"))]);
        fake.answer("sources.trust", job("source-trust-1"));
        // The install waits for the answer, and ends with the install that ran after it.
        let first = install(&runner, cx, "8.3");
        cx.run_until_parked();
        let prompt = runner.read_with(cx, |r, _| r.prompt()).expect("the trust question");
        assert!(matches!(&prompt, Prompt::Trust { formula, .. } if formula == "shivammathur/php/php@8.3"));
        runner.update(cx, |r, cx| r.answer(0, cx));
        cx.run_until_parked();
        fake.event(finished("source-trust-1", None));
        cx.run_until_parked();
        fake.event(finished("php-install-2", None));
        cx.run_until_parked();
        let names: Vec<String> = fake.calls().into_iter().map(|(n, _)| n).filter(|n| n != "status").collect();
        assert_eq!(names, ["php.install", "sources.trust", "php.install"]);
        assert!(runner.read_with(cx, |r, _| r.prompt().is_none()));
        assert!(matches!(first.await, JobPhase::Succeeded(_)));
    }

    fn untrusted(v: &str) -> YardError {
        YardError::new(ErrorCode::FormulaUntrusted, "not trusted")
            .with_data(json!({"formula": format!("shivammathur/php/php@{v}"), "tap": "shivammathur/php"}))
    }

    fn asked(runner: &Entity<ActionRunner>, cx: &mut TestAppContext) -> Option<String> {
        runner.read_with(cx, |r, _| match r.prompt() {
            Some(Prompt::Trust { formula, .. }) => Some(formula),
            _ => None,
        })
    }

    /// Two installs refused for trust are both asked, one after the other, and each yes repeats
    /// its own install (M6 final review: the second question replaced the first, and answering
    /// the first repeated the second's install).
    #[gpui::test]
    async fn each_trust_question_is_asked_and_repeats_its_own_work(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        fake.answers(
            "php.install",
            vec![Err(untrusted("8.3")), Err(untrusted("8.2")), Ok(job("php-install-3"))],
        );
        fake.answer("sources.trust", job("source-trust-1"));
        let _first = install(&runner, cx, "8.3");
        cx.run_until_parked();
        let _second = install(&runner, cx, "8.2");
        cx.run_until_parked();
        assert_eq!(asked(&runner, cx).as_deref(), Some("shivammathur/php/php@8.3"));
        assert!(runner.read_with(cx, |r, _| r.prompt()).is_some());
        runner.update(cx, |r, cx| r.answer(0, cx));
        cx.run_until_parked();
        assert_eq!(
            asked(&runner, cx).as_deref(),
            Some("shivammathur/php/php@8.2"),
            "the second waits its turn"
        );
        fake.event(finished("source-trust-1", None));
        cx.run_until_parked();
        let installs: Vec<Value> = fake
            .calls()
            .into_iter()
            .filter(|(n, _)| n == "php.install")
            .map(|(_, p)| p["php"].clone())
            .collect();
        assert_eq!(installs, [json!("8.3"), json!("8.2"), json!("8.3")]);
    }

    /// A settle stopped by brew's trust carries on once the formula is trusted.
    #[gpui::test]
    async fn a_settle_waits_for_the_trust_question_and_carries_on(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        let needs =
            json!([{"kind": "php", "php": "7.3"}, {"kind": "extension", "name": "redis", "php": "7.3"}]);
        let legacy: yard_proto::SiteInfo = serde_json::from_value(site("legacy", needs)).unwrap();
        fake.answers("php.install", vec![Err(untrusted("7.3")), Ok(job("php-install-2"))]);
        fake.answer("sources.trust", job("source-trust-1"));
        fake.answer("ext.add", job("ext-add-1"));
        let after_php = site("legacy", json!([{"kind": "extension", "name": "redis", "php": "7.3"}]));
        fake.answers("sites.list", vec![Ok(json!([after_php])), Ok(json!([site("legacy", json!([]))]))]);
        let task = runner.update(cx, |r, cx| r.settle(legacy, cx));
        cx.run_until_parked();
        assert!(runner.read_with(cx, |r, _| r.prompt()).is_some(), "the trust question");
        runner.update(cx, |r, cx| r.answer(0, cx));
        cx.run_until_parked();
        fake.event(finished("source-trust-1", None));
        cx.run_until_parked();
        fake.event(finished("php-install-2", None));
        cx.run_until_parked();
        assert_eq!(fake.count("ext.add"), 1, "the settle went on to the extension");
        fake.event(finished("ext-add-1", None));
        cx.run_until_parked();
        assert!(task.await.is_some());
    }

    #[gpui::test]
    async fn no_to_trust_runs_nothing(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        let untrusted = YardError::new(ErrorCode::FormulaUntrusted, "not trusted")
            .with_data(json!({"formula": "shivammathur/php/php@8.3", "tap": "shivammathur/php"}));
        fake.fail("php.install", untrusted);
        let task = install(&runner, cx, "8.3");
        cx.run_until_parked();
        assert!(runner.read_with(cx, |r, _| r.prompt()).is_some());
        runner.update(cx, |r, cx| r.answer(1, cx));
        let phase = task.await;
        assert!(
            matches!(phase, JobPhase::Failed(ref e) if e.code == ErrorCode::FormulaUntrusted),
            "{phase:?}"
        );
        assert_eq!(fake.count("php.install"), 1);
        assert_eq!(fake.count("sources.trust"), 0);
        assert!(runner.read_with(cx, |r, _| r.errors.get(&ActionId("php:8.3:install".into())).is_some()));
    }

    #[gpui::test]
    async fn jobs_are_refused_without_events(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        fake.without_events();
        let phase = install(&runner, cx, "8.3").await;
        let JobPhase::Failed(e) = phase else { panic!("{phase:?}") };
        assert_eq!(e.message, "cannot follow the job's progress without yardd's events");
        assert_eq!(e.hint.as_deref(), Some("yard start"));
        assert_eq!(fake.count("php.install"), 0);
    }

    #[gpui::test]
    async fn link_sends_the_canonical_path(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("via")).unwrap();
        fake.answer("sites.link", site("blog", json!([])));
        let linked = runner.update(cx, |r, cx| r.link(dir.path().join("via"), "blog".into(), cx)).await;
        assert!(linked.is_some());
        let (_, params) = fake.calls().into_iter().find(|(n, _)| n == "sites.link").unwrap();
        let real = std::fs::canonicalize(dir.path().join("real")).unwrap();
        assert_eq!(params, json!({"name": "blog", "path": real.display().to_string()}));
    }

    #[gpui::test]
    async fn settle_stops_at_the_first_failure(cx: &mut TestAppContext) {
        let (fake, runner) = setup(cx);
        let needs =
            json!([{"kind": "php", "php": "7.3"}, {"kind": "extension", "name": "redis", "php": "7.3"}]);
        let legacy: yard_proto::SiteInfo = serde_json::from_value(site("legacy", needs)).unwrap();
        fake.answer("php.install", job("php-install-1"));
        let task = runner.update(cx, |r, cx| r.settle(legacy, cx));
        cx.run_until_parked();
        fake.event(finished("php-install-1", Some(YardError::new(ErrorCode::JobFailed, "brew failed"))));
        cx.run_until_parked();
        assert_eq!(task.await, None);
        assert_eq!(fake.count("ext.add"), 0);
    }
}
