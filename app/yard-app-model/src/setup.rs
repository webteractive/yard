//! Onboarding's checklist (M6 design §8.4), computed from facts each time it is shown.

use std::path::PathBuf;
use yard_proto::Profile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupFacts {
    pub brew: bool,
    pub yard: Option<PathBuf>,
    pub agent_installed: bool,
    pub daemon_answers: bool,
    pub helper_version: Option<String>,
    pub ca_trusted: Option<bool>,
    pub profile: Option<Profile>,
    pub site_count: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStepId {
    Brew,
    Cli,
    Daemon,
    Helper,
    Https,
    Shims,
    Ports,
    FirstSite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStep {
    pub id: SetupStepId,
    pub done: bool,
    /// An earlier step must be done first.
    pub blocked: bool,
    pub optional: bool,
}

/// The eight steps of design §8.4, in order. Without brew every later step is blocked; a step that
/// runs `yard` needs it located, and one that asks yardd needs yardd answering.
pub fn steps(f: &SetupFacts) -> Vec<SetupStep> {
    use SetupStepId::*;
    let has_yard = f.yard.is_some();
    let rows = [
        (Brew, f.brew, false, false),
        (Cli, has_yard, false, false),
        (Daemon, f.daemon_answers, !has_yard, false),
        (Helper, f.helper_version.is_some(), !has_yard, false),
        (Https, f.ca_trusted == Some(true), !f.daemon_answers, true),
        (Shims, false, !has_yard, true),
        (Ports, f.profile == Some(Profile::Privileged), !f.daemon_answers, true),
        (FirstSite, f.site_count.is_some_and(|n| n > 0), !f.daemon_answers, true),
    ];
    rows.into_iter()
        .map(|(id, done, blocked, optional)| SetupStep {
            id,
            done,
            blocked: id != Brew && (!f.brew || blocked),
            optional,
        })
        .collect()
}

/// Onboarding opens on first launch, or when there is neither an agent nor an answering yardd.
pub fn needs_onboarding(f: &SetupFacts, first_launch: bool) -> bool {
    first_launch || (!f.agent_installed && !f.daemon_answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> SetupFacts {
        SetupFacts {
            brew: true,
            yard: None,
            agent_installed: false,
            daemon_answers: false,
            helper_version: None,
            ca_trusted: None,
            profile: None,
            site_count: None,
        }
    }

    fn step(steps: &[SetupStep], id: SetupStepId) -> SetupStep {
        steps.iter().find(|s| s.id == id).cloned().unwrap()
    }

    #[test]
    fn setup_steps_from_facts() {
        let s = steps(&fresh());
        assert!(step(&s, SetupStepId::Brew).done);
        assert!(s.iter().filter(|x| x.id != SetupStepId::Brew).all(|x| !x.done));
        assert!(step(&s, SetupStepId::Daemon).blocked, "no yard, no start");
        assert!(needs_onboarding(&fresh(), false));

        let mut f = fresh();
        f.yard = Some("/r/yard".into());
        assert!(!step(&steps(&f), SetupStepId::Daemon).blocked);

        let no_brew = steps(&SetupFacts { brew: false, ..fresh() });
        assert!(no_brew.iter().filter(|x| x.id != SetupStepId::Brew).all(|x| x.blocked));

        let set_up = SetupFacts {
            brew: true,
            yard: Some("/r/yard".into()),
            agent_installed: true,
            daemon_answers: true,
            helper_version: Some("0.1.0".into()),
            ca_trusted: Some(false),
            profile: Some(Profile::Standard),
            site_count: Some(0),
        };
        let s = steps(&set_up);
        assert!(s.iter().filter(|x| !x.optional).all(|x| x.done), "{s:?}");
        assert!(!step(&s, SetupStepId::Shims).done, "shims are never checked");
        assert!(!needs_onboarding(&set_up, false));
        assert!(needs_onboarding(&set_up, true), "first launch always shows it");
    }
}
