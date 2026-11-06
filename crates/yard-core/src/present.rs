//! How the CLI and Yard.app word a service's state, its sleep setting and a missing Node.

use crate::services::sleep::SleepAfter;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use yard_proto::{EnvBlock, ServiceInfo, ServiceManager, ServiceState, SiteNode};

/// `yard node install 22`, quoted where the shell would read the spec.
pub use crate::node::install_hint;

/// The state's own name: `running`, `starting`, `stopped`, `failed`, or `unknown`.
pub fn state_name(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "running",
        ServiceState::Starting => "starting",
        ServiceState::Stopped => "stopped",
        ServiceState::Failed { .. } => "failed",
        ServiceState::Other => "unknown",
    }
}

/// The state as `yard services`, `yard info` and Yard.app show it: `sleeping 2h` for an instance
/// that is asleep (M4e spec §9), `waking` while it starts again, else [`state_name`].
pub fn state_text(info: &ServiceInfo, now: OffsetDateTime) -> String {
    let since = info.sleeping_since.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
    match (&info.state, since) {
        (ServiceState::Stopped, Some(since)) => format!("sleeping {}", age(now - since)),
        (ServiceState::Starting, Some(_)) => "waking".into(),
        _ => state_name(&info.state).into(),
    }
}

/// `45s`, `12m`, `2h` or `3d`: the largest whole unit.
fn age(d: time::Duration) -> String {
    let secs = d.whole_seconds().max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The effective `sleep_after`: `15m`, `1h`, or `off`; `-` for an instance Yard does not run.
pub fn sleep_text(info: &ServiceInfo) -> String {
    if info.managed_by != ServiceManager::Yard {
        // Yard never puts something it does not run to sleep.
        return "-".to_string();
    }
    info.sleep_after_secs.map_or_else(|| "off".to_string(), |secs| SleepAfter::After(secs).to_string())
}

/// The Node spec a site asks for that nothing installed matches.
pub fn missing_node(node: Option<&SiteNode>) -> Option<&str> {
    node.filter(|n| n.error.is_none() && n.version.is_none()).and_then(|n| n.spec.as_deref())
}

/// Takes emailulator's note out of its block when the block still has lines (emailulator is
/// caught but not answering): it is a warning for the reader, not a line to paste. Returns the
/// blocks to paste and the notes taken out.
pub fn split_env_warnings(blocks: Vec<EnvBlock>) -> (Vec<EnvBlock>, Vec<String>) {
    let mut warnings = vec![];
    let blocks = blocks
        .into_iter()
        .map(|mut block| {
            if block.instance == crate::emailulator::NAME
                && !block.lines.is_empty()
                && let Some(note) = block.note.take()
            {
                warnings.push(note);
            }
            block
        })
        .collect();
    (blocks, warnings)
}

/// The `.env` text `yard env` prints and Yard.app copies: `# <instance>`, a `# note` when there
/// is one, then the lines; blocks are separated by a blank line. A block with a note and no lines
/// is that one comment line: the note already names the instance and the command that fixes it.
pub fn env_text(blocks: &[EnvBlock]) -> String {
    let rendered: Vec<String> = blocks
        .iter()
        .map(|block| match (&block.note, block.lines.is_empty()) {
            (Some(note), true) => format!("# {note}\n"),
            (note, _) => {
                let mut out = format!("# {}\n", block.instance);
                if let Some(note) = note {
                    out.push_str(&format!("# {note}\n"));
                }
                for line in &block.lines {
                    out.push_str(line);
                    out.push('\n');
                }
                out
            }
        })
        .collect();
    rendered.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;
    use yard_proto::{Account, ServiceInfo, ServiceManager, ServiceState};

    fn service(state: ServiceState, sleeping_since: Option<&str>) -> ServiceInfo {
        ServiceInfo {
            name: "redis".into(),
            engine: "redis".into(),
            version: None,
            formula: "redis".into(),
            ports: vec![],
            state,
            stopped_by_user: false,
            account: Account { user: None, password: None },
            console: None,
            used_by: vec![],
            sleeping_since: sleeping_since.map(str::to_string),
            sleep_after_secs: Some(900),
            sleep_after_source: Default::default(),
            managed_by: ServiceManager::Yard,
        }
    }

    #[test]
    fn sleeping_age_uses_the_largest_whole_unit() {
        let now = datetime!(2026-11-06 12:00:00 UTC);
        let at = |since: &str| state_text(&service(ServiceState::Stopped, Some(since)), now);
        assert_eq!(at("2026-11-06T11:59:15Z"), "sleeping 45s");
        assert_eq!(at("2026-11-06T11:48:00Z"), "sleeping 12m");
        assert_eq!(at("2026-11-06T10:00:00Z"), "sleeping 2h");
        assert_eq!(at("2026-11-03T12:00:00Z"), "sleeping 3d");
        assert_eq!(state_text(&service(ServiceState::Stopped, None), now), "stopped");
    }

    #[test]
    fn waking_is_starting_while_sleeping() {
        let now = datetime!(2026-11-06 12:00:00 UTC);
        let s = service(ServiceState::Starting, Some("2026-11-06T11:00:00Z"));
        assert_eq!(state_text(&s, now), "waking");
        assert_eq!(state_text(&service(ServiceState::Starting, None), now), "starting");
        assert_eq!(sleep_text(&s), "15m");
    }
}
