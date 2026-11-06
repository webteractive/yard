//! Hints become actions (M6 design §10.2): a fixed table over the hint's shell words. Anything not
//! in the table, or with an argument that does not look like the API's, is only offered to copy.

use crate::shell_words::split_words;
use std::path::PathBuf;
pub use yard_proto::methods::{
    ExtAddParams, ExtParams, NodeInstallParams, PhpParams, ServiceAddParams, ServiceParams, SourceParams,
};

/// A screen of the main window, with what to select on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Sites(Option<String>),
    Services(Option<String>),
    Php,
    Node,
    Tunnels,
    Ports,
    Logs(Option<String>),
    Doctor,
    Sources,
    /// The jobs list; no hint leads here.
    Activity,
}

/// A hint that is one quick call.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcCall {
    ServicesStart(ServiceParams),
    ExtEnable(ExtParams),
    ExtDisable(ExtParams),
}

/// A hint that is a job, followed with its progress.
#[derive(Debug, Clone, PartialEq)]
pub enum JobCall {
    PhpInstall(PhpParams),
    ExtAdd(ExtAddParams),
    ExtRemove(ExtParams),
    NodeInstall(NodeInstallParams),
    ServiceAdd(ServiceAddParams),
    SourceTrust(SourceParams),
}

/// The only commands the app hands to Terminal; it builds their arguments itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCommand {
    /// `sudo <yard> [--root <root>] helper install`, with the located `yard`.
    HelperInstall,
    Brew {
        reinstall: bool,
        formula: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HintAction {
    Rpc(RpcCall),
    Job(JobCall),
    /// `yard <args> --json`, run by the app.
    Cli(Vec<String>),
    RestartDaemon,
    GoTo(Screen),
    EditPort {
        owner: String,
        is_service: bool,
    },
    OpenFile(PathBuf),
    Settle {
        site: Option<String>,
    },
    Terminal(TerminalCommand),
    Copy(String),
}

fn is_php(v: &str) -> bool {
    let mut parts = v.split('.');
    let digits = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// An instance or unit name: `redis`, `mysql-8.4`, `nginx-http`.
fn is_name(v: &str) -> bool {
    let mut bytes = v.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

/// A formula, extension or Node spec: one word with no shell syntax.
fn is_word(v: &str) -> bool {
    !v.is_empty()
        && !v.starts_with('-')
        && !v.chars().any(|c| c.is_whitespace() || ";&|<>$`'\"()".contains(c))
}

fn is_placeholder(v: &str) -> bool {
    v.starts_with('<') && v.ends_with('>')
}

impl HintAction {
    /// The table in design §10.2. A hint not in it, or with an argument the API would not take,
    /// is `Copy`.
    pub fn parse(hint: &str) -> HintAction {
        let copy = || HintAction::Copy(hint.to_string());
        if hint.trim() == "yard stop && yard start" {
            return HintAction::RestartDaemon;
        }
        let Some(words) = split_words(hint) else { return copy() };
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let owned = |s: &str| s.to_string();
        match w.as_slice() {
            ["yard", "start"] => HintAction::Cli(vec![owned("start")]),
            ["yard", "trust"] => HintAction::Cli(vec![owned("trust")]),
            ["yard", "php", "install", v] if is_php(v) => {
                HintAction::Job(JobCall::PhpInstall(PhpParams { php: owned(v) }))
            }
            ["yard", "ext", "add", x, "--php", v] if is_word(x) && is_php(v) => {
                HintAction::Job(JobCall::ExtAdd(ExtAddParams { name: owned(x), php: Some(owned(v)) }))
            }
            ["yard", "ext", verb @ ("enable" | "disable"), x, rest @ ..] if is_word(x) => {
                let php = match rest {
                    [] => None,
                    ["--php", v] if is_php(v) => Some(owned(v)),
                    _ => return copy(),
                };
                let p = ExtParams { name: owned(x), php };
                HintAction::Rpc(if *verb == "enable" {
                    RpcCall::ExtEnable(p)
                } else {
                    RpcCall::ExtDisable(p)
                })
            }
            ["yard", "ext", "remove", x, "--php", v] if is_word(x) && is_php(v) => {
                HintAction::Job(JobCall::ExtRemove(ExtParams { name: owned(x), php: Some(owned(v)) }))
            }
            ["yard", "node", "install", spec] if is_word(spec) => {
                HintAction::Job(JobCall::NodeInstall(NodeInstallParams {
                    spec: Some(owned(spec)),
                    path: None,
                }))
            }
            ["yard", "service", "add", engine, rest @ ..] if is_name(engine) => {
                let version = match rest {
                    [] => None,
                    ["--version", l] if is_word(l) => Some(owned(l)),
                    _ => return copy(),
                };
                HintAction::Job(JobCall::ServiceAdd(ServiceAddParams {
                    engine: owned(engine),
                    version,
                    port: None,
                    path: None,
                }))
            }
            ["yard", "service", "start", i] if is_name(i) => {
                HintAction::Rpc(RpcCall::ServicesStart(ServiceParams { name: owned(i) }))
            }
            ["yard", "source", "trust", f] if is_word(f) => {
                HintAction::Job(JobCall::SourceTrust(SourceParams { target: owned(f), tap: false }))
            }
            ["yard", "service", "logs", i] if is_name(i) => HintAction::GoTo(Screen::Logs(Some(owned(i)))),
            ["yard", "doctor"] => HintAction::GoTo(Screen::Doctor),
            ["yard", "services"] => HintAction::GoTo(Screen::Services(None)),
            ["yard", "info"] => HintAction::GoTo(Screen::Sites(None)),
            ["yard", "info", site] if is_name(site) => HintAction::GoTo(Screen::Sites(Some(owned(site)))),
            ["yard", "service", "port", i, p] if is_name(i) && is_placeholder(p) => {
                HintAction::EditPort { owner: owned(i), is_service: true }
            }
            ["yard", "ports", "set", u, p] if is_name(u) && is_placeholder(p) => {
                HintAction::EditPort { owner: owned(u), is_service: false }
            }
            ["yard", "init"] | ["yard", "init", "--yes"] => HintAction::Settle { site: None },
            ["edit", file] if file.starts_with('/') => HintAction::OpenFile(PathBuf::from(file)),
            // With `--root`, doctor names the root yardd serves; the app runs it for its own root,
            // which is the same one.
            ["sudo", "yard", "helper", "install"] | ["sudo", "yard", "--root", _, "helper", "install"] => {
                HintAction::Terminal(TerminalCommand::HelperInstall)
            }
            ["brew", verb @ ("install" | "reinstall"), f] if is_word(f) => {
                HintAction::Terminal(TerminalCommand::Brew {
                    reinstall: *verb == "reinstall",
                    formula: owned(f),
                })
            }
            _ => copy(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// Doctor's hint for a root that is not `~/.yard` names it; the app builds the command from its
    /// own root, which is that one (found by the checklist script: it offered only Copy).
    #[test]
    fn the_helper_install_hint_for_another_root_opens_terminal() {
        assert_eq!(
            HintAction::parse("sudo yard --root /tmp/yard-check helper install"),
            HintAction::Terminal(TerminalCommand::HelperInstall)
        );
        assert_eq!(
            HintAction::parse("sudo yard --root '/tmp/a b' helper install"),
            HintAction::Terminal(TerminalCommand::HelperInstall)
        );
    }

    #[test]
    fn hint_table() {
        let p = HintAction::parse;
        assert_eq!(p("yard start"), HintAction::Cli(vec![s("start")]));
        assert_eq!(p("yard stop && yard start"), HintAction::RestartDaemon);
        assert_eq!(p("yard trust"), HintAction::Cli(vec![s("trust")]));
        assert_eq!(
            p("yard php install 7.3"),
            HintAction::Job(JobCall::PhpInstall(PhpParams { php: s("7.3") }))
        );
        assert_eq!(
            p("yard ext add redis --php 7.1"),
            HintAction::Job(JobCall::ExtAdd(ExtAddParams { name: s("redis"), php: Some(s("7.1")) }))
        );
        assert_eq!(
            p("yard ext enable xdebug --php 8.4"),
            HintAction::Rpc(RpcCall::ExtEnable(ExtParams { name: s("xdebug"), php: Some(s("8.4")) }))
        );
        assert_eq!(
            p("yard ext disable opcache"),
            HintAction::Rpc(RpcCall::ExtDisable(ExtParams { name: s("opcache"), php: None }))
        );
        assert_eq!(
            p("yard ext remove apcu --php 8.4"),
            HintAction::Job(JobCall::ExtRemove(ExtParams { name: s("apcu"), php: Some(s("8.4")) }))
        );
        assert_eq!(
            p("yard node install 'lts/*'"),
            HintAction::Job(JobCall::NodeInstall(NodeInstallParams { spec: Some(s("lts/*")), path: None }))
        );
        assert_eq!(
            p("yard node install 22"),
            HintAction::Job(JobCall::NodeInstall(NodeInstallParams { spec: Some(s("22")), path: None }))
        );
        assert_eq!(
            p("yard service add mysql --version 8.4"),
            HintAction::Job(JobCall::ServiceAdd(ServiceAddParams {
                engine: s("mysql"),
                version: Some(s("8.4")),
                port: None,
                path: None
            }))
        );
        assert_eq!(
            p("yard service add emailulator"),
            HintAction::Job(JobCall::ServiceAdd(ServiceAddParams {
                engine: s("emailulator"),
                version: None,
                port: None,
                path: None
            }))
        );
        assert_eq!(
            p("yard service start redis"),
            HintAction::Rpc(RpcCall::ServicesStart(ServiceParams { name: s("redis") }))
        );
        assert_eq!(
            p("yard source trust shivammathur/php/php@8.3"),
            HintAction::Job(JobCall::SourceTrust(SourceParams {
                target: s("shivammathur/php/php@8.3"),
                tap: false
            }))
        );
        assert_eq!(p("yard service logs mysql-8.4"), HintAction::GoTo(Screen::Logs(Some(s("mysql-8.4")))));
        assert_eq!(p("yard doctor"), HintAction::GoTo(Screen::Doctor));
        assert_eq!(p("yard services"), HintAction::GoTo(Screen::Services(None)));
        assert_eq!(p("yard info blog"), HintAction::GoTo(Screen::Sites(Some(s("blog")))));
        assert_eq!(
            p("yard service port mysql-8.4 <port>"),
            HintAction::EditPort { owner: s("mysql-8.4"), is_service: true }
        );
        assert_eq!(
            p("yard ports set nginx-http <port>"),
            HintAction::EditPort { owner: s("nginx-http"), is_service: false }
        );
        assert_eq!(p("yard init"), HintAction::Settle { site: None });
        assert_eq!(p("yard init --yes"), HintAction::Settle { site: None });
        assert_eq!(
            p("edit /Users/me/Sites/blog/.yard"),
            HintAction::OpenFile(PathBuf::from("/Users/me/Sites/blog/.yard"))
        );
        assert_eq!(p("sudo yard helper install"), HintAction::Terminal(TerminalCommand::HelperInstall));
        assert_eq!(
            p("brew install nginx"),
            HintAction::Terminal(TerminalCommand::Brew { reinstall: false, formula: s("nginx") })
        );
        assert_eq!(
            p("brew reinstall shivammathur/php/php@7.1"),
            HintAction::Terminal(TerminalCommand::Brew {
                reinstall: true,
                formula: s("shivammathur/php/php@7.1")
            })
        );
    }

    #[test]
    fn unknown_and_malformed_hints_only_copy() {
        for hint in [
            "rm -rf ~/x",
            "yard service start \"a b\"",
            "yard php install 8.4; rm x",
            "sudo security remove-trusted-cert -d /x",
            "yard php install 'unclosed",
            "yard php install latest",
            "yard init --replace",
            "yard services lists the instances",
            "yard ports set nginx-http 8081",
            "emailulator service status",
            "brew install nginx && rm -rf /",
        ] {
            assert_eq!(HintAction::parse(hint), HintAction::Copy(hint.to_string()), "{hint}");
        }
    }
}
