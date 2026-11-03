//! Events pushed to subscribers after `subscribe`.

use crate::{Holder, SiteInfo, TunnelInfo, UnitState, YardError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Units,
    Sites,
    Logs,
    Ports,
    Jobs,
    Tunnels,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    UnitStateChanged {
        unit: String,
        state: UnitState,
    },
    /// Boxed: a site is much larger than every other event. The wire form is the same.
    SiteAdded {
        site: Box<SiteInfo>,
    },
    SiteRemoved {
        name: String,
    },
    LogLine {
        unit: String,
        line: String,
    },
    PortConflict {
        unit: String,
        port: u16,
        holder: Option<Holder>,
    },
    /// One line of a long operation's output (spec §6.2).
    JobProgress {
        job_id: String,
        line: String,
    },
    /// The job is over; `error` is why it failed. `result` is what a job that produces
    /// something made (a `TunnelInfo` from `tunnels.add`, protocol 2.10).
    JobFinished {
        job_id: String,
        error: Option<YardError>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
    /// A tunnel's state or URL changed. Boxed, like `SiteAdded`.
    TunnelChanged {
        tunnel: Box<TunnelInfo>,
    },
}

impl Event {
    pub fn topic(&self) -> Topic {
        match self {
            Event::UnitStateChanged { .. } => Topic::Units,
            Event::SiteAdded { .. } | Event::SiteRemoved { .. } => Topic::Sites,
            Event::LogLine { .. } => Topic::Logs,
            Event::PortConflict { .. } => Topic::Ports,
            Event::JobProgress { .. } | Event::JobFinished { .. } => Topic::Jobs,
            Event::TunnelChanged { .. } => Topic::Tunnels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_tags_and_topics() {
        let e = Event::SiteRemoved { name: "x".into() };
        assert_eq!(serde_json::to_value(&e).unwrap(), json!({"type":"site_removed","name":"x"}));
        assert_eq!(e.topic(), Topic::Sites);
        assert_eq!(Event::LogLine { unit: "nginx".into(), line: "l".into() }.topic(), Topic::Logs);
        assert_eq!(
            Event::PortConflict { unit: "redis".into(), port: 6379, holder: None }.topic(),
            Topic::Ports
        );
        assert_eq!(serde_json::to_value(Topic::Units).unwrap(), json!("units"));
        let done = Event::JobFinished { job_id: "php-install-1".into(), error: None, result: None };
        assert_eq!(done.topic(), Topic::Jobs);
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            json!({"type": "job_finished", "job_id": "php-install-1", "error": null})
        );
        assert_eq!(serde_json::to_value(Topic::Jobs).unwrap(), json!("jobs"));
    }

    #[test]
    fn job_finished_carries_a_result_and_reads_without_one() {
        let old: Event =
            serde_json::from_value(json!({"type":"job_finished","job_id":"j-1","error":null})).unwrap();
        assert!(matches!(old, Event::JobFinished { result: None, .. }));
        let done =
            Event::JobFinished { job_id: "j-1".into(), error: None, result: Some(json!({"site":"blog"})) };
        assert_eq!(serde_json::to_value(&done).unwrap()["result"]["site"], "blog");
        let changed = Event::TunnelChanged { tunnel: Box::new(crate::types::tests::sample_tunnel()) };
        assert_eq!(changed.topic(), Topic::Tunnels);
    }
}
