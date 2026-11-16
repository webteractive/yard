//! The engines the Services screen offers (recipes spec §11): what yardd's `engines.list` says,
//! or the fixed list an older yardd implies.

use yard_proto::{EnginesListResult, engine_display};

/// What an older yardd, which has no `engines.list`, runs.
pub const FALLBACK: [&str; 5] = ["mysql", "postgres", "redis", "minio", "emailulator"];

/// The Add picker's entries as `(key, name)`: yardd's engines in its order, then emailulator,
/// which Yard points at rather than runs; without a list, the fallback.
pub fn picker(list: Option<&EnginesListResult>) -> Vec<(String, String)> {
    match list {
        Some(list) => list
            .engines
            .iter()
            .map(|e| (e.key.clone(), e.name.clone()))
            .chain([("emailulator".to_string(), engine_display("emailulator").to_string())])
            .collect(),
        None => FALLBACK.iter().map(|k| (k.to_string(), engine_display(k).to_string())).collect(),
    }
}

/// The recipe errors the Services screen shows: each file that did not load, and each engine
/// running on its last good version, by message.
pub fn errors(list: Option<&EnginesListResult>) -> Vec<String> {
    let Some(list) = list else { return vec![] };
    list.engines
        .iter()
        .filter_map(|e| e.error.as_ref())
        .chain(&list.errors)
        .map(|e| e.message.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list() -> EnginesListResult {
        serde_json::from_value(json!({
            "engines": [
                {"key": "meilisearch", "name": "Meilisearch", "source": "brew", "origin": {"kind": "carried"},
                 "installed": false, "ports": [], "console": true, "databases": false, "connect": true},
                {"key": "mysql", "name": "MySQL", "source": "brew", "origin": {"kind": "built_in"},
                 "installed": true, "ports": [], "console": false, "databases": true, "connect": true},
                {"key": "echo", "name": "Echo", "source": "path", "origin": {"kind": "user", "file": "/u/.yard/engines/echo.toml"},
                 "installed": true, "ports": [], "console": false, "databases": false, "connect": false,
                 "error": {"code": "ENGINE_RECIPE_INVALID", "message": "/u/.yard/engines/echo.toml:3: name is empty"}},
            ],
            "errors": [{"code": "ENGINE_RECIPE_INVALID", "message": "/u/.yard/engines/bad.toml:1: oops"}],
        }))
        .unwrap()
    }

    #[test]
    fn picker_lists_engines_from_daemon() {
        let names: Vec<(String, String)> = picker(Some(&list()));
        let keys: Vec<&str> = names.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["meilisearch", "mysql", "echo", "emailulator"]);
        assert_eq!(names[0].1, "Meilisearch");
    }

    #[test]
    fn picker_falls_back_without_engines_list() {
        let keys: Vec<String> = picker(None).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, FALLBACK);
        assert_eq!(picker(None)[0].1, "MySQL");
    }

    #[test]
    fn recipe_errors_are_listed() {
        assert_eq!(
            errors(Some(&list())),
            ["/u/.yard/engines/echo.toml:3: name is empty", "/u/.yard/engines/bad.toml:1: oops"]
        );
        assert!(errors(None).is_empty());
    }
}
