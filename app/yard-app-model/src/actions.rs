//! What the app remembers about the actions a person started: the last error per control, and
//! the questions it must ask before going on (M6 design §10, §11).

use std::collections::HashMap;
use yard_proto::{ErrorCode, YardError};

/// The control an action belongs to: `site:blog:secure`, `service:redis:start`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActionId(pub String);

/// The last error of each action, shown beside its control until the action succeeds.
#[derive(Debug, Default)]
pub struct ActionErrors {
    errors: HashMap<ActionId, YardError>,
}

impl ActionErrors {
    pub fn record(&mut self, id: ActionId, error: YardError) {
        self.errors.insert(id, error);
    }

    pub fn clear(&mut self, id: &ActionId) {
        self.errors.remove(id);
    }

    pub fn get(&self, id: &ActionId) -> Option<&YardError> {
        self.errors.get(id)
    }

    /// Every error whose action id starts with `prefix`, sorted by id: a screen's errors.
    pub fn matching(&self, prefix: &str) -> Vec<(ActionId, YardError)> {
        let mut found: Vec<(ActionId, YardError)> = self
            .errors
            .iter()
            .filter(|(id, _)| id.0.starts_with(prefix))
            .map(|(id, e)| (id.clone(), e.clone()))
            .collect();
        found.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        found
    }
}

/// A question the app asks before it goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// brew must trust `formula` (from `tap`) before Yard installs it.
    Trust { formula: String, tap: String },
    /// `destructive` is the index of the button that does the irreversible thing.
    Confirm { title: String, message: String, buttons: Vec<String>, destructive: usize },
}

/// The trust question for a `FORMULA_UNTRUSTED` that names its formula and tap.
pub fn trust_prompt(e: &YardError) -> Option<Prompt> {
    if e.code != ErrorCode::FormulaUntrusted {
        return None;
    }
    let data = e.data.as_ref()?;
    let field = |k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Some(Prompt::Trust { formula: field("formula")?, tap: field("tap")? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yard_proto::{ErrorCode, YardError};

    #[test]
    fn errors_are_kept_per_action_and_cleared_on_success() {
        let mut errors = ActionErrors::default();
        let secure = ActionId("site:blog:secure".into());
        let php = ActionId("site:blog:php".into());
        errors.record(secure.clone(), YardError::new(ErrorCode::Internal, "boom"));
        errors.record(php.clone(), YardError::new(ErrorCode::PhpNotInstalled, "PHP 7.0 is not installed"));
        assert_eq!(errors.get(&secure).map(|e| e.message.as_str()), Some("boom"));
        errors.clear(&secure);
        assert!(errors.get(&secure).is_none());
        assert_eq!(errors.get(&php).map(|e| e.code), Some(ErrorCode::PhpNotInstalled));
    }

    #[test]
    fn errors_are_found_by_prefix() {
        let mut errors = ActionErrors::default();
        errors.record(ActionId("php:8.4:install".into()), YardError::new(ErrorCode::Internal, "a"));
        errors.record(ActionId("php:7.1:default".into()), YardError::new(ErrorCode::Internal, "b"));
        errors.record(ActionId("node:install".into()), YardError::new(ErrorCode::Internal, "c"));
        let php: Vec<String> = errors.matching("php:").into_iter().map(|(id, _)| id.0).collect();
        assert_eq!(php, ["php:7.1:default", "php:8.4:install"], "sorted by id");
    }

    #[test]
    fn trust_prompt_needs_formula_and_tap() {
        let untrusted =
            YardError::new(ErrorCode::FormulaUntrusted, "shivammathur/php/php@8.3 is not trusted");
        let with = untrusted
            .clone()
            .with_data(json!({"formula": "shivammathur/php/php@8.3", "tap": "shivammathur/php"}));
        assert_eq!(
            trust_prompt(&with),
            Some(Prompt::Trust {
                formula: "shivammathur/php/php@8.3".into(),
                tap: "shivammathur/php".into()
            })
        );
        assert_eq!(trust_prompt(&untrusted), None, "no data, nothing to trust");
        let other = YardError::new(ErrorCode::Internal, "x").with_data(json!({"formula": "a", "tap": "b"}));
        assert_eq!(trust_prompt(&other), None);
        assert_eq!(trust_prompt(&untrusted.with_data(json!({"formula": "a"}))), None);
    }
}
