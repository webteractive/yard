//! Comment-preserving edits to the user's TOML files. Every mutation of
//! `yard.toml`, `sites.toml` or `ports.toml` goes through here.

use super::{config_error, read_optional};
use std::path::{Path, PathBuf};
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

    pub fn doc(&self) -> &DocumentMut {
        &self.doc
    }

    pub fn set(&mut self, keys: &[&str], value: Item) {
        let Some((last, parents)) = keys.split_last() else {
            return;
        };
        let table = descend(self.doc.as_table_mut(), parents);
        table.insert(last, value);
    }

    /// Removes the key, and any table above it that is left empty, so no bare header stays.
    pub fn remove(&mut self, keys: &[&str]) -> bool {
        remove_in(self.doc.as_table_mut(), keys)
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
        let tmp = self.path.with_extension("toml.tmp");
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

fn remove_in(table: &mut dyn TableLike, keys: &[&str]) -> bool {
    let Some((first, rest)) = keys.split_first() else {
        return false;
    };
    if rest.is_empty() {
        return table.remove(first).is_some();
    }
    let Some(child) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
        return false;
    };
    let removed = remove_in(child, rest);
    if removed && child.is_empty() {
        table.remove(first);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::value;

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
