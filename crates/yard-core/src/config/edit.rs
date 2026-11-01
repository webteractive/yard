//! Comment-preserving edits to the user's TOML files. Every mutation of
//! `yard.toml`, `sites.toml` or `ports.toml` goes through here.

use super::{config_error, read_optional};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value};
use yard_proto::{ErrorCode, YardError};

#[derive(Debug)]
pub struct TomlFile {
    path: PathBuf,
    doc: DocumentMut,
}

impl TomlFile {
    /// A missing file loads as an empty document; `save` creates it.
    pub fn load(path: &Path) -> Result<Self, YardError> {
        let doc = match read_optional(path)? {
            Some(text) => {
                text.parse::<DocumentMut>().map_err(|e| config_error(path, &text, e.span(), e.message()))?
            }
            None => DocumentMut::new(),
        };
        Ok(Self { path: path.to_path_buf(), doc })
    }

    /// Loads `path`, applies `f`, and saves if `f` succeeded and changed anything, all
    /// under one lock for the process, so two edits never lose each other's change.
    /// Every read-modify-write of a user TOML in the daemon goes through here.
    pub fn edit<R>(
        path: &Path,
        f: impl FnOnce(&mut TomlFile) -> Result<R, YardError>,
    ) -> Result<R, YardError> {
        Self::locked(|| {
            let mut file = Self::load(path)?;
            let before = file.doc.to_string();
            let result = f(&mut file)?;
            if file.doc.to_string() != before {
                file.save()?;
            }
            Ok(result)
        })
    }

    /// Runs `f` under the lock `edit` takes, for a write that does not go through `edit`
    /// (`yard init` writing a managed `.yard`) but must not lose an edit made meanwhile.
    pub fn locked<R>(f: impl FnOnce() -> R) -> R {
        static LOCK: Mutex<()> = Mutex::new(());
        let _held = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f()
    }

    pub fn doc(&self) -> &DocumentMut {
        &self.doc
    }

    pub fn set(&mut self, keys: &[&str], value: Item) {
        let Some((last, parents)) = keys.split_last() else {
            return;
        };
        let table = descend(self.doc.as_table_mut(), parents);
        // Replace in place: `insert` replaces the key too, and with it the comment above it.
        match table.get_mut(last) {
            Some(item) => *item = value,
            None => {
                table.insert(last, value);
            }
        }
    }

    /// Removes the key, and any table above it that is left empty, so no bare header stays.
    pub fn remove(&mut self, keys: &[&str]) -> bool {
        let Some((_, parents)) = keys.split_last() else {
            return false;
        };
        // Where the file printed what goes, so its comments land above what printed next.
        let order = print_order(self.doc.as_table());
        let removes_table = table_at(self.doc.as_table(), keys).is_some();
        let (removed, comments) = remove_in(self.doc.as_table_mut(), keys);
        if let Some(comments) = comments {
            let anchor = if removes_table { keys } else { parents };
            self.keep_after(&order, anchor, &comments);
        }
        removed
    }

    /// Puts `comments` above the first header printed after the table at `anchor` (or its
    /// nearest printed parent), or first in what ends the file when none follows.
    fn keep_after(&mut self, order: &[Vec<String>], anchor: &[&str], comments: &str) {
        let start = (0..=anchor.len())
            .rev()
            .find_map(|n| {
                order.iter().position(|p| p.iter().map(String::as_str).eq(anchor[..n].iter().copied()))
            })
            .unwrap_or_default();
        for path in &order[start + 1..] {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let Some(table) = table_at_mut(self.doc.as_table_mut(), &path) else { continue };
            if table.is_dotted() || (table.is_implicit() && table.get_values().is_empty()) {
                continue;
            }
            let decor = table.decor_mut();
            let own = decor.prefix().and_then(|p| p.as_str()).unwrap_or_default().to_string();
            decor.set_prefix(format!("{comments}{own}"));
            return;
        }
        let trailing = self.doc.trailing().as_str().unwrap_or_default().to_string();
        self.doc.set_trailing(format!("{comments}{trailing}"));
    }

    /// Appends `value` to the string array at `keys` unless already present.
    pub fn array_push_unique(&mut self, keys: &[&str], value: &str) -> bool {
        let Some(array) = self.array_mut(keys) else {
            return false;
        };
        if array.iter().any(|v| v.as_str() == Some(value)) {
            return false;
        }
        array.push(value);
        true
    }

    pub fn array_remove(&mut self, keys: &[&str], value: &str) -> bool {
        let Some(array) = self.array_mut(keys) else {
            return false;
        };
        let before = array.len();
        array.retain(|v| v.as_str() != Some(value));
        array.len() != before
    }

    /// The array at `keys`, created (or replacing a non-array value) if needed.
    fn array_mut(&mut self, keys: &[&str]) -> Option<&mut Array> {
        let (last, parents) = keys.split_last()?;
        let table = descend(self.doc.as_table_mut(), parents);
        let item = table.entry(last).or_insert(Item::Value(Value::Array(Array::new())));
        if item.as_array().is_none() {
            *item = Item::Value(Value::Array(Array::new()));
        }
        item.as_array_mut()
    }

    /// Atomic: writes `<file>.tmp` and renames it over the original.
    pub fn save(&self) -> Result<(), YardError> {
        let io = |e: std::io::Error| {
            YardError::new(ErrorCode::Internal, format!("cannot write {}: {e}", self.path.display()))
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        // Unique, so a save from another process (the offline CLI) never shares it.
        static SAVES: AtomicU64 = AtomicU64::new(0);
        let n = SAVES.fetch_add(1, Ordering::Relaxed);
        let tmp = self.path.with_extension(format!("toml.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, self.doc.to_string()).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }
}

/// Walks to the table at `keys`, creating implicit tables so no empty
/// `[parent]` header is written for a path that only holds subtables.
fn descend<'a>(mut table: &'a mut dyn TableLike, keys: &[&str]) -> &'a mut dyn TableLike {
    for key in keys {
        let item = table.entry(key).or_insert_with(implicit_table);
        if item.as_table_like().is_none() {
            *item = implicit_table();
        }
        // Infallible: the item is a table (just ensured above).
        let Some(next) = item.as_table_like_mut() else { unreachable!() };
        table = next;
    }
    table
}

fn implicit_table() -> Item {
    let mut t = Table::new();
    t.set_implicit(true);
    Item::Table(t)
}

/// Removes the key at `keys`, and any table left empty above it. Returns whether it was
/// there, and the comment lines above it (and above each table it empties) when no value
/// follows it in its own table, for the caller to keep; otherwise they move above that value.
fn remove_in(table: &mut dyn TableLike, keys: &[&str]) -> (bool, Option<String>) {
    let Some((first, rest)) = keys.split_first() else {
        return (false, None);
    };
    if rest.is_empty() {
        let comments = leading_comments(table, first);
        // Values print before tables, so only a value can be what prints next in this table.
        let next = match table.get(first) {
            Some(item) if item.is_value() => table
                .iter()
                .skip_while(|(k, _)| k != first)
                .skip(1)
                .find(|(_, i)| i.is_value())
                .map(|(k, _)| k.to_string()),
            _ => None,
        };
        let removed = table.remove(first).is_some();
        return match (comments, next) {
            (Some(comments), Some(next)) => {
                keep_above_key(table, &next, &comments);
                (removed, None)
            }
            (comments, _) => (removed, comments),
        };
    }
    let own = leading_comments(table, first);
    let Some(child) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
        return (false, None);
    };
    let (removed, comments) = remove_in(child, rest);
    if removed && child.is_empty() {
        table.remove(first);
        // The header goes too, and the comments above it.
        let comments = format!("{}{}", own.unwrap_or_default(), comments.unwrap_or_default());
        return (removed, Some(comments).filter(|c| !c.is_empty()));
    }
    (removed, comments)
}

/// The comment lines toml_edit keeps above `key` in its decor (a table's, above its header),
/// which go when it is removed.
fn leading_comments(table: &dyn TableLike, key: &str) -> Option<String> {
    let (key, item) = table.get_key_value(key)?;
    let decor = match item.as_table() {
        Some(t) => t.decor(),
        None => key.leaf_decor(),
    };
    let prefix = decor.prefix()?.as_str()?;
    prefix.contains('#').then(|| prefix.to_string())
}

/// Puts `comments` above the value at `next`.
fn keep_above_key(table: &mut dyn TableLike, next: &str, comments: &str) {
    let Some((mut key, _)) = table.get_key_value_mut(next) else { return };
    let decor = key.leaf_decor_mut();
    let own = decor.prefix().and_then(|p| p.as_str()).unwrap_or_default().to_string();
    decor.set_prefix(format!("{comments}{own}"));
}

/// The paths of the tables under `root` in the order toml_edit prints them: by the position
/// each was parsed at, a table added since following the one before it.
fn print_order(root: &Table) -> Vec<Vec<String>> {
    fn visit(table: &Table, path: &mut Vec<String>, last: &mut isize, out: &mut Vec<(isize, Vec<String>)>) {
        if !table.is_dotted() {
            if let Some(position) = table.position() {
                *last = position;
            }
            out.push((*last, path.clone()));
        }
        for (key, item) in table.iter() {
            if let Some(child) = item.as_table() {
                path.push(key.to_string());
                visit(child, path, last, out);
                path.pop();
            }
        }
    }
    let mut out = Vec::new();
    visit(root, &mut Vec::new(), &mut 0, &mut out);
    // Stable, as toml_edit's is: the root first, then by position.
    out.sort_by_key(|(position, path)| (!path.is_empty(), *position));
    out.into_iter().map(|(_, path)| path).collect()
}

fn table_at<'a>(mut table: &'a Table, keys: &[&str]) -> Option<&'a Table> {
    for key in keys {
        table = table.get(key)?.as_table()?;
    }
    Some(table)
}

fn table_at_mut<'a>(mut table: &'a mut Table, keys: &[&str]) -> Option<&'a mut Table> {
    for key in keys {
        table = table.get_mut(key)?.as_table_mut()?;
    }
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::value;

    #[test]
    fn remove_keeps_the_comments_above_the_key_it_removes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".yard");
        let text = "# header\n# legacy mcrypt\nphp = \"7.3\"\ndocroot = \"web\"\n# tail\nsecure = true\n\n[x]\na = 1\n";
        std::fs::write(&path, text).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["php"]));
        assert_eq!(
            f.doc().to_string(),
            "# header\n# legacy mcrypt\ndocroot = \"web\"\n# tail\nsecure = true\n\n[x]\na = 1\n"
        );
        // The last plain key: its comments go to what follows it, the [x] header.
        assert!(f.remove(&["secure"]));
        assert_eq!(
            f.doc().to_string(),
            "# header\n# legacy mcrypt\ndocroot = \"web\"\n# tail\n\n[x]\na = 1\n"
        );
    }

    #[test]
    fn remove_keeps_the_comments_above_a_last_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".yard");
        // As yard isolate leaves a managed .yard: php appended last, comments after it.
        let text = "# header\ndocroot = \"web\"\n# above php\nphp = \"7.1\"\n\n# [services]\n# keep me\n";
        std::fs::write(&path, text).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["php"]));
        assert_eq!(
            f.doc().to_string(),
            "# header\ndocroot = \"web\"\n# above php\n\n# [services]\n# keep me\n"
        );

        // The last key of a table: its comments go above what follows the table.
        let text = "[sites.a]\npath = \"/a\"\n# a's php\nphp = \"7.1\"\n\n[sites.b]\npath = \"/b\"\n";
        std::fs::write(&path, text).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["sites", "a", "php"]));
        assert_eq!(f.doc().to_string(), "[sites.a]\npath = \"/a\"\n# a's php\n\n[sites.b]\npath = \"/b\"\n");
    }

    #[test]
    fn remove_keeps_the_comments_above_the_table_printed_next() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("services.toml");
        // Key order is services, emailulator; the file prints [services.b] after [emailulator].
        let text = "[services.a]\nengine = \"redis\"\n\n# mine\n[emailulator]\npath = \"/e\"\n\n# b\n[services.b]\nengine = \"redis\"\n";
        std::fs::write(&path, text).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["emailulator"]));
        assert_eq!(
            f.doc().to_string(),
            "[services.a]\nengine = \"redis\"\n\n# mine\n\n# b\n[services.b]\nengine = \"redis\"\n"
        );
    }

    #[test]
    fn remove_keeps_the_comments_above_a_table_it_empties() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yard.toml");
        let text = "# about x\n[x]\n# k\nk = 1\n\n[y]\nv = 2\n";
        std::fs::write(&path, text).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["x", "k"]));
        assert_eq!(f.doc().to_string(), "# about x\n# k\n\n[y]\nv = 2\n");
    }

    #[test]
    fn remove_keeps_the_comments_above_a_table_it_removes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("services.toml");
        let text = "# mine\n[emailulator]\npath = \"/e\"\n\n# redis\n[services.redis]\nengine = \"redis\"\n";
        std::fs::write(&path, text).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["emailulator"]));
        assert_eq!(f.doc().to_string(), "# mine\n\n# redis\n[services.redis]\nengine = \"redis\"\n");

        // The only table: its comments stay as the file's trailing comments.
        std::fs::write(&path, "# mine\n[emailulator]\npath = \"/e\"\n").unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["emailulator"]));
        assert_eq!(f.doc().to_string(), "# mine\n");
    }

    #[test]
    fn set_keeps_the_comment_above_a_key_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".yard");
        std::fs::write(&path, "# keep me\nphp = \"7.3\"\n# and me\n[sites.ee]\n# mine\nphp = \"7.1\"\n")
            .unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        f.set(&["php"], value("7.4"));
        f.set(&["sites", "ee", "php"], value("7.2"));
        assert_eq!(
            f.doc().to_string(),
            "# keep me\nphp = \"7.4\"\n# and me\n[sites.ee]\n# mine\nphp = \"7.2\"\n"
        );
    }

    #[test]
    fn toml_edit_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sites.toml");
        std::fs::write(&path, "# my sites\n[sites.api]\nproxy = \"http://127.0.0.1:3000\"  # vite\n")
            .unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        f.set(&["sites", "blog", "path"], value("/code/blog"));
        f.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my sites"), "{text}");
        assert!(text.contains("# vite"), "{text}");
        assert!(text.contains("[sites.blog]"), "{text}");
        assert!(!text.contains("[sites]\n"), "{text}");
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(parsed["sites"]["blog"]["path"].as_str(), Some("/code/blog"));
    }

    #[test]
    fn missing_file_loads_empty_and_save_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yard.toml");
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.array_push_unique(&["items"], "/a"));
        f.save().unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("items = [\"/a\"]"));
    }

    #[test]
    fn array_push_unique_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = TomlFile::load(&dir.path().join("y.toml")).unwrap();
        assert!(f.array_push_unique(&["items"], "/a"));
        assert!(!f.array_push_unique(&["items"], "/a"));
        assert!(f.array_remove(&["items"], "/a"));
        assert!(!f.array_remove(&["items"], "/a"));
    }

    #[test]
    fn remove_reports_whether_key_existed() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = TomlFile::load(&dir.path().join("s.toml")).unwrap();
        f.set(&["sites", "a", "path"], value("/a"));
        assert!(f.remove(&["sites", "a"]));
        assert!(!f.remove(&["sites", "a"]));
        assert!(!f.remove(&["nope", "x"]));
    }

    #[test]
    fn concurrent_edits_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yard.toml");
        std::thread::scope(|s| {
            for t in 0..4 {
                let path = &path;
                s.spawn(move || {
                    for i in 0..25 {
                        TomlFile::edit(path, |f| {
                            f.set(&["k", &format!("t{t}_{i}")], value(true));
                            Ok(())
                        })
                        .unwrap();
                    }
                });
            }
        });
        let doc = TomlFile::load(&path).unwrap();
        assert_eq!(doc.doc()["k"].as_table().unwrap().len(), 100);
    }

    #[test]
    fn remove_drops_the_tables_it_empties() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yard.toml");
        std::fs::write(
            &path,
            "default_php = \"8.4\"\n\n[php.\"8.4\".extensions]\nredis = { source = \"pecl\" }\n",
        )
        .unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        assert!(f.remove(&["php", "8.4", "extensions", "redis"]));
        f.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "default_php = \"8.4\"\n");
    }

    #[test]
    fn invalid_toml_is_config_invalid_with_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "a = 1\nb = \n").unwrap();
        let e = TomlFile::load(&path).unwrap_err();
        assert_eq!(e.code, yard_proto::ErrorCode::ConfigInvalid);
        assert_eq!(e.data.unwrap()["line"], 2);
    }
}
