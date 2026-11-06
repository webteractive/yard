//! Installing what a site's `.yard` asks for, in the CLI's order (M6 design §8.2 Sites): PHP, then
//! extensions, then services, then Node.

use yard_core::present::{install_hint, missing_node};
use yard_proto::{ErrorCode, Need, SiteInfo, YardError};

/// One stage of a settle, run as jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleStage {
    Php(Vec<String>),
    /// `(name, php)`.
    Extensions(Vec<(String, String)>),
    /// `(engine, version)`.
    Services(Vec<(String, Option<String>)>),
    Node(String),
}

/// What the settle form shows before the user agrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlePlan {
    /// One line per need, as the CLI prints them: `PHP 7.3  (yard php install 7.3)`.
    pub lines: Vec<String>,
    /// Needs the app cannot install (manual, or unknown to this build): listed, never run.
    pub manual: Vec<Need>,
    pub node_only: bool,
    /// What **Not now** leaves beside the site when its Node is missing: the note `yard init`
    /// prints, with the install command as its hint. A missing Node never blocks serving.
    pub not_now: Option<YardError>,
}

impl SettlePlan {
    /// `None` when the site needs nothing.
    pub fn new(site: &SiteInfo) -> Option<SettlePlan> {
        let node = missing_node(site.node.as_ref());
        if site.needs.is_empty() && node.is_none() {
            return None;
        }
        let mut lines: Vec<String> =
            site.needs.iter().map(|n| format!("{}  ({})", n.describe(), n.command())).collect();
        if let Some(spec) = node {
            lines.push(format!("Node {spec}  ({})", install_hint(&spec)));
        }
        let manual = site.needs.iter().filter(|n| !n.installable()).cloned().collect();
        let not_now = node.map(|spec| {
            YardError::new(
                ErrorCode::NodeMissing,
                format!("{} runs Node {spec}, which is not installed", site.name),
            )
            .with_hint(install_hint(&spec))
        });
        Some(SettlePlan { lines, manual, node_only: site.needs.is_empty(), not_now })
    }
}

fn stages(site: &SiteInfo) -> Vec<SettleStage> {
    let mut php = vec![];
    let mut ext = vec![];
    let mut svc = vec![];
    for need in &site.needs {
        match need {
            Need::Php { php: v } => php.push(v.clone()),
            Need::Extension { name, php: v } => ext.push((name.clone(), v.clone())),
            Need::Service { engine, version } => svc.push((engine.clone(), version.clone())),
            Need::Manual { .. } | Need::Other => {}
        }
    }
    let mut out = vec![];
    if !php.is_empty() {
        out.push(SettleStage::Php(php));
    }
    if !ext.is_empty() {
        out.push(SettleStage::Extensions(ext));
    }
    if !svc.is_empty() {
        out.push(SettleStage::Services(svc));
    }
    if let Some(spec) = missing_node(site.node.as_ref()) {
        out.push(SettleStage::Node(spec.to_string()));
    }
    out
}

fn rank(stage: &SettleStage) -> u8 {
    match stage {
        SettleStage::Php(_) => 0,
        SettleStage::Extensions(_) => 1,
        SettleStage::Services(_) => 2,
        SettleStage::Node(_) => 3,
    }
}

/// The stage after `after`, read from the site as yardd reports it now: extensions are only known
/// once their PHP is installed, so the caller refetches the site between stages.
pub fn next_stage(site: &SiteInfo, after: Option<&SettleStage>) -> Option<SettleStage> {
    let floor = after.map_or(0, |a| rank(a) + 1);
    stages(site).into_iter().find(|s| rank(s) >= floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(needs: serde_json::Value, node: serde_json::Value) -> SiteInfo {
        serde_json::from_value(json!({
            "name": "legacy", "host": "legacy.yard", "url": "http://legacy.yard:8080/", "source": "linked",
            "kind": "php", "path": "/p", "docroot": "/p/public", "php": "7.3", "needs": needs, "node": node
        }))
        .unwrap()
    }

    fn all() -> SiteInfo {
        site(
            json!([
                {"kind": "service", "engine": "mysql", "version": "8.0"},
                {"kind": "extension", "name": "redis", "php": "7.3"},
                {"kind": "php", "php": "7.3"}
            ]),
            json!({"spec": "lts/*", "source": "project", "version": null}),
        )
    }

    #[test]
    fn settle_orders_php_then_extensions_then_services_then_node() {
        let s = all();
        let php = next_stage(&s, None);
        assert_eq!(php, Some(SettleStage::Php(vec!["7.3".into()])));
        let ext = next_stage(&s, php.as_ref());
        assert_eq!(ext, Some(SettleStage::Extensions(vec![("redis".into(), "7.3".into())])));
        let svc = next_stage(&s, ext.as_ref());
        assert_eq!(svc, Some(SettleStage::Services(vec![("mysql".into(), Some("8.0".into()))])));
        let node = next_stage(&s, svc.as_ref());
        assert_eq!(node, Some(SettleStage::Node("lts/*".into())));
        assert_eq!(next_stage(&s, node.as_ref()), None);
    }

    #[test]
    fn settle_lines_match_the_cli() {
        let plan = SettlePlan::new(&all()).unwrap();
        assert_eq!(
            plan.lines,
            [
                "MySQL 8.0  (yard service add mysql --version 8.0)".to_string(),
                "the redis extension for PHP 7.3  (yard ext add redis --php 7.3)".to_string(),
                "PHP 7.3  (yard php install 7.3)".to_string(),
                "Node lts/*  (yard node install 'lts/*')".to_string(),
            ]
        );
        assert!(!plan.node_only && plan.manual.is_empty());
    }

    #[test]
    fn manual_and_other_are_listed_not_run() {
        let s = site(
            json!([{"kind": "manual", "what": "emailulator", "command": "curl … | sh"}, {"kind": "smoke"}]),
            json!(null),
        );
        let plan = SettlePlan::new(&s).unwrap();
        assert_eq!(plan.manual.len(), 2);
        assert_eq!(next_stage(&s, None), None, "nothing to run");
    }

    /// Not now leaves the note `yard init` prints for a missing Node, with its install command
    /// (design §11.1; M6 final review: Not now left nothing).
    #[test]
    fn not_now_leaves_the_node_note() {
        let s = site(json!([]), json!({"spec": "22", "source": "project", "version": null}));
        let note = SettlePlan::new(&s).unwrap().not_now.expect("a note");
        assert_eq!(note.message, "legacy runs Node 22, which is not installed");
        assert_eq!(note.hint.as_deref(), Some("yard node install 22"));
        assert_eq!(note.code, yard_proto::ErrorCode::NodeMissing);
        assert!(SettlePlan::new(&all()).unwrap().not_now.is_some());
        let php_only = site(json!([{"kind": "php", "php": "7.3"}]), json!(null));
        assert_eq!(SettlePlan::new(&php_only).unwrap().not_now, None);
    }

    #[test]
    fn node_alone_is_only_node() {
        let s = site(json!([]), json!({"spec": "22", "source": "project", "version": null}));
        let plan = SettlePlan::new(&s).unwrap();
        assert!(plan.node_only);
        assert_eq!(next_stage(&s, None), Some(SettleStage::Node("22".into())));
        assert!(SettlePlan::new(&site(json!([]), json!(null))).is_none());
    }
}
