//! The user's recipes in `~/.yard/engines/` (recipes spec §7): read at every reconcile, each
//! one's last good version kept, and the set handed to the snapshot.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use yard_core::Paths;
use yard_core::services::Engines;
use yard_core::services::engines::{LoadOptions, Origin, RecipeEngine, RecipeError, parse_recipe};
use yard_proto::YardError;

/// Every `*.toml` in `engines/`, sorted by name, read and parsed. A file that cannot be read is
/// an error for it; a directory that does not exist is no recipes.
pub fn read_recipes(
    paths: &Paths,
    opts: &LoadOptions,
) -> Vec<(PathBuf, Option<String>, Result<RecipeEngine, RecipeError>)> {
    let Ok(dir) = std::fs::read_dir(paths.engines_dir()) else { return Vec::new() };
    let mut files: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml") && p.is_file())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|file| match std::fs::read_to_string(&file) {
            Ok(text) => {
                let parsed = parse_recipe(&text, &file, Origin::User(file.clone()), opts);
                (file, Some(text), parsed)
            }
            Err(e) => {
                let error =
                    RecipeError { file: file.clone(), line: None, message: format!("cannot read it: {e}") };
                (file, None, Err(error))
            }
        })
        .collect()
}

/// The engines with the user's recipes, read once: for the offline doctor and the orphan sweep
/// at start, which have no last good versions to keep.
pub fn load_once(paths: &Paths, opts: &LoadOptions) -> Engines {
    let user = read_recipes(paths, opts).into_iter().map(|(_, _, r)| r).collect();
    Engines::with_user(Engines::builtin_shared(), user, &mut BTreeMap::new())
}

/// Keeps each recipe's last good version between reconciles, and what the files said, so a
/// reconcile knows whether the set changed.
#[derive(Debug, Clone, Default)]
pub struct RecipeLoader {
    opts: LoadOptions,
    last_good: BTreeMap<String, Arc<RecipeEngine>>,
    /// Each file and its text at the last load; `None` before the first.
    seen: Option<Vec<(PathBuf, Option<String>)>>,
}

impl RecipeLoader {
    pub fn new(opts: LoadOptions) -> Self {
        RecipeLoader { opts, ..Default::default() }
    }

    /// The engine set now, and whether any recipe file was added, edited or removed since the
    /// last load (never on the first).
    pub fn load(&mut self, paths: &Paths) -> (Engines, bool) {
        let read = read_recipes(paths, &self.opts);
        let now: Vec<(PathBuf, Option<String>)> =
            read.iter().map(|(f, t, _)| (f.clone(), t.clone())).collect();
        let changed = self.seen.as_ref().is_some_and(|seen| *seen != now);
        self.seen = Some(now);
        let user = read.into_iter().map(|(_, _, r)| r).collect();
        (Engines::with_user(Engines::builtin_shared(), user, &mut self.last_good), changed)
    }
}

/// What `status` and doctor say about the recipes: each file that did not load, and for one
/// whose engine runs on, that it does.
pub fn errors(engines: &Engines) -> Vec<YardError> {
    let mut out: Vec<YardError> = engines.errors().iter().map(RecipeError::to_yard_error).collect();
    for key in engines.keys() {
        if let Some(e) = engines.stale(&key) {
            let mut error = e.to_yard_error();
            error.message = format!("{} (the {key} engine runs on its last good version)", error.message);
            out.push(error);
        }
    }
    out
}
