//! Deciding what to write so that Yard never silently loses a hand edit.
//!
//! The rule compares the file on disk with the hash Yard stored when it last wrote it:
//!
//! | on disk vs stored hash | render vs stored hash | action |
//! |---|---|---|
//! | missing | — | Write |
//! | equal to the render | — | Unchanged (hash recorded) |
//! | equal | equal | Unchanged |
//! | equal | differs | Write |
//! | differs (hand-edited) | equal | KeepHandEdit (reported by `yard doctor`) |
//! | differs (hand-edited), or no stored hash | differs | BackupThenWrite (`<file>.yard-backup`) |
//! | in a managed dir, not rendered, has a stored hash | — | Remove, or BackupThenRemove if hand-edited |
//!
//! A file in a managed dir without a stored hash is never removed: it is the user's.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hashes(pub BTreeMap<PathBuf, String>);

impl Hashes {
    pub fn load(path: &Path) -> Result<Hashes, YardError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map(Hashes).map_err(|e| {
                YardError::new(ErrorCode::Internal, format!("{} is corrupt: {e}", path.display())).with_hint(
                    format!("delete {}; Yard re-adopts files that match their render", path.display()),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Hashes::default()),
            Err(e) => {
                Err(YardError::new(ErrorCode::Internal, format!("cannot read {}: {e}", path.display())))
            }
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.0).map_err(io::Error::other)?;
        write_atomic(path, &json)
    }
}

pub fn sha256_hex(content: &str) -> String {
    Sha256::digest(content.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Write,
    Unchanged,
    KeepHandEdit,
    BackupThenWrite,
    Remove,
    BackupThenRemove,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WritePlan {
    pub actions: BTreeMap<PathBuf, Action>,
    pub hand_edited: Vec<PathBuf>,
}

pub fn plan(
    rendered: &BTreeMap<PathBuf, String>,
    hashes: &Hashes,
    managed_dirs: &[PathBuf],
    disk: &dyn Fn(&Path) -> Option<String>,
    list: &dyn Fn(&Path) -> Vec<PathBuf>,
) -> WritePlan {
    let mut out = WritePlan::default();
    for (path, content) in rendered {
        let stored = hashes.0.get(path);
        let action = match disk(path) {
            None => Action::Write,
            Some(on_disk) if on_disk == *content => Action::Unchanged,
            Some(on_disk) => {
                let untouched = stored == Some(&sha256_hex(&on_disk));
                let render_same = stored == Some(&sha256_hex(content));
                match (untouched, render_same) {
                    (true, _) => Action::Write,
                    (false, true) => {
                        out.hand_edited.push(path.clone());
                        Action::KeepHandEdit
                    }
                    (false, false) => Action::BackupThenWrite,
                }
            }
        };
        out.actions.insert(path.clone(), action);
    }
    for dir in managed_dirs {
        for path in list(dir) {
            if rendered.contains_key(&path) {
                continue;
            }
            let Some(stored) = hashes.0.get(&path) else { continue };
            let action = match disk(&path) {
                Some(on_disk) if sha256_hex(&on_disk) != *stored => Action::BackupThenRemove,
                _ => Action::Remove,
            };
            out.actions.insert(path, action);
        }
    }
    out
}

/// What `apply` replaced, so a failed `nginx -t` can put everything back.
#[derive(Debug, Default)]
pub struct Applied {
    previous: BTreeMap<PathBuf, Option<String>>,
    previous_hashes: BTreeMap<PathBuf, Option<String>>,
}

impl Applied {
    pub fn changed(&self) -> impl Iterator<Item = &Path> {
        self.previous.keys().map(PathBuf::as_path)
    }
}

pub fn apply(
    plan: &WritePlan,
    rendered: &BTreeMap<PathBuf, String>,
    hashes: &mut Hashes,
) -> io::Result<Applied> {
    let mut applied = Applied::default();
    for (path, action) in &plan.actions {
        let remember = |applied: &mut Applied, hashes: &Hashes| -> io::Result<Option<String>> {
            let current = read_optional(path)?;
            applied.previous.insert(path.clone(), current.clone());
            applied.previous_hashes.insert(path.clone(), hashes.0.get(path).cloned());
            Ok(current)
        };
        match action {
            Action::Unchanged => {
                if let Some(content) = rendered.get(path) {
                    hashes.0.insert(path.clone(), sha256_hex(content));
                }
            }
            Action::KeepHandEdit => {}
            Action::Write | Action::BackupThenWrite => {
                let Some(content) = rendered.get(path) else { continue };
                let current = remember(&mut applied, hashes)?;
                if *action == Action::BackupThenWrite
                    && let Some(current) = current
                {
                    write_atomic(&backup_path(path), &current)?;
                }
                write_atomic(path, content)?;
                hashes.0.insert(path.clone(), sha256_hex(content));
            }
            Action::Remove | Action::BackupThenRemove => {
                let current = remember(&mut applied, hashes)?;
                if *action == Action::BackupThenRemove
                    && let Some(current) = current
                {
                    write_atomic(&backup_path(path), &current)?;
                }
                remove_optional(path)?;
                hashes.0.remove(path);
            }
        }
    }
    Ok(applied)
}

pub fn rollback(applied: &Applied, hashes: &mut Hashes) -> io::Result<()> {
    for (path, previous) in &applied.previous {
        match previous {
            Some(content) => write_atomic(path, content)?,
            None => remove_optional(path)?,
        }
    }
    for (path, hash) in &applied.previous_hashes {
        match hash {
            Some(h) => hashes.0.insert(path.clone(), h.clone()),
            None => hashes.0.remove(path),
        };
    }
    Ok(())
}

/// Creates a user-owned file once; returns whether it was created. Never writes an existing file.
pub fn ensure_user_file(path: &Path, initial: &str) -> io::Result<bool> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            io::Write::write_all(&mut f, initial.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".yard-backup");
    path.with_file_name(name)
}

/// Writes `<file>.yard-tmp` then renames it, so readers never see a half-written file.
pub fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".yard-tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// [`write_atomic`] for a file the user owns: a symlink is written through, not replaced
/// by a file, and the file keeps its mode.
pub fn write_preserving(path: &Path, content: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let target = std::fs::canonicalize(path)?;
    let mode = std::fs::metadata(&target)?.permissions().mode();
    write_atomic(&target, content)?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn remove_optional(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};

    const F: &str = "/r/nginx/sites/a.conf";

    #[test]
    fn write_preserving_follows_a_symlink_and_keeps_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (real, link) = (dir.path().join("dotfiles.ini"), dir.path().join("99-overrides.ini"));
        std::fs::write(&real, "a = 1\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_preserving(&link, "a = 2\n").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "a = 2\n");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    }

    fn rendered(pairs: &[(&str, &str)]) -> BTreeMap<PathBuf, String> {
        pairs.iter().map(|(p, c)| (PathBuf::from(p), c.to_string())).collect()
    }

    fn hashes(pairs: &[(&str, &str)]) -> Hashes {
        Hashes(pairs.iter().map(|(p, c)| (PathBuf::from(p), sha256_hex(c))).collect())
    }

    fn disk(pairs: &[(&str, &str)]) -> impl Fn(&Path) -> Option<String> {
        let m: HashMap<PathBuf, String> =
            pairs.iter().map(|(p, c)| (PathBuf::from(p), c.to_string())).collect();
        move |p| m.get(p).cloned()
    }

    fn no_list(_: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn action(plan: &WritePlan, p: &str) -> Action {
        plan.actions[Path::new(p)]
    }

    #[test]
    fn missing_file_is_written() {
        let p = plan(&rendered(&[(F, "X")]), &hashes(&[(F, "X")]), &[], &disk(&[]), &no_list);
        assert_eq!(action(&p, F), Action::Write);
    }

    #[test]
    fn untouched_and_unchanged_is_left_alone() {
        let p = plan(&rendered(&[(F, "X")]), &hashes(&[(F, "X")]), &[], &disk(&[(F, "X")]), &no_list);
        assert_eq!(action(&p, F), Action::Unchanged);
    }

    #[test]
    fn untouched_and_changed_is_written() {
        let p = plan(&rendered(&[(F, "Y")]), &hashes(&[(F, "X")]), &[], &disk(&[(F, "X")]), &no_list);
        assert_eq!(action(&p, F), Action::Write);
    }

    #[test]
    fn hand_edited_and_unchanged_render_is_kept() {
        let p = plan(&rendered(&[(F, "X")]), &hashes(&[(F, "X")]), &[], &disk(&[(F, "X-edited")]), &no_list);
        assert_eq!(action(&p, F), Action::KeepHandEdit);
        assert_eq!(p.hand_edited, vec![PathBuf::from(F)]);
    }

    #[test]
    fn hand_edited_and_changed_render_is_backed_up() {
        let p = plan(&rendered(&[(F, "Y")]), &hashes(&[(F, "X")]), &[], &disk(&[(F, "X-edited")]), &no_list);
        assert_eq!(action(&p, F), Action::BackupThenWrite);
    }

    #[test]
    fn unknown_file_matching_render_is_adopted_and_differing_one_backed_up() {
        let same = plan(&rendered(&[(F, "X")]), &Hashes::default(), &[], &disk(&[(F, "X")]), &no_list);
        assert_eq!(action(&same, F), Action::Unchanged);
        let other = plan(&rendered(&[(F, "X")]), &Hashes::default(), &[], &disk(&[(F, "mine")]), &no_list);
        assert_eq!(action(&other, F), Action::BackupThenWrite);
    }

    #[test]
    fn stale_generated_file_is_removed() {
        let dir = "/r/nginx/sites";
        let list = |_: &Path| vec![PathBuf::from(F)];
        let p = plan(&rendered(&[]), &hashes(&[(F, "X")]), &[PathBuf::from(dir)], &disk(&[(F, "X")]), &list);
        assert_eq!(action(&p, F), Action::Remove);
        let edited =
            plan(&rendered(&[]), &hashes(&[(F, "X")]), &[PathBuf::from(dir)], &disk(&[(F, "Z")]), &list);
        assert_eq!(action(&edited, F), Action::BackupThenRemove);
    }

    #[test]
    fn user_files_without_hash_are_never_removed() {
        let dir = "/r/nginx/sites";
        let list = |_: &Path| vec![PathBuf::from(F), PathBuf::from("/r/nginx/sites/a.conf.yard-backup")];
        let p = plan(&rendered(&[]), &Hashes::default(), &[PathBuf::from(dir)], &disk(&[(F, "X")]), &list);
        assert!(p.actions.is_empty(), "{:?}", p.actions);
    }

    #[test]
    fn hand_edit_is_backed_up_when_render_changes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.conf");
        std::fs::write(&f, "X-edited").unwrap();
        let mut h = Hashes([(f.clone(), sha256_hex("X"))].into_iter().collect());
        let r: BTreeMap<PathBuf, String> = [(f.clone(), "Y".to_string())].into_iter().collect();
        let p = plan(&r, &h, &[], &|p| std::fs::read_to_string(p).ok(), &no_list);
        assert_eq!(p.actions[&f], Action::BackupThenWrite);
        apply(&p, &r, &mut h).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.conf.yard-backup")).unwrap(), "X-edited");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "Y");
        assert_eq!(h.0[&f], sha256_hex("Y"));
    }

    #[test]
    fn rollback_restores_previous_contents_and_removes_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.conf");
        let new = dir.path().join("sub/new.conf");
        std::fs::write(&old, "A").unwrap();
        let mut h = Hashes([(old.clone(), sha256_hex("A"))].into_iter().collect());
        let before = h.clone();
        let r: BTreeMap<PathBuf, String> =
            [(old.clone(), "B".to_string()), (new.clone(), "N".to_string())].into_iter().collect();
        let p = plan(&r, &h, &[], &|p| std::fs::read_to_string(p).ok(), &no_list);
        let applied = apply(&p, &r, &mut h).unwrap();
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "N");
        rollback(&applied, &mut h).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "A");
        assert!(!new.exists());
        assert_eq!(h, before);
    }

    #[test]
    fn apply_records_hash_for_adopted_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.conf");
        std::fs::write(&f, "X").unwrap();
        let mut h = Hashes::default();
        let r: BTreeMap<PathBuf, String> = [(f.clone(), "X".to_string())].into_iter().collect();
        let p = plan(&r, &h, &[], &|p| std::fs::read_to_string(p).ok(), &no_list);
        apply(&p, &r, &mut h).unwrap();
        assert_eq!(h.0[&f], sha256_hex("X"));
    }

    #[test]
    fn hashes_round_trip_and_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/hashes.json");
        assert_eq!(Hashes::load(&path).unwrap(), Hashes::default());
        let h = hashes(&[(F, "X")]);
        h.save(&path).unwrap();
        assert_eq!(Hashes::load(&path).unwrap(), h);
    }

    #[test]
    fn ensure_user_file_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("overrides/a.conf");
        assert!(ensure_user_file(&f, "# yours\n").unwrap());
        std::fs::write(&f, "mine").unwrap();
        assert!(!ensure_user_file(&f, "# yours\n").unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "mine");
    }
}
