//! The one-glance health the menu bar icon and the window show (M6 design §10.4).

use crate::hint::HintAction;
use yard_core::present::holder_kind_name;
use yard_proto::{
    HolderKind, PortInfo, PortStatus, ServiceInfo, StatusInfo, TunnelInfo, TunnelState, UnitState,
};

/// The connection as health sees it; yard-app maps the session's state into this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// `stale`: yardd speaks an older protocol than this app.
    Connected {
        stale: bool,
    },
    Offline(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub text: String,
    pub action: Option<HintAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Health {
    Offline(String),
    Error(Vec<Problem>),
    Warning(Vec<Problem>),
    Busy,
    Ok,
}

fn problem(text: String, hint: Option<&str>) -> Problem {
    Problem { text, action: hint.map(HintAction::parse) }
}

/// Offline beats everything; then errors (a config that does not parse, a failed unit or service),
/// warnings (a held port, a failed tunnel, a stale yardd), busy while jobs run, else ok. A unit in
/// backoff or a waking service is not a problem.
pub fn evaluate(
    link: &Link,
    status: Option<&StatusInfo>,
    ports: Option<&[PortInfo]>,
    services: Option<&[ServiceInfo]>,
    tunnels: Option<&[TunnelInfo]>,
    running_jobs: usize,
) -> Health {
    let stale = match link {
        Link::Offline(why) => return Health::Offline(why.clone()),
        Link::Connected { stale } => *stale,
    };
    let mut errors = vec![];
    if let Some(s) = status {
        if let Some(e) = &s.config_error {
            errors.push(problem(e.message.clone(), e.hint.as_deref()));
        }
        // A service instance is a unit too; its failure is the service's problem below.
        let is_service = |unit: &str| services.unwrap_or_default().iter().any(|svc| svc.name == unit);
        for u in
            s.units.iter().filter(|u| matches!(u.state, UnitState::Failed { .. }) && !is_service(&u.name))
        {
            errors.push(problem(format!("{} failed", u.name), Some("yard doctor")));
        }
    }
    for s in services.unwrap_or_default() {
        if let Some(e) = s.state.failure(&s.name) {
            errors.push(problem(e.message, e.hint.as_deref()));
        }
    }
    if !errors.is_empty() {
        return Health::Error(errors);
    }
    let mut warnings = vec![];
    let is_service = |unit: &str| services.unwrap_or_default().iter().any(|svc| svc.name == unit);
    for p in ports.unwrap_or_default().iter().filter(|p| p.status == PortStatus::InUse) {
        let holder = p.holder.as_ref().map_or_else(
            || "another program".to_string(),
            |h| match h.kind {
                HolderKind::Unknown | HolderKind::Other => h.name.clone(),
                kind => format!("{} ({})", h.name, holder_kind_name(kind)),
            },
        );
        warnings.push(Problem {
            text: format!("{}'s port {} is held by {holder}", p.unit, p.port),
            action: Some(HintAction::EditPort { owner: p.unit.clone(), is_service: is_service(&p.unit) }),
        });
    }
    for t in tunnels.unwrap_or_default().iter().filter(|t| t.state == TunnelState::Failed) {
        let why = t.last_error.as_deref().unwrap_or("no reason given");
        warnings.push(problem(format!("{}'s tunnel failed: {why}", t.site), None));
    }
    if stale {
        warnings.push(problem("yardd is older than this app".into(), Some("yard stop && yard start")));
    }
    if !warnings.is_empty() {
        return Health::Warning(warnings);
    }
    if running_jobs > 0 { Health::Busy } else { Health::Ok }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yard_proto::{ErrorCode, YardError};

    fn status(units: serde_json::Value, config_error: Option<YardError>) -> StatusInfo {
        let mut s: StatusInfo = serde_json::from_value(json!({
            "version": {"cli": null, "daemon": "0.1.0", "helper": "0.1.0", "protocol": "2.11", "commit": "x", "build_date": "d"},
            "profile": "standard", "tld": "yard", "units": units, "sites": 0, "config_error": null, "sleeping": []
        }))
        .unwrap();
        s.config_error = config_error;
        s
    }

    fn unit(name: &str, state: serde_json::Value) -> serde_json::Value {
        let mut u = json!({"name": name, "pid": null, "restarts": 0, "last_error": null});
        u.as_object_mut().unwrap().extend(state.as_object().unwrap().clone());
        u
    }

    fn port(status: &str) -> PortInfo {
        serde_json::from_value(json!({
            "unit": "nginx-http", "port": 8080, "mode": "pinned", "preferred": null, "status": status,
            "holder": {"pid": 9, "name": "nginx", "path": null, "ppid": null, "kind": "herd"}
        }))
        .unwrap()
    }

    const UP: Link = Link::Connected { stale: false };

    #[test]
    fn health_order() {
        let broken = Some(YardError::new(ErrorCode::ConfigInvalid, "sites.toml does not parse"));
        let s = status(json!([]), broken.clone());
        assert!(matches!(
            evaluate(&Link::Offline("yardd is not running".into()), Some(&s), None, None, None, 3),
            Health::Offline(_)
        ));
        let Health::Error(problems) = evaluate(&UP, Some(&s), Some(&[port("in_use")]), None, None, 1) else {
            panic!()
        };
        assert_eq!(problems[0].text, "sites.toml does not parse");
        let failed = status(json!([unit("nginx", json!({"state": "failed", "last_lines": ["boom"]}))]), None);
        assert!(matches!(evaluate(&UP, Some(&failed), None, None, None, 0), Health::Error(_)));
        let ok = status(json!([unit("nginx", json!({"state": "backoff", "retry_in_ms": 500}))]), None);
        let Health::Warning(held) = evaluate(&UP, Some(&ok), Some(&[port("in_use")]), None, None, 1) else {
            panic!()
        };
        // It names what holds the port and offers to move Yard's unit (M6 design §8.1).
        assert_eq!(held[0].text, "nginx-http's port 8080 is held by nginx (Herd)");
        assert_eq!(
            held[0].action,
            Some(HintAction::EditPort { owner: "nginx-http".into(), is_service: false })
        );
        assert_eq!(evaluate(&UP, Some(&ok), Some(&[port("yard")]), None, None, 1), Health::Busy);
        assert_eq!(
            evaluate(&UP, Some(&ok), Some(&[port("yard")]), None, None, 0),
            Health::Ok,
            "backoff is not a problem"
        );
        assert!(matches!(
            evaluate(&Link::Connected { stale: true }, Some(&ok), None, None, None, 0),
            Health::Warning(_)
        ));
    }

    /// A service instance is a unit too: when it fails it is one problem, the service's, which
    /// names the holder and offers Port…, not also a bare `redis failed` that offers doctor (found
    /// by the checklist script: the menu listed both, the bare one first).
    #[test]
    fn a_failed_service_is_one_problem_that_names_the_holder() {
        let failed = status(json!([unit("redis", json!({"state": "failed", "last_lines": []}))]), None);
        let svc: ServiceInfo = serde_json::from_value(json!({
            "name": "redis", "engine": "redis", "version": null, "formula": "redis",
            "ports": [{"name": "redis", "port": 6380}], "stopped_by_user": false,
            "account": {"user": null, "password": null}, "console": null, "used_by": [],
            "state": {"failed": {"last_lines": [], "reason": {
                "code": "PORT_IN_USE", "message": "port 6380 is held by Python (pid 7, unknown)",
                "hint": "yard service port redis <port>"}}}
        }))
        .unwrap();
        let Health::Error(problems) = evaluate(&UP, Some(&failed), None, Some(&[svc]), None, 0) else {
            panic!()
        };
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].text.contains("held by Python"), "{}", problems[0].text);
    }
}
