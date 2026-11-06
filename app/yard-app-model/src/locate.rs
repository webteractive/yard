//! Choosing the data root and the `yard` to run (M6 design §5.8); reading the plist and checking
//! the executable bit happen in yard-app.

use std::collections::HashSet;
use std::path::PathBuf;
pub use yard_core::render::launchd::AgentFacts;

/// What the app knows when it starts; yard-app reads the plist and checks which candidates are
/// executable (`executables`).
#[derive(Debug, Clone)]
pub struct LocateInput {
    pub root_arg: Option<PathBuf>,
    pub cli_setting: Option<PathBuf>,
    pub agent: Option<AgentFacts>,
    pub home: PathBuf,
    /// `Contents/Helpers` inside the app bundle, when it has one (M7).
    pub bundle_helpers: Option<PathBuf>,
    pub executables: HashSet<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub root: PathBuf,
    /// The root is `~/.yard`, so the CLI needs no `--root`.
    pub root_is_default: bool,
    pub yard: Option<PathBuf>,
    /// A LaunchAgent serves this root.
    pub agent_installed: bool,
}

/// Design §5.8's order: the root from `--root`, the agent's `YARD_ROOT`, else `~/.yard`; `yard`
/// from the setting, beside the agent's yardd, then the bundle, the first that is executable.
pub fn locate(input: &LocateInput) -> Location {
    let default = input.home.join(".yard");
    let root = input
        .root_arg
        .clone()
        .or_else(|| input.agent.as_ref().and_then(|a| a.root.clone()))
        .unwrap_or_else(|| default.clone());
    let beside_agent = input.agent.as_ref().and_then(|a| a.program.parent().map(|d| d.join("yard")));
    let in_bundle = input.bundle_helpers.as_ref().map(|d| d.join("yard"));
    let yard = [input.cli_setting.clone(), beside_agent, in_bundle]
        .into_iter()
        .flatten()
        .find(|p| input.executables.contains(p));
    // The agent has one label and serves one root (its `YARD_ROOT`, else `~/.yard`); for any other
    // root there is no agent, and `yard start` refuses to take that one.
    let agent_installed =
        input.agent.as_ref().is_some_and(|a| a.root.clone().unwrap_or_else(|| default.clone()) == root);
    Location { root_is_default: root == default, root, yard, agent_installed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn input() -> LocateInput {
        LocateInput {
            root_arg: None,
            cli_setting: None,
            agent: None,
            home: PathBuf::from("/Users/me"),
            bundle_helpers: None,
            executables: HashSet::new(),
        }
    }

    fn agent(program: &str, root: Option<&str>) -> AgentFacts {
        AgentFacts { program: PathBuf::from(program), root: root.map(PathBuf::from) }
    }

    #[test]
    fn root_argument_wins() {
        let l = locate(&LocateInput {
            root_arg: Some("/tmp/yard-dev".into()),
            agent: Some(agent("/x/yardd", Some("/elsewhere"))),
            ..input()
        });
        assert_eq!((l.root.as_path(), l.root_is_default), (Path::new("/tmp/yard-dev"), false));
        // The agent serves /elsewhere: for this root there is none, so Start yardd (which `yard
        // start` would refuse) is not offered, and onboarding counts the agent step as to do.
        assert!(!l.agent_installed);
    }

    #[test]
    fn an_agent_counts_only_for_the_root_it_serves() {
        let home_agent = || Some(agent("/x/yardd", None));
        let mine =
            locate(&LocateInput { root_arg: Some("/Users/me/.yard".into()), agent: home_agent(), ..input() });
        assert!(mine.agent_installed);
        let scratch =
            locate(&LocateInput { root_arg: Some("/tmp/yard-check".into()), agent: home_agent(), ..input() });
        assert!(!scratch.agent_installed);
    }

    #[test]
    fn agent_environment_root_is_used() {
        let l = locate(&LocateInput { agent: Some(agent("/x/yardd", Some("/data/yard"))), ..input() });
        assert_eq!(l.root, PathBuf::from("/data/yard"));
        assert!(l.agent_installed);
    }

    #[test]
    fn default_root_is_dot_yard() {
        let l = locate(&input());
        assert_eq!(
            (l.root.as_path(), l.root_is_default, l.agent_installed),
            (Path::new("/Users/me/.yard"), true, false)
        );
    }

    #[test]
    fn yard_is_beside_the_agents_yardd() {
        let mut i = LocateInput { agent: Some(agent("/repo/target/release/yardd", None)), ..input() };
        i.executables.insert("/repo/target/release/yard".into());
        assert_eq!(locate(&i).yard, Some(PathBuf::from("/repo/target/release/yard")));
    }

    #[test]
    fn setting_beats_the_agent() {
        let mut i = LocateInput {
            agent: Some(agent("/repo/target/release/yardd", None)),
            cli_setting: Some("/opt/bin/yard".into()),
            ..input()
        };
        i.executables.extend(["/repo/target/release/yard".into(), "/opt/bin/yard".into()]);
        assert_eq!(locate(&i).yard, Some(PathBuf::from("/opt/bin/yard")));
    }

    #[test]
    fn bundle_helpers_come_last() {
        let mut i =
            LocateInput { bundle_helpers: Some("/Applications/Yard.app/Contents/Helpers".into()), ..input() };
        i.executables.insert("/Applications/Yard.app/Contents/Helpers/yard".into());
        assert_eq!(locate(&i).yard, Some(PathBuf::from("/Applications/Yard.app/Contents/Helpers/yard")));
    }

    #[test]
    fn no_yard_is_none() {
        let i = LocateInput {
            agent: Some(agent("/gone/yardd", None)),
            cli_setting: Some("/nope/yard".into()),
            ..input()
        };
        assert_eq!(locate(&i).yard, None);
    }
}
