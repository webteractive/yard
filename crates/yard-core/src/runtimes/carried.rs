//! The runtime recipes Yard carries (runtimes spec §5), compiled in so a dev build and a
//! release read the same ones.

use super::Origin;
use super::recipe::{RuntimeRecipe, parse_runtime_recipe};
use crate::services::engines::LoadOptions;
use std::path::PathBuf;

/// `(key, text)` for each carried recipe, by key.
pub fn texts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("composer", include_str!("../../runtimes/composer.toml")),
        ("node", include_str!("../../runtimes/node.toml")),
        ("php", include_str!("../../runtimes/php.toml")),
    ]
}

/// The carried recipes that load: every one does, which `every_carried_recipe_loads` checks,
/// so a broken one is a failed test, never a panic in a shim.
pub fn all() -> Vec<RuntimeRecipe> {
    texts()
        .into_iter()
        .filter_map(|(key, text)| {
            parse_runtime_recipe(
                text,
                &PathBuf::from(format!("{key}.toml")),
                Origin::Carried,
                &LoadOptions::default(),
            )
            .ok()
        })
        .collect()
}
