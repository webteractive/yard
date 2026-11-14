//! Dumps and imports Yard.app runs through the CLI (db spec §10): one record each, shown in
//! Activity beside yardd's jobs with the bytes moved so far, read from the CLI's
//! `--progress-json` lines (`{"bytes":N,"total":M}` on stderr).

use crate::jobs::JobPhase;
use serde::Deserialize;
use std::collections::VecDeque;
use time::OffsetDateTime;
use yard_proto::{ErrorCode, YardError};

const MAX_LINES: usize = 200;
const MAX_FINISHED: usize = 20;

/// Bytes moved so far, and the whole when the CLI knows it (a plain import file's size).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Progress {
    pub bytes: u64,
    pub total: Option<u64>,
}

/// A `--progress-json` line, or `None` for anything else the CLI writes to stderr (a note).
pub fn progress_line(line: &str) -> Option<Progress> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// `12.0 MB of 40.0 MB`, or `12.0 MB` with no total.
pub fn progress_text(p: Progress) -> String {
    match p.total {
        Some(total) => format!("{} of {}", crate::db::bytes_text(p.bytes), crate::db::bytes_text(total)),
        None => crate::db::bytes_text(p.bytes),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub id: u64,
    pub title: String,
    pub phase: JobPhase,
    pub progress: Option<Progress>,
    /// What the CLI wrote that is not progress (notes, the outcome), oldest first.
    pub lines: VecDeque<String>,
    pub started_at: OffsetDateTime,
}

/// Every running transfer and the last 20 finished ones, oldest first.
#[derive(Debug, Default)]
pub struct TransferStore {
    next: u64,
    records: Vec<Transfer>,
}

impl TransferStore {
    /// A new running transfer; its id.
    pub fn start(&mut self, title: String, now: OffsetDateTime) -> u64 {
        self.next += 1;
        self.records.push(Transfer {
            id: self.next,
            title,
            phase: JobPhase::Running,
            progress: None,
            lines: VecDeque::new(),
            started_at: now,
        });
        self.next
    }

    /// Takes in a line the CLI wrote to stderr; `true` when it was progress.
    pub fn line(&mut self, id: u64, line: &str) -> bool {
        let Some(r) = self.get_mut(id) else { return false };
        if r.phase.is_finished() {
            return false;
        }
        match progress_line(line) {
            Some(p) => {
                r.progress = Some(p);
                true
            }
            None => {
                if !line.trim().is_empty() {
                    r.lines.push_back(line.trim_end().to_string());
                    if r.lines.len() > MAX_LINES {
                        r.lines.pop_front();
                    }
                }
                false
            }
        }
    }

    /// The end: done with what to say about it, cancelled (`JOB_CANCELLED`), or failed.
    pub fn finish(&mut self, id: u64, result: Result<String, YardError>) {
        let Some(r) = self.get_mut(id) else { return };
        r.phase = match result {
            Ok(said) => {
                if !said.is_empty() {
                    r.lines.push_back(said);
                }
                JobPhase::Succeeded(None)
            }
            Err(e) if e.code == ErrorCode::JobCancelled => JobPhase::Cancelled,
            Err(e) => JobPhase::Failed(e),
        };
        self.trim();
    }

    pub fn get(&self, id: u64) -> Option<&Transfer> {
        self.records.iter().find(|r| r.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Transfer> {
        self.records.iter_mut().find(|r| r.id == id)
    }

    pub fn running(&self) -> impl Iterator<Item = &Transfer> {
        self.records.iter().filter(|r| !r.phase.is_finished())
    }

    pub fn all(&self) -> &[Transfer] {
        &self.records
    }

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

/// What quitting asks while transfers run, and its buttons (button 0 stops them and quits).
pub fn quit_question(running: &[&Transfer]) -> Option<(String, String, [&'static str; 2])> {
    let title = match running {
        [] => return None,
        [one] => format!("{} is still running. Stop it and quit?", one.title),
        many => format!("{} dumps and imports are still running. Stop them and quit?", many.len()),
    };
    let message =
        "A stopped dump leaves no file; a stopped import keeps what it imported so far.".to_string();
    Some((title, message, ["Stop and quit", "Cancel"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-15 12:00 UTC);

    #[test]
    fn progress_lines_are_read_and_notes_kept() {
        assert_eq!(
            progress_line(r#"{"bytes":10,"total":40}"#),
            Some(Progress { bytes: 10, total: Some(40) })
        );
        assert_eq!(progress_line(r#"{"bytes":10,"total":null}"#), Some(Progress { bytes: 10, total: None }));
        assert_eq!(progress_line("note: the dump has no CREATE DATABASE"), None);
        assert_eq!(progress_line("{not json"), None);

        let mut s = TransferStore::default();
        let id = s.start("Dump shop".into(), NOW);
        assert!(!s.line(id, "a note"));
        assert!(s.line(id, r#"{"bytes":2097152,"total":4194304}"#));
        let r = s.get(id).unwrap();
        assert_eq!(r.lines, ["a note"]);
        assert_eq!(progress_text(r.progress.unwrap()), "2.0 MB of 4.0 MB");
        assert_eq!(progress_text(Progress { bytes: 512, total: None }), "512 bytes");
    }

    #[test]
    fn a_transfer_ends_done_cancelled_or_failed() {
        let mut s = TransferStore::default();
        let (a, b, c) = (s.start("a".into(), NOW), s.start("b".into(), NOW), s.start("c".into(), NOW));
        assert_eq!(s.running().count(), 3);
        s.finish(a, Ok("Dumped shop (3.0 MB).".into()));
        s.finish(b, Err(YardError::new(ErrorCode::JobCancelled, "cancelled")));
        s.finish(c, Err(YardError::new(ErrorCode::DbToolFailed, "mysqldump failed")));
        assert_eq!(s.get(a).unwrap().phase, JobPhase::Succeeded(None));
        assert_eq!(s.get(a).unwrap().lines, ["Dumped shop (3.0 MB)."]);
        assert_eq!(s.get(b).unwrap().phase, JobPhase::Cancelled);
        assert!(matches!(&s.get(c).unwrap().phase, JobPhase::Failed(e) if e.message == "mysqldump failed"));
        assert!(!s.line(a, r#"{"bytes":1,"total":null}"#), "a finished transfer takes no more");
        assert_eq!(s.running().count(), 0);
    }

    #[test]
    fn finished_transfers_are_capped() {
        let mut s = TransferStore::default();
        let running = s.start("still going".into(), NOW);
        for i in 0..30 {
            let id = s.start(format!("t{i}"), NOW);
            s.finish(id, Ok(String::new()));
        }
        assert_eq!(s.all().len(), 21);
        assert!(s.get(running).is_some(), "a running one is never dropped");
    }

    #[test]
    fn quitting_asks_only_while_one_runs() {
        let mut s = TransferStore::default();
        assert_eq!(quit_question(&s.running().collect::<Vec<_>>()), None);
        s.start("Dump shop from mysql-8.4".into(), NOW);
        let (title, _, buttons) = quit_question(&s.running().collect::<Vec<_>>()).unwrap();
        assert_eq!(title, "Dump shop from mysql-8.4 is still running. Stop it and quit?");
        assert_eq!(buttons, ["Stop and quit", "Cancel"]);
        s.start("Import blog into postgres-17".into(), NOW);
        let (title, _, _) = quit_question(&s.running().collect::<Vec<_>>()).unwrap();
        assert_eq!(title, "2 dumps and imports are still running. Stop them and quit?");
    }
}
