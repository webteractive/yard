//! Job records (M6 design §9.1): one per `(epoch, job_id)`, from the events yardd sends and the
//! answers to the calls that started them.

use serde_json::Value;
use std::collections::VecDeque;
use time::OffsetDateTime;
use yard_proto::{ErrorCode, Event, YardError};

const MAX_LINES: usize = 2000;
const MAX_FINISHED: usize = 50;

/// A job id is only unique within one connection's epoch: yardd numbers jobs from 1 again after
/// a restart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobKey {
    pub epoch: u64,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobPhase {
    /// Queued behind another job: its only line is `waiting for <job id>`; `behind` is that job's
    /// title.
    Waiting {
        behind: String,
    },
    Running,
    Succeeded(Option<Value>),
    Failed(YardError),
    Cancelled,
    /// The connection dropped while it ran: its result is unknown.
    Lost,
}

impl JobPhase {
    pub fn is_finished(&self) -> bool {
        !matches!(self, JobPhase::Waiting { .. } | JobPhase::Running)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub key: JobKey,
    pub title: String,
    pub started_by_app: bool,
    pub phase: JobPhase,
    pub lines: VecDeque<String>,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
}

/// A title for a job the app did not start, from its id's kind (`php-install-3`).
pub fn title_elsewhere(job_id: &str) -> String {
    let kind = job_id.rsplit_once('-').filter(|(_, n)| n.parse::<u64>().is_ok()).map_or(job_id, |(k, _)| k);
    let title = match kind {
        "php-install" => "Install PHP",
        "php-uninstall" => "Remove PHP",
        "ext-add" => "Add an extension",
        "ext-remove" => "Remove an extension",
        "service-add" => "Add a service",
        "node-install" => "Install Node",
        "node-uninstall" => "Remove Node",
        "tunnel-add" => "Share a site",
        "tunnel-remove" => "Stop sharing a site",
        "tunnel-login" => "Sign in to Cloudflare",
        "source-trust" => "Trust a formula",
        "source-untrust" => "Untrust a formula",
        _ => return job_id.to_string(),
    };
    format!("{title} (started elsewhere)")
}

/// Every running job and the last 50 finished ones, oldest first.
#[derive(Debug, Default)]
pub struct JobsStore {
    records: Vec<JobRecord>,
}

impl JobsStore {
    /// Takes in a job event that arrived on connection `epoch`.
    pub fn apply(&mut self, event: &Event, epoch: u64, now: OffsetDateTime) {
        match event {
            Event::JobProgress { job_id, line } => {
                let behind = line.strip_prefix("waiting for ").map(|id| self.title_of(id, epoch));
                let r = self.record(job_id, epoch, now);
                if r.phase.is_finished() {
                    return;
                }
                r.phase = match (r.lines.is_empty(), behind) {
                    (true, Some(behind)) => JobPhase::Waiting { behind },
                    _ => JobPhase::Running,
                };
                r.lines.push_back(line.clone());
                if r.lines.len() > MAX_LINES {
                    r.lines.pop_front();
                }
            }
            Event::JobFinished { job_id, error, result } => {
                let r = self.record(job_id, epoch, now);
                r.phase = match error {
                    Some(e) if e.code == ErrorCode::JobCancelled => JobPhase::Cancelled,
                    Some(e) => JobPhase::Failed(e.clone()),
                    None => JobPhase::Succeeded(result.clone()),
                };
                r.finished_at = Some(now);
                self.trim();
            }
            _ => {}
        }
    }

    /// A new connection: whatever ran on the old one has an unknown end.
    pub fn epoch_changed(&mut self, to: u64, now: OffsetDateTime) {
        for r in self.records.iter_mut().filter(|r| r.key.epoch < to && !r.phase.is_finished()) {
            r.phase = JobPhase::Lost;
            r.finished_at = Some(now);
        }
        self.trim();
    }

    /// The app started `job_id`: it gets the action's title, whether its events came first or not.
    pub fn claim(&mut self, job_id: &str, epoch: u64, title: String, now: OffsetDateTime) {
        let r = self.record(job_id, epoch, now);
        r.title = title;
        r.started_by_app = true;
    }

    /// What Activity calls `job_id`: its record's title, or the title for a job started elsewhere.
    fn title_of(&self, job_id: &str, epoch: u64) -> String {
        self.records
            .iter()
            .find(|r| r.key.epoch == epoch && r.key.id == job_id)
            .map_or_else(|| title_elsewhere(job_id), |r| r.title.clone())
    }

    pub fn phase(&self, key: &JobKey) -> Option<&JobPhase> {
        self.records.iter().find(|r| &r.key == key).map(|r| &r.phase)
    }

    pub fn running(&self) -> impl Iterator<Item = &JobRecord> {
        self.records.iter().filter(|r| !r.phase.is_finished())
    }

    pub fn jobs(&self) -> &[JobRecord] {
        &self.records
    }

    fn record(&mut self, job_id: &str, epoch: u64, now: OffsetDateTime) -> &mut JobRecord {
        let key = JobKey { epoch, id: job_id.to_string() };
        let at = match self.records.iter().position(|r| r.key == key) {
            Some(i) => i,
            None => {
                self.records.push(JobRecord {
                    title: title_elsewhere(job_id),
                    key,
                    started_by_app: false,
                    phase: JobPhase::Running,
                    lines: VecDeque::new(),
                    started_at: now,
                    finished_at: None,
                });
                self.records.len() - 1
            }
        };
        &mut self.records[at]
    }

    /// Drops the oldest finished records past 50; a running one is never dropped.
    fn trim(&mut self) {
        let mut finished = self.records.iter().filter(|r| r.phase.is_finished()).count();
        self.records.retain(|r| {
            if finished > MAX_FINISHED && r.phase.is_finished() {
                finished -= 1;
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;
    use yard_proto::{ErrorCode, Event, YardError};

    const NOW: OffsetDateTime = datetime!(2026-11-06 12:00 UTC);

    fn line(id: &str, line: &str) -> Event {
        Event::JobProgress { job_id: id.into(), line: line.into() }
    }

    fn done(id: &str, error: Option<YardError>) -> Event {
        Event::JobFinished { job_id: id.into(), error, result: None }
    }

    fn key(epoch: u64, id: &str) -> JobKey {
        JobKey { epoch, id: id.into() }
    }

    #[test]
    fn events_before_the_answer_make_one_record() {
        let mut s = JobsStore::default();
        s.apply(&line("php-install-1", "==> Downloading"), 1, NOW);
        s.claim("php-install-1", 1, "Install PHP 8.3".into(), NOW);
        assert_eq!(s.jobs().len(), 1);
        let r = &s.jobs()[0];
        assert_eq!((r.title.as_str(), r.started_by_app), ("Install PHP 8.3", true));
        assert_eq!(r.lines, ["==> Downloading"]);
        s.apply(&done("php-install-1", None), 1, NOW);
        assert_eq!(s.phase(&key(1, "php-install-1")), Some(&JobPhase::Succeeded(None)));
    }

    /// The job it waits for is named as Activity names that job, never by its id (found by
    /// checklist-installs.sh: Activity said `waiting for php-install-1`).
    #[test]
    fn waiting_names_the_job_it_waits_for() {
        let mut s = JobsStore::default();
        s.claim("php-install-1", 1, "Install PHP 7.1".into(), NOW);
        s.claim("php-install-2", 1, "Install PHP 7.2".into(), NOW);
        s.apply(&line("php-install-2", "waiting for php-install-1"), 1, NOW);
        assert_eq!(
            s.phase(&key(1, "php-install-2")),
            Some(&JobPhase::Waiting { behind: "Install PHP 7.1".into() })
        );
        s.apply(&line("ext-add-4", "waiting for php-install-3"), 1, NOW);
        assert_eq!(
            s.phase(&key(1, "ext-add-4")),
            Some(&JobPhase::Waiting { behind: "Install PHP (started elsewhere)".into() })
        );
    }

    #[test]
    fn waiting_for_is_a_phase() {
        let mut s = JobsStore::default();
        s.claim("php-install-2", 1, "Install PHP 8.2".into(), NOW);
        s.apply(&line("php-install-2", "waiting for php-install-1"), 1, NOW);
        assert_eq!(
            s.phase(&key(1, "php-install-2")),
            Some(&JobPhase::Waiting { behind: "Install PHP (started elsewhere)".into() })
        );
        s.apply(&line("php-install-2", "==> Downloading"), 1, NOW);
        assert_eq!(s.phase(&key(1, "php-install-2")), Some(&JobPhase::Running));
        s.apply(&done("php-install-2", Some(YardError::new(ErrorCode::JobCancelled, "cancelled"))), 1, NOW);
        assert_eq!(s.phase(&key(1, "php-install-2")), Some(&JobPhase::Cancelled));
    }

    #[test]
    fn a_reused_id_after_reconnect_is_a_new_job() {
        let mut s = JobsStore::default();
        s.claim("php-install-1", 1, "Install PHP 8.3".into(), NOW);
        s.epoch_changed(2, NOW);
        assert_eq!(s.phase(&key(1, "php-install-1")), Some(&JobPhase::Lost));
        s.apply(&line("php-install-1", "==> Pouring"), 2, NOW);
        assert_eq!(s.jobs().len(), 2);
        assert_eq!(s.phase(&key(2, "php-install-1")), Some(&JobPhase::Running));
        assert_eq!(s.phase(&key(1, "php-install-1")), Some(&JobPhase::Lost), "the old record keeps its end");
        assert_eq!(s.jobs()[1].title, "Install PHP (started elsewhere)");
        assert_eq!(s.running().count(), 1);
    }

    #[test]
    fn finished_records_are_capped_at_50() {
        let mut s = JobsStore::default();
        s.claim("node-install-0", 1, "Install Node 22".into(), NOW);
        for i in 1..=60 {
            s.apply(&done(&format!("ext-add-{i}"), None), 1, NOW);
        }
        assert_eq!(s.jobs().iter().filter(|r| r.finished_at.is_some()).count(), 50);
        assert!(s.phase(&key(1, "ext-add-10")).is_none(), "the oldest finished go first");
        assert!(s.phase(&key(1, "ext-add-60")).is_some());
        assert_eq!(s.running().count(), 1, "a running job is never dropped");
    }

    #[test]
    fn lines_keep_the_last_2000() {
        let mut s = JobsStore::default();
        for i in 0..2100 {
            s.apply(&line("php-install-1", &i.to_string()), 1, NOW);
        }
        let lines = &s.jobs()[0].lines;
        assert_eq!((lines.len(), lines.front().map(String::as_str)), (2000, Some("100")));
    }

    #[test]
    fn titles_for_jobs_started_elsewhere() {
        assert_eq!(title_elsewhere("php-install-3"), "Install PHP (started elsewhere)");
        assert_eq!(title_elsewhere("tunnel-login-1"), "Sign in to Cloudflare (started elsewhere)");
        assert_eq!(title_elsewhere("source-untrust-2"), "Untrust a formula (started elsewhere)");
        assert_eq!(title_elsewhere("rebuild-9"), "rebuild-9");
    }
}
