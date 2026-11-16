//! The recipes Yard carries (recipes spec §8), compiled in so a dev build and a release read
//! the same ones. A user recipe with the same key replaces one.

use super::recipe::{LoadOptions, RecipeEngine, parse_recipe};
use super::{Origin, ServiceEngine};
use std::path::PathBuf;
use std::sync::Arc;

/// `(key, text)` for each carried recipe.
pub fn texts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("meilisearch", include_str!("../../../engines/meilisearch.toml")),
        ("memcached", include_str!("../../../engines/memcached.toml")),
        ("redis", include_str!("../../../engines/redis.toml")),
        ("typesense", include_str!("../../../engines/typesense.toml")),
        ("valkey", include_str!("../../../engines/valkey.toml")),
    ]
}

/// The carried recipes that load: every one does, which `every_carried_recipe_loads` checks,
/// so a broken one is a failed test, never a panic in yardd.
pub fn all() -> Vec<Arc<RecipeEngine>> {
    texts()
        .into_iter()
        .filter_map(|(key, text)| {
            parse_recipe(
                text,
                &PathBuf::from(format!("{key}.toml")),
                Origin::Carried,
                &LoadOptions::default(),
            )
            .ok()
        })
        .map(Arc::new)
        .collect()
}

pub(super) fn engines() -> Vec<Arc<dyn ServiceEngine>> {
    all().into_iter().map(|r| r as Arc<dyn ServiceEngine>).collect()
}
