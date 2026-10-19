//! Events pushed to subscribers after `subscribe`.

use crate::{Holder, SiteInfo, UnitState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Units,
    Sites,
    Logs,
    Ports,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    UnitStateChanged {
        unit: String,
        state: UnitState,
    },
    SiteAdded {
        site: SiteInfo,
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
}

impl Event {
    pub fn topic(&self) -> Topic {
        match self {
            Event::UnitStateChanged { .. } => Topic::Units,
            Event::SiteAdded { .. } | Event::SiteRemoved { .. } => Topic::Sites,
            Event::LogLine { .. } => Topic::Logs,
            Event::PortConflict { .. } => Topic::Ports,
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
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type":"site_removed","name":"x"})
        );
        assert_eq!(e.topic(), Topic::Sites);
        assert_eq!(
            Event::LogLine {
                unit: "nginx".into(),
                line: "l".into()
            }
            .topic(),
            Topic::Logs
        );
        assert_eq!(
            Event::PortConflict {
                unit: "redis".into(),
                port: 6379,
                holder: None
            }
            .topic(),
            Topic::Ports
        );
        assert_eq!(serde_json::to_value(Topic::Units).unwrap(), json!("units"));
    }
}
