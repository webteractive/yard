# M4a.1 Managed `.yard` Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A site's `.yard` can live at `<data root>/sites/<name>/.yard`, so Yard serves a
project the way its `.yard` says without a file in the repository. `yard init` writes that
managed file by default; a committed `.yard` always wins.

**Architecture:**
- **Precedence.** One precedence function in `yard-core::project::load` picks each linked
  site's file: the committed `.yard` (anything at the path), else the managed one when its
  `project` key matches the site's path. Everything downstream, including resolution,
  needs, the setup page, the doctor and `info`, reads the chosen file through a new
  `Loaded` record instead of a bare `Result<ProjectFile>`.
- **Who uses it.** The shims apply the same rule without a daemon. `sites.init` writes the
  managed file. `isolate`, `secure` and `docroot` edit it through `TomlFile`.

**Tech Stack:** Rust 1.92 workspace (`yard-proto`, `yard-core`, `yard-daemon`, `yard-cli`,
`yard-shim`), `toml`/`toml_edit`, `notify` debouncer (FSEvents), `insta`, tokio.

**Spec:** `docs/superpowers/specs/2026-10-24-yard-managed-project-file-design.md` (amends
`docs/superpowers/specs/2026-10-22-yard-project-file-design.md`; read both).

## Global Constraints

- **Managed file path:** `<data root>/sites/<name>/.yard`, keyed by the bare site name,
  never `<name>.<tld>`.
- **Committed `.yard` wins** as soon as anything is at `<project>/.yard` (`symlink_metadata`).
  An unusable committed file never falls through to the managed one.
- **The `project` key:**
  - It is an absolute path, valid only in a managed file. In a committed one it is
    `PROJECT_FILE_INVALID`.
  - A managed file is used only while `project` equals the site's `path` in `sites.toml`
    exactly.
  - `init` writes the site's `path` as `sites.toml` records it.
- **Reading and writing files:**
  - Reads stay at most 64 KiB and must resolve to a regular file. Reads follow symlinks.
  - Writes never go through a symlink: `init` refuses a symlinked target, and the edit
    commands refuse a symlinked managed file.
- **Writing into the project:** Yard writes into a project only in `yard init --local`.
- **Editing `.yard` files:** Yard never edits a committed `.yard`. A managed file is
  edited only by `init` and by `isolate`/`unisolate`/`secure`/`unsecure`/`docroot`, through
  `toml_edit` under `TomlFile::edit`'s lock.
- **Unlink:** `yard unlink` never deletes a managed file.
- **Headers:**
  - A managed file starts with `# .yard for <name>, kept by Yard outside the project.`
  - A committed one starts with `# .yard: how Yard serves this project. Commit it.`
    (`project::HEADER`, unchanged).
- **Protocol** goes 2.5 → 2.6 (additive). New error code `PROJECT_FILE_EXISTS`.
- **Doctor ids:** `managed-file-ignored:<site>` (warn), `managed-file-mismatch:<name>`
  (warn), `managed-file-orphan:<name>` (info).
- **Repo conventions:**
  - `yard-core` stays pure.
  - No `.unwrap()`/`.expect()` on daemon runtime paths.
  - Tests use a temp `YARD_ROOT`.
  - Clippy `-D warnings` and `cargo fmt --check` stay clean.
  - Docs move with the code (AGENTS.md, README, specs, `.hydra/rules`, `--help`).
- **Commits:** a lowercase prose subject, no type prefix, no trailers. Implementers stage
  only, and the controller commits.

## Review Focus

1. **A `sites.toml` path spelled through a symlink.** A hand-edited path such as
   `~/code` → `/Volumes/x/code` must still match the `project` that `init` wrote, because
   `init` writes the entry's own spelling. Pinned in Task 4.
2. **A managed file the user broke by hand.** Expected: the site serves the setup page
   naming the managed file, and `yard isolate` fails with `PROJECT_FILE_INVALID` and
   writes nothing. Pinned in Tasks 1 and 5.
3. **`yard isolate 7.2` for a version that isn't installed.** Expected: it is refused
   exactly as today, and the managed file is untouched. Pinned in Task 5.
4. **Relinking a name with a different directory.** After `yard unlink app` and
   `yard link app` in another directory, the old managed file is not used, and `link`
   names it as being for the other project. Pinned in Task 4.
5. **`sites/` doesn't exist when yardd starts, and appears later.** Expected: a managed
   file created afterwards is picked up within about 2 s. Pinned in Task 2.

---

### Task 1: The managed file in core resolution, protocol fields, and every reader

**Files:**
- Modify: `crates/yard-core/src/paths.rs` (two helpers)
- Modify: `crates/yard-core/src/project.rs` (`project` key, `Loaded`, `load`)
- Modify: `crates/yard-core/src/sites.rs:99-140` (resolution reads `Loaded`)
- Modify: `crates/yard-proto/src/types.rs` (`SiteInfo` fields, `ProjectFileKind`, `IgnoredManagedFile`), `crates/yard-proto/src/lib.rs` (`PROTOCOL_VERSION = "2.6"`)
- Modify: `crates/yard-daemon/src/project.rs` (`read_project_file` dangling symlink, `read_projects(sites, paths)`, `fill_needs`)
- Modify every consumer of `Projects`:
  - `crates/yard-daemon/src/reconcile.rs:159,275`
  - `crates/yard-daemon/src/doctor.rs:108,331,497,548`
  - `crates/yard-daemon/src/services.rs:131`
  - `crates/yard-daemon/src/rpc/project.rs:109`
  - `crates/yard-daemon/src/rpc/php.rs:117-135`
  - `crates/yard-daemon/src/rpc/services.rs:96-101`
  - `crates/yard-daemon/src/rpc/info.rs`
  - `crates/yard-daemon/src/state.rs`
- Test: `crates/yard-core/src/project.rs` (unit), `crates/yard-core/src/sites.rs` (unit), `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Produces, in `yard_core::Paths`:
  - `pub fn managed_sites_dir(&self) -> PathBuf`, which is `<root>/sites`;
  - `pub fn managed_project_file(&self, name: &str) -> PathBuf`, which is `<root>/sites/<name>/.yard`.
- Produces, in `yard_core::project`:
  - `pub enum FileKind { Committed, Managed }`.
  - `pub struct Loaded { pub file: PathBuf, pub kind: FileKind, pub parsed: Result<ProjectFile, YardError> }`, with `impl Loaded { pub fn ok(&self) -> Option<&ProjectFile> }`.
  - `pub struct Ignored { pub file: PathBuf, pub because: IgnoredBecause }`.
  - `pub enum IgnoredBecause { Committed, OtherProject(Option<PathBuf>) }`.
  - `pub type Projects = BTreeMap<String, Loaded>`: only the sites served from a file.
  - `pub type IgnoredFiles = BTreeMap<String, Ignored>`: per site, a managed file that is not used.
  - `pub fn parse_managed(text: &str, file: &Path) -> Result<(ProjectFile, Option<PathBuf>), YardError>`: the second value is `project`.
  - `pub fn load(sites: &SitesConfig, paths: &Paths, read: &dyn Fn(&Path) -> Option<Result<String, YardError>>) -> (Projects, IgnoredFiles)`.
  - `pub const MANAGED_HEADER_PREFIX: &str = "# .yard for "`, and `pub fn managed_header(name: &str) -> String`, which returns `"# .yard for {name}, kept by Yard outside the project.\n"`.
- Produces, in `yard_proto`:
  - On `SiteInfo`:
    - `#[serde(default, skip_serializing_if = "Option::is_none")] pub project_file_kind: Option<ProjectFileKind>`;
    - `#[serde(default, skip_serializing_if = "Option::is_none")] pub ignored_managed_file: Option<IgnoredManagedFile>`.
  - `#[serde(rename_all = "snake_case")] pub enum ProjectFileKind { Project, Managed }`.
  - `pub struct IgnoredManagedFile { pub file: String, pub because: IgnoredBecause, #[serde(default, skip_serializing_if = "Option::is_none")] pub project: Option<String> }`, with `#[serde(rename_all = "snake_case")] pub enum IgnoredBecause { Committed, OtherProject }`.
- Produces, in `yard_daemon::project`: `pub fn read_projects(sites: &SitesConfig, paths: &Paths) -> (Projects, IgnoredFiles)`. `Snapshot` gains `pub ignored_managed: IgnoredFiles` beside `projects`.

- [ ] **Step 1: Write the failing core tests** in `project.rs`'s test module:

```rust
#[test]
fn project_key_is_only_for_a_managed_file() {
    let e = parse("project = \"/code/ee\"\n", Path::new(FILE)).unwrap_err();
    assert_eq!(e.code, ErrorCode::ProjectFileInvalid);
    assert_eq!(e.data.as_ref().unwrap()["line"], 1);
    let (f, project) = parse_managed("php = \"7.3\"\nproject = \"/code/ee\"\n", Path::new("/r/sites/ee/.yard")).unwrap();
    assert_eq!((f.php, project), (Some(v("7.3")), Some(PathBuf::from("/code/ee"))));
    let e = parse_managed("project = \"code/ee\"\n", Path::new("/r/sites/ee/.yard")).unwrap_err();
    assert_eq!(e.code, ErrorCode::ProjectFileInvalid); // must be absolute
}

#[test]
fn load_picks_committed_then_a_matching_managed_file() {
    // Uses a read closure over a BTreeMap<PathBuf, Option<Result<String,_>>> and Paths::new("/r").
    // sites: ee -> /code/ee, app -> /code/app, other -> /code/other, broken -> /code/broken
    // files: /code/ee/.yard = php 7.3; /r/sites/ee/.yard = php 7.4, project /code/ee
    //        /r/sites/app/.yard = php 7.4, project /code/app
    //        /r/sites/other/.yard = php 7.4, project /code/elsewhere
    //        /code/broken/.yard = Err(PROJECT_FILE_INVALID "is a symlink to nothing"); /r/sites/broken/.yard = php 7.4, project /code/broken
    let (p, ignored) = load(&sites, &Paths::new("/r"), &read);
    assert_eq!((p["ee"].kind, p["ee"].ok().unwrap().php), (FileKind::Committed, Some(v("7.3"))));
    assert_eq!(ignored["ee"].file, PathBuf::from("/r/sites/ee/.yard"));
    assert_eq!(ignored["ee"].because, IgnoredBecause::Committed);
    assert_eq!((p["app"].kind, p["app"].file.as_path()), (FileKind::Managed, Path::new("/r/sites/app/.yard")));
    assert!(!ignored.contains_key("app"));
    assert!(!p.contains_key("other")); // served from sites.toml, as if it had no file
    assert_eq!(ignored["other"].because, IgnoredBecause::OtherProject(Some("/code/elsewhere".into())));
    assert!(p["broken"].parsed.is_err() && p["broken"].kind == FileKind::Committed); // never falls through
    assert_eq!(ignored["broken"].because, IgnoredBecause::Committed);
}

#[test]
fn a_broken_managed_file_is_the_sites_error() {
    // /r/sites/ee/.yard = "php = 7.3\n" (unquoted): the site loads Managed with Err naming that file.
}
```


- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p yard-core project::tests`
Expected: compile errors (`parse_managed`, `load` signature, `FileKind` not found).

- [ ] **Step 3: Implement it in core.**
  - **`load`** returns two maps: the sites served from a committed or matching managed
    file, and, per site, a managed file that is not used (hidden by a committed one, or
    naming another project or none).
  - **`parse`** stays the committed parser and rejects `project` at its span with the
    message `project is only for a managed .yard`.
  - **`parse_managed`** accepts it, as `Raw` with `project: Option<Spanned<PathBuf>>`.
  - **Precedence:**
    - If `read(committed)` returns `Some`, the committed file is used, and a managed file
      that `read` also finds goes into the ignored map as `Committed`.
    - Otherwise, if `read(managed)` returns `Some(Err)`, or text that fails
      `parse_managed`, the site loads as `Managed` with that error.
    - Otherwise, if `project == entry.path`, the site loads as `Managed` with that file.
    - Otherwise the managed file goes into the ignored map as `OtherProject`.
    - Proxies (no `path`) get nothing.
  - **`sites::resolve`** takes `projects: &Projects` and `ignored: &IgnoredFiles`.
    It fills `project_file` from `Loaded.file`, `project_file_kind` from `Loaded.kind`, and
    `ignored_managed_file` from the ignored map (with `project` as a display string).
  - Bump `PROTOCOL_VERSION` to `"2.6"` and add the proto types from Interfaces.

- [ ] **Step 4: Write the failing daemon test** in `crates/yard-daemon/tests/project.rs`:

```rust
#[tokio::test]
async fn managed_file_is_served_and_committed_wins() {
    let d = prepared("").await;
    d.fx.add_php("7.3");
    d.fx.add_php("7.4");
    let dir = d.project("ee"); // linked as `ee` by prepared's sites.toml helper
    let managed = d.fx.paths.managed_project_file("ee");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, format!("php = \"7.4\"\nproject = {:?}\n", dir.display().to_string())).unwrap();
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "ee").await;
    assert_eq!((s["php"].as_str(), s["project_file"].as_str(), s["project_file_kind"].as_str()),
        (Some("7.4"), Some(managed.to_str().unwrap()), Some("managed")), "{s}");
    write_yard(&dir, "php = \"7.3\"\n");
    reconcile(&mut c).await;
    let s = site(&mut c, "ee").await;
    assert_eq!((s["php"].as_str(), s["project_file_kind"].as_str()), (Some("7.3"), Some("project")), "{s}");
    assert_eq!(s["ignored_managed_file"]["because"], "committed", "{s}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_dangling_committed_symlink_does_not_fall_through() {
    // dir/.yard -> dir/nowhere (symlink), plus a valid matching managed file:
    // the site's project_error is PROJECT_FILE_INVALID naming dir/.yard, and project_file_kind is "project".
}
```

- [ ] **Step 5: Implement the daemon side.**
  - **`read_project_file`:** when `metadata` is `NotFound` but `symlink_metadata` is `Ok`,
    return `Some(Err(PROJECT_FILE_INVALID "<file> is a symlink to nothing"))`.
  - **`read_projects`** returns `load(sites, paths, &read_project_file)`.
  - **`Snapshot`** stores both maps: `projects` and `ignored_managed`.
  - **Pattern updates:** replace `Some(Ok(file)) = projects.get(..)` with
    `projects.get(..).and_then(Loaded::ok)`.
  - **Use `Loaded.file`, not `path.join(FILE)`,** in these hints:
    - `php.uninstall`'s in-use hint (`rpc/php.rs:121`);
    - `services.remove`'s `files` (`rpc/services.rs:100`).
  - **`info.rs`** follows the same rule.

- [ ] **Step 6: Run the whole suite.**

Run: `cargo test --workspace` (redirect output to a file and read its tail)
Expected: all pass, including every existing project-file test unchanged.

- [ ] **Step 7: Stage.**

```bash
git add crates/yard-core crates/yard-proto crates/yard-daemon
```

The controller commits with: `read a site's .yard from Yard's data root when the project has none`.

---

### Task 2: Watch managed files

**Files:**
- Modify: `crates/yard-daemon/src/watch.rs` (managed directories alongside `project_dirs`)
- Modify: `crates/yard-daemon/src/daemon.rs` (create `managed_sites_dir()` at startup, as other data-root dirs are)
- Test: `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Consumes: `Paths::managed_sites_dir`, `Paths::managed_project_file` (Task 1).
- Produces:
  - `fn managed_dirs(paths: &yard_core::Paths) -> BTreeSet<PathBuf>`: canonical; holds
    `managed_sites_dir()` plus each `sites/<name>/` that exists, for every name in
    `sites.toml`.
  - `fn is_managed_file(path: &Path, dirs: &BTreeSet<PathBuf>) -> bool`: true for a
    `.yard` whose parent is in `dirs`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn a_managed_file_written_later_is_applied() {
    // Linked `ee` with no file, daemon started, then remove_dir_all(paths.managed_sites_dir())
    // to cover a missing sites/, then create sites/ee/.yard with php 7.4 + matching project.
    // php_within(&mut c, "ee", "7.4", Duration::from_secs(5)) succeeds.
}

#[tokio::test]
async fn removing_the_committed_file_falls_back_to_the_managed_one() {
    // committed php 7.3 + managed php 7.4; delete dir/.yard; php_within(.., "7.4", 5 s).
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo test -p yard-daemon --test project managed`. Expected: the first test times out waiting for 7.4.

- [ ] **Step 3: Implement it.**
  - Watch `managed_dirs` alongside `projects`. When `sites.toml` changes, or a changed path
    is the managed directory or lies under it, recompute `managed_dirs` and
    `change_watches` the difference.
  - A change is relevant when `is_managed_file`.
  - Recompute before testing relevance, so a `sites/<name>/` created by the same event
    batch is watched.

- [ ] **Step 4: Run the tests until they pass,** then run `cargo test -p yard-daemon --test project` three times. Expected: all pass each time.

- [ ] **Step 5: Stage** `crates/yard-daemon`. Commit: `watch the managed .yard files`.

---

### Task 3: The shims follow the managed file

**Files:**
- Modify: `crates/yard-core/src/shim.rs:38-76` (`resolve_version` gains `paths`; `Source::Managed`)
- Modify: `crates/yard-shim/src/main.rs:61` (pass `&paths`)
- Test: `crates/yard-core/src/shim.rs` (unit), `crates/yard-shim/tests/shim.rs`

**Interfaces:**
- Consumes: `project::parse_managed`, `Paths::managed_project_file` (Task 1).
- Produces:
  - `pub fn resolve_version(cwd: &Path, yard: &YardConfig, sites: &SitesConfig, paths: &Paths, read: &dyn Fn(&Path) -> Option<String>) -> Result<(PhpVersion, Source), YardError>`.
  - `Source::Managed(PathBuf)`, which `Display`s as the path.

- [ ] **Step 1: Write the failing unit tests:**

```rust
#[test]
fn managed_file_sits_between_committed_and_isolation() {
    // sites: old -> /code/old (php = "7.1" isolation). paths = Paths::new("/r").
    // /r/sites/old/.yard = "php = \"7.3\"\nproject = \"/code/old\"\n"
    assert_eq!(at("/code/old/app"), (v("7.3"), Source::Managed("/r/sites/old/.yard".into())));
    // plus /code/old/app/.yard = php 7.4 -> committed nearest wins: (7.4, Source::Project(..))
    // managed project = "/code/elsewhere" -> falls to isolation: (7.1, Source::Site("old"))
}

#[test]
fn bad_managed_file_names_it() {
    // /r/sites/old/.yard = "php = 7.3\n" -> Err whose message contains "/r/sites/old/.yard"
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo test -p yard-core shim::tests`. Expected: compile error on the new parameter.

- [ ] **Step 3: Implement it.**
  - **Step 2 of the shim order (spec §4):** take the innermost site, picked as today.
    Read its managed file. If `parse_managed` succeeds and `project` equals the site's
    `path`, return the file's `php` or the default, with `Source::Managed`.
  - A parse error is returned as the error.
  - Otherwise, fall through to that site's isolation.

- [ ] **Step 4: Add a `yard-shim` integration test,** `managed_file_php_is_run`, mirroring
  `project_file_php_is_run`. It writes `$YARD_ROOT/sites/<name>/.yard`, runs the shim in
  the project, and checks the fake PHP ran the 7.3 keg.

- [ ] **Step 5: Run.** Run: `cargo test -p yard-core shim && cargo test -p yard-shim`. Expected: PASS.

- [ ] **Step 6: Stage** `crates/yard-core crates/yard-shim`. Commit: `let the shims run the managed .yard's php`.

---

### Task 4: `yard init` writes the managed file; `--local`; `yard link` adopts it

**Files:**
- Modify: `crates/yard-proto/src/methods.rs` (`InitParams.local`), `crates/yard-proto/src/types.rs` (`InitResult` fields), `crates/yard-proto/src/error.rs` (`ProjectFileExists`)
- Modify: `crates/yard-core/src/project.rs` (`render_managed`, `set_project`, `to_committed`)
- Modify: `crates/yard-daemon/src/rpc/project.rs` (the table in spec §5), `crates/yard-daemon/src/rpc/handlers.rs:209-219` (`link`)
- Modify: `crates/yard-cli/src/cli.rs:57-71` (`--local`, help text), `crates/yard-cli/src/commands/init.rs`, the CLI's `link` output, and `crates/yard-cli/tests/cli.rs` (`MockDaemon`)
- Test: `crates/yard-core/src/project.rs`, `crates/yard-daemon/tests/project.rs`, `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `Loaded`, `FileKind`, `Ignored`, `managed_header`, `Paths::managed_project_file` (Task 1).
- Produces:
  - `InitParams { …, #[serde(default)] pub local: bool }`.
  - On `InitResult`, both `#[serde(default, skip_serializing_if = "Option::is_none")]`:
    - `pub from_managed: Option<String>`: the managed file `--local` moved;
    - `pub backup: Option<String>`: the `.yard-backup` a replace kept.
  - `ErrorCode::ProjectFileExists`, which serializes as `"PROJECT_FILE_EXISTS"`.
  - `project::render_managed(name: &str, project: &Path, d: &Detected) -> String`: the
    managed header, `project = "<path>"`, then what `render` writes after its header.
  - `project::set_project(text: &str, project: &Path) -> Result<String, YardError>`: sets
    or replaces `project`, using `toml_edit`.
  - `project::to_committed(text: &str) -> Result<String, YardError>`:
    - drops `project`;
    - replaces a first line that starts with `MANAGED_HEADER_PREFIX` by `HEADER`, and
      prepends `HEADER` when there is none;
    - keeps everything else.

- [ ] **Step 1: Write failing core tests** for `render_managed`, `set_project` and
  `to_committed`. Each round-trips through `parse_managed` or `parse`. `to_committed` keeps
  a user comment and `[services]`.

- [ ] **Step 2: Rewrite the existing init tests' expectations, and write the new failing daemon tests.**
  - The existing init tests check `<project>/.yard`, so add `"local": true` to their
    params. Rename none of them.
  - Write these new tests:
    - **`init_writes_the_managed_file_by_default`:** `file` is
      `paths.managed_project_file("ee")`. The text starts with
      `# .yard for ee, kept by Yard outside the project.` and contains
      `project = "<dir>"`. `<project>/.yard` doesn't exist, and `site.project_file_kind`
      is `"managed"`.
    - **`init_applies_an_existing_managed_file`:** run twice, the second call returns
      `written: false` with the same text.
    - **`init_local_moves_the_managed_file`:**
      - `<project>/.yard` starts with `HEADER` and has no `project` key.
      - The managed file is gone, and `from_managed` names it.
      - With a committed file already there, the call fails with `PROJECT_FILE_EXISTS`
        and writes nothing.
    - **`init_force_local_moves_then_rewrites`:** `extensions = ["redis"]` survives.
    - **`init_with_a_committed_file_ignores_the_managed_one`:** `--force` without
      `--local` leaves the managed bytes unchanged.
    - **`init_refuses_a_symlinked_managed_file`:** run with `--force`.
    - **`init_writes_project_as_sites_toml_spells_it`:**
      - `sites.toml` links `ee` at a symlinked spelling of the directory (`/tmp/…/link`
        → real dir).
      - `init`, run in the real directory, finds the link through `canonicalize`.
      - The managed file's `project` is the symlinked spelling, and the site is served
        from it. This is Review Focus 1.
    - **`init_replace_backs_up_another_projects_managed_file`:**
      - The name `app` is linked at dir A, and `sites/app/.yard` has `project = A`.
      - `init app` in dir B without `replace` is `SITE_NAME_TAKEN`.
      - With `replace`, `sites/app/.yard-backup` holds the old bytes, the new file has
        `project = B`, and `backup` names the backup.
    - **`link_adopts_a_matching_managed_file_and_names_another`:**
      - `link` with a matching managed file serves it, and drops `php` from the
        `sites.toml` entry.
      - After `unlink`, `link` of the same name in another directory serves the default,
        and `ignored_managed_file.because == "other_project"`. This is Review Focus 4.

- [ ] **Step 3: Run them to see them fail.** Run: `cargo test -p yard-daemon --test project init link`. Expected: the new tests fail; the edited old ones pass.

- [ ] **Step 4: Implement `sites.init` (spec §5).**
  - **Paths.** `committed = dir/.yard`, `managed = paths.managed_project_file(&name)`,
    and `site_path` is the entry's `path` if the site is linked, else `dir`.
  - **Dispatch:**

    | Committed | Managed file                | `local` | Action                                     |
    |-----------|-----------------------------|---------|--------------------------------------------|
    | present   | any                         | any     | today's path, unchanged                    |
    | absent    | absent                      | false   | write `render_managed`                     |
    | absent    | absent                      | true    | write `render` into the project            |
    | absent    | for this site               | false   | apply; with `force`, rewrite then `set_project` |
    | absent    | for this site               | true    | `to_committed`, write to `committed`, delete `managed`; then `rewrite` when `force` |
    | absent    | for another project or none | any     | `SITE_NAME_TAKEN` unless `replace`         |

  - **Replace.** On `replace`, copy the old file to `<managed>.yard-backup`, then write a
    fresh `render_managed`.
  - **Writes.** Every write uses `write_project_file`, which refuses symlinks. Create the
    `sites/<name>/` directory first, with `create_dir_all`.
  - **`sites.toml`.** The edit is unchanged: drop `php`, `secure` and `docroot`, and link.
  - **`carry`.** It keeps working for the committed path. For a site moved by name, it
    still feeds `detect`.
  - **`link`.** After linking, if the managed file loads for this site (`snapshot`
    projects shows kind `Managed`), drop `php`, `secure` and `docroot` from the entry, then
    reconcile again.
  - **CLI.**
    - `yard init --local`. The help reads "Write .yard into the project instead of Yard's
      data root (sites/<name>/.yard)".
    - Print `Wrote <path>:`, or for a move `Moved <from> to <path>:`.
    - `yard link` prints `Using <path>` when `project_file_kind` is `managed`, and
      `<path> is for <project>, so it is not used here` for `ignored_managed_file`
      `other_project`.

- [ ] **Step 5: Run.** Run: `cargo test --workspace`. Expected: PASS. Run `cargo clippy --workspace --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 6: Stage** `crates/`. Commit: `write the managed .yard from yard init, and --local for the project's own`.

---

### Task 5: `isolate`, `secure` and `docroot` edit the managed file

**Files:**
- Modify: `crates/yard-daemon/src/rpc/handlers.rs:152-164,251-290` (`refuse_project_file` becomes `project_file_target`), `crates/yard-daemon/src/rpc/php.rs:173-205` (`isolate`)
- Create: `crates/yard-daemon/src/managed.rs` (`edit_managed`)
- Test: `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Consumes: `read_project_file`, `project::parse_managed`, `TomlFile::edit` (shares its lock).
- Produces:
  - `pub(super) enum Target { SitesToml, Managed(PathBuf) }`.
  - `pub(super) fn project_file_target(ctx: &Context, site: &str, change: &str) -> Result<Target, YardError>`:
    - `SITE_HAS_PROJECT_FILE` for a committed file, exactly as `refuse_project_file` does today;
    - `Managed` when the site resolves from its managed file;
    - else `SitesToml`.
  - `pub fn edit_managed(file: &Path, f: impl FnOnce(&mut TomlFile) -> Result<(), YardError>) -> Result<(), YardError>`:
    - refuses a symlink, with `INVALID_ARGUMENT` and the hint `edit <file>`;
    - reads with `read_project_file`, then runs `parse_managed`, which fails with
      `PROJECT_FILE_INVALID` at its line;
    - runs `TomlFile::edit(file, …)`, applying `f` and then `parse_managed` on the result
      before it is saved.

- [ ] **Step 1: Write the failing tests:**
  - **`isolate_edits_the_managed_file_and_keeps_comments`:**
    - The managed file is `# keep me\nphp = "7.3"\nproject = …`.
    - `php.isolate {site, php: "7.4"}` leaves the text with `# keep me` and
      `php = "7.4"`.
    - The site serves 7.4, and `sites.toml` has no `php` for it.
  - **`unisolate_secure_unsecure_docroot_edit_the_managed_file`:**
    - `unisolate` removes `php`.
    - `secure` sets `secure = true` and `unsecure` sets `secure = false`.
    - `docroot "web"` sets `docroot = "web"`, and `docroot` with no path removes it.
  - **`isolate_refuses_a_broken_managed_file`:** the file is `php = 7.3` (no quotes). The
    call fails with `PROJECT_FILE_INVALID` and the bytes are unchanged. This is Review
    Focus 2.
  - **`isolate_to_a_missing_version_leaves_the_managed_file`:** `7.2` is not installed.
    The error is the same code as today, and the bytes are unchanged. This is Review
    Focus 3.
  - **`docroot_outside_the_project_is_refused_for_a_managed_file`:** `../x` gives
    `INVALID_ARGUMENT` or `PROJECT_FILE_INVALID`, and the bytes are unchanged.
  - **`commands_still_refuse_a_committed_file`:** this exists today. Keep it, adding a
    managed file next to the committed one.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo test -p yard-daemon --test project isolate secure docroot`. Expected: the new tests fail with `SITE_HAS_PROJECT_FILE`.

- [ ] **Step 3: Implement it.** Each handler calls `project_file_target`.
  - On `Managed(file)`, it runs `edit_managed` with the edit from the spec §4 table, then
    `reconcile` and `site`.
  - On `SitesToml`, today's `sites.toml` edit is unchanged.
  - Validation happens before any write:
    - `isolate` checks the version is installed;
    - `docroot` checks the path via `parse_managed` on the edited text.

- [ ] **Step 4: Run.** Run: `cargo test --workspace`. Expected: PASS.

- [ ] **Step 5: Stage** `crates/yard-daemon`. Commit: `edit the managed .yard from isolate, secure and docroot`.

---

### Task 6: Doctor checks, `yard info` and `yard sites`

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs` (three checks)
- Modify: `crates/yard-proto/src/types.rs` (`SettingSource::ManagedFile`), `crates/yard-daemon/src/rpc/info.rs:240-250`
- Modify: `crates/yard-cli/src/commands/info.rs:46-90` (sources and the `config` line)
- Test: `crates/yard-daemon/tests/doctor.rs`, `crates/yard-cli/tests/cli.rs` (snapshot)

**Interfaces:**
- Consumes: `SiteInfo.project_file_kind`, `SiteInfo.ignored_managed_file`, `Paths::managed_sites_dir` (Tasks 1 and 4).
- Produces:
  - `SettingSource::ManagedFile`, which serializes as `"managed_file"`.
  - Three doctor checks:

    | Id                              | Level | Message                                                                    | Hint                                    |
    |---------------------------------|-------|----------------------------------------------------------------------------|-----------------------------------------|
    | `managed-file-ignored:<site>`   | warn  | `<site> has a committed .yard, so <file> is ignored`                       | `rm <file>`                             |
    | `managed-file-mismatch:<name>`  | warn  | `<file> is for <project or "no project">, not <site path>`                 | `cd <site path> && yard init --replace` |
    | `managed-file-orphan:<name>`    | info  | `<file> belongs to no linked site`                                         | `yard link <name>`                      |

  - The orphan check is found by listing `managed_sites_dir()`. It covers names with no
    site, and names that are proxies.

- [ ] **Step 1: Write the failing tests:**
  - one doctor test per check id, asserting its level and its hint;
  - an `insta` snapshot `info_managed_snapshot`, showing `config  managed (~/…/sites/ee/.yard)`
    and `php  7.4  from managed`.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo test -p yard-daemon --test doctor managed && cargo test -p yard-cli info_managed`. Expected: FAIL.

- [ ] **Step 3: Implement it.**
  - `info.rs` maps a setting the file sets to `ManagedFile` when the kind is `Managed`.
  - The CLI names the source `managed`, and the `config` line reads either
    `managed (<tilde path>)` or `.yard`.

- [ ] **Step 4: Run.** Run: `cargo test --workspace`. Review the new snapshot against spec §6, then accept it.

- [ ] **Step 5: Stage** `crates/`. Commit: `report managed .yard files in the doctor and yard info`.

---

### Task 7: Docs and the real-machine check

**Files:**
- Modify: `docs/superpowers/specs/2026-10-22-yard-project-file-design.md` (§2, §3 anti-goal, §4, §5, §6, §7, per the managed spec §7)
- Modify: `docs/superpowers/specs/2026-10-19-yard-design.md` (the "nothing is written into the project" decision, §4.5, the data root layout, §6.2 for the new param and error code)
- Modify: `AGENTS.md` ("Sites are explicit", Status, Build & test), `README.md`, `.hydra/rules/config-ownership.md`, `.hydra/rules/cli-output.md`
- Create: `scripts/e2e-m4a1.sh`

- [ ] **Step 1: Update the docs.**
  - Grep the docs for `yard init`, `.yard`, `SITE_HAS_PROJECT_FILE`, `project_file` and
    `2.5`, and fix each stale line.
  - `config-ownership.md` gains the managed file's rules: who writes it, the symlink
    refusal, the `.yard-backup` on replace, and that `unlink` never deletes it.

- [ ] **Step 2: Write `scripts/e2e-m4a1.sh`,** modelled on `scripts/e2e-m4a.sh`.
  - **Setup:** a scratch git repo under `$TMPDIR`, never under `~/Herd`. Back up and
    restore `sites.toml` with a trap.
  - **Init:** run `yard init --yes`, then check:
    - `git status --porcelain` is empty;
    - `yard info` shows `managed`;
    - curl shows the PHP version (`php = "7.1"` via `yard isolate 7.1`, after the init's
      default);
    - the managed file still has its comment.
  - **Committed file:** commit a `.yard` with `php = "8.4"`, then check that curl serves
    8.4 within 5 s and that `yard doctor` reports `managed-file-ignored`.
  - **Move:** remove the committed file and run `yard init --local`. Check that the
    project's `.yard` exists and the managed file is gone.
  - **Teardown:** `yard unlink`, and remove the scratch nginx override and managed
    directory.
  - `shellcheck scripts/e2e-m4a1.sh` stays clean.
  - Do not run it: the controller asks the maintainer.

- [ ] **Step 3: Run** `cargo test --workspace`, clippy and `cargo fmt --check`. Expected: all clean.

- [ ] **Step 4: Stage** the docs and script. Commit: `document managed .yard files and add their real-machine check`.
