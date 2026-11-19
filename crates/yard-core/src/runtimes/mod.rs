//! Runtimes (runtimes spec, `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md`):
//! PHP, Node and Composer behind one contract, each defined by a recipe.

pub mod carried;
pub mod recipe;
pub mod version;

use recipe::{Driver, RuntimeRecipe, SourceDef};
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

/// Where a runtime's recipe came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Compiled into Yard (`crates/yard-core/runtimes/`).
    Carried,
    /// The user's file in `~/.yard/runtimes/`.
    User(PathBuf),
}

/// Which code runs a runtime: PHP's driver, Node's fnm source, or the recipe alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Php,
    Node,
    Recipe,
}

/// Where a `.yard` pins a runtime: `php` and `node` at the top level, the rest under
/// `[runtimes]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinKey {
    TopLevel(&'static str),
    Runtimes(String),
}

/// A runtime (runtimes spec §4). Every runtime is a recipe; [`RuntimeKind`] says which code
/// runs it.
pub trait Runtime: Send + Sync + Debug {
    fn key(&self) -> &str;
    /// What it is called (`Composer`).
    fn display(&self) -> &str;
    fn origin(&self) -> &Origin;
    fn kind(&self) -> RuntimeKind;
    /// Its shim names, sorted.
    fn shims(&self) -> Vec<String>;
    fn pin_key(&self) -> PinKey;
    fn pin_files(&self) -> &[String];
    /// Files in a site root that mean the site uses the runtime.
    fn uses_when(&self) -> &[String];
    /// Whether `shim` asks wake.sock for the site's sleeping services before it runs.
    fn wakes(&self, shim: &str) -> bool;
    fn recipe(&self) -> &RuntimeRecipe;
}

/// The one [`Runtime`]: a loaded recipe.
#[derive(Debug, Clone)]
pub struct RecipeRuntime {
    pub recipe: RuntimeRecipe,
}

impl Runtime for RecipeRuntime {
    fn key(&self) -> &str {
        &self.recipe.key
    }

    fn display(&self) -> &str {
        &self.recipe.name
    }

    fn origin(&self) -> &Origin {
        &self.recipe.origin
    }

    fn kind(&self) -> RuntimeKind {
        match (&self.recipe.driver, &self.recipe.source) {
            (Some(Driver::Php), _) => RuntimeKind::Php,
            (None, SourceDef::Fnm) => RuntimeKind::Node,
            _ => RuntimeKind::Recipe,
        }
    }

    fn shims(&self) -> Vec<String> {
        self.recipe.shims.keys().cloned().collect()
    }

    fn pin_key(&self) -> PinKey {
        match self.kind() {
            RuntimeKind::Php => PinKey::TopLevel("php"),
            RuntimeKind::Node => PinKey::TopLevel("node"),
            RuntimeKind::Recipe => PinKey::Runtimes(self.recipe.key.clone()),
        }
    }

    fn pin_files(&self) -> &[String] {
        &self.recipe.pin_files
    }

    fn uses_when(&self) -> &[String] {
        &self.recipe.uses_when
    }

    fn wakes(&self, shim: &str) -> bool {
        self.recipe.shims.get(shim).is_some_and(|s| s.wake.unwrap_or(self.recipe.wake))
    }

    fn recipe(&self) -> &RuntimeRecipe {
        &self.recipe
    }
}

/// Every runtime this Yard knows, by key.
#[derive(Debug, Clone)]
pub struct Runtimes {
    list: Vec<Arc<dyn Runtime>>,
}

impl Runtimes {
    /// The carried recipes (this round reads no user recipes).
    pub fn carried() -> Runtimes {
        let list = carried::all()
            .into_iter()
            .map(|recipe| Arc::new(RecipeRuntime { recipe }) as Arc<dyn Runtime>)
            .collect();
        Runtimes { list }
    }

    pub fn get(&self, key: &str) -> Option<&Arc<dyn Runtime>> {
        self.list.iter().find(|r| r.key() == key)
    }

    /// The runtime owning the shim `name`.
    pub fn by_shim(&self, name: &str) -> Option<&Arc<dyn Runtime>> {
        self.list.iter().find(|r| r.recipe().shims.contains_key(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Runtime>> {
        self.list.iter()
    }
}

#[cfg(test)]
mod recipe_tests;
