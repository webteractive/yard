# M4c Node Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `node`, `npm` and `npx` run the Node version each project pins (`.yard`, the
site's setting, `.nvmrc`, `.node-version`, else the default). Versions come from brew's
`fnm`, kept in `<data root>/node`, and are managed with `yard node install | uninstall | list
| default | use | unuse`.

**Architecture:**
- **Pure logic in `yard-core::node`.** Version and spec types, LTS names, matching a spec
  against installed or remote versions, and parsing `fnm ls-remote`.
- **Resolution beside PHP's.** It lives in `yard-core::shim` and shares the
  innermost-site walk with `resolve_version`.
- **The shim needs no daemon.** It reads the version directories itself, and never runs fnm.
- **yardd runs fnm** (`<brew prefix>/bin/fnm`) only in `node.install` and `node.uninstall`
  jobs, and for `node.list --available`.
- **A site's Node is worked out per request, not in the reconcile snapshot.** It is filled
  into each `SiteInfo` that `sites.list`, `sites.info`, the site-returning mutations and
  doctor hand out. Ruling: an `.nvmrc` edit or a Node install must not trigger a reconcile,
  and the watcher only sees each project root, while the spec walks up from it.

**Tech Stack:** Rust 1.92 workspace (`yard-proto`, `yard-core`, `yard-daemon`, `yard-cli`,
`yard-shim`), `toml`/`toml_edit`, serde, tokio, clap. The e2e uses brew's `fnm` against
nodejs.org.

**Spec:** `docs/superpowers/specs/2026-10-25-yard-m4c-node-design.md` (approved 2026-10-26;
it refines main spec §8.1 and the project-file spec's `node` key). Read it first: section
numbers below (§4, §5…) are its sections.

## Global Constraints

- **Where versions live:** `<data root>/node` is `FNM_DIR`, and a version lives at
  `node/node-versions/v<x.y.z>/installation/bin/<name>`. A version directory without
  `installation/bin/node` is not installed.
- **fnm:**
  - Its path is `<brew prefix>/bin/fnm`, absolute, never found through PATH.
  - It runs with `FNM_DIR=<root>/node` and `FNM_LOGLEVEL=info`, and `fnm install` with
    `--progress never`.
  - Yard always passes fnm an exact `v<x.y.z>`, never a major, an LTS name or `lts/*`.
  - When it is missing, only the `node.install` job installs it, with
    `<brew prefix>/bin/brew install fnm` (homebrew/core, no trust gate).
- **Never nvm's or Herd's Node:**
  - Yard never reads or writes `~/.nvm`, Herd's `config/nvm` or the user's shell files.
  - A shim never runs fnm or downloads anything.
  - A shim never falls through a pin it cannot meet.
- **Spec forms (§4):**
  - **Accepted:** `22`, `22.17`, `16.14.0`, `lts/<name>` (lowercased) and `lts/*`, with an
    optional leading `v`.
  - **Refused:** `node`, `stable`, `latest`, `system`, `iojs`, any range (`>=`, `^`, `~`,
    `x`, `*` outside `lts/*`, a space or `||`), and a major below 16.
- **The built-in default is `22`. The built-in LTS map** is `argon 4, boron 6, carbon 8,
  dubnium 10, erbium 12, fermium 14, gallium 16, hydrogen 18, iron 20, jod 22, krypton 24`.
- **Error codes:**
  - New: `NODE_MISSING` and `NODE_IN_USE`.
  - Unchanged, used for Node: `INVALID_ARGUMENT` for a bad spec from a command or from
    `.nvmrc`/`.node-version`, `PROJECT_FILE_INVALID` for a bad `node` in a `.yard`,
    `CONFIG_INVALID` for one in `yard.toml`/`sites.toml`, and `SITE_HAS_PROJECT_FILE`.
- **Protocol:** 2.6 → 2.7 (additive). The methods are `node.list`, `node.install`,
  `node.uninstall`, `node.default`, `node.use` and `node.unuse`.
- **The shim's missing-version error** is exactly
  `yard: Node <spec> is not installed (<where>)` then `  hint: yard node install <spec>`.
  `<where>` is the file's absolute path, `site <name> uses it`, or `the default`.
- **Doctor ids:** `node:<site>` (warn), `node-spec:<site>` (warn), `node-default` (warn).
  Doctor never runs fnm.
- **"Uses Node":** a linked site uses Node only when its root has a `package.json`, or
  resolution gives a spec from anywhere but the default. Only those sites get a `node`
  part, install offers, doctor checks and `NODE_IN_USE` protection.
- **Repo conventions:**
  - `yard-core` stays pure.
  - No `.unwrap()`/`.expect()` on daemon runtime paths.
  - Tests use a temp `YARD_ROOT` and never download Node.
  - Clippy `-D warnings` and `cargo fmt --check` stay clean.
  - Docs move with the code.
- **Commits:** a lowercase prose subject, no type prefix, no trailers. Implementers stage
  only, and the controller commits.

## Review Focus

1. **An `.nvmrc` as editors and nvm users write it.**
   - Example: `v16.14.0\r\n`, or `# pinned for node-sass\n\n16.14.0\n`.
   - Expected: both mean `16.14.0`. An `.nvmrc` with only comments or blank lines stops
     the search with `INVALID_ARGUMENT` naming the file.
   - Pinned in Task 1.
2. **A half-installed version.**
   - When: an fnm download fails or is cancelled midway, leaving `node-versions/v22.18.0/`
     without `installation/bin/node`.
   - Expected: the shims, `node.list` and `info` treat it as not installed, and a second
     `yard node install` succeeds.
   - Pinned in Tasks 1 and 7.
3. **`NODE_EXTRA_CA_CERTS` the user already set.**
   - A value the user set is kept.
   - An empty value counts as unset.
   - Nothing is set while Yard's CA does not exist.
   - Pinned in Task 4.
4. **A linked site whose directory was deleted.**
   - Expected: `sites.list`, `info` and doctor still answer. The site has no `node`
     part, and no other site loses its own.
   - Pinned in Task 6.
5. **An exact pin installed while nodejs.org cannot be listed.**
   - Expected: `yard node install 16.14.0` still runs `fnm install v16.14.0` after a job
     line saying the listing failed. A major or LTS spec fails with `JOB_FAILED`, since
     it cannot be resolved.
   - Pinned in Task 7.

---

### Task 1: `yard-core::node`: versions, specs, LTS names, matching, remote listings

**Files:**
- Create: `crates/yard-core/src/node.rs`
- Modify: `crates/yard-core/src/lib.rs` (`pub mod node;`), `crates/yard-core/src/paths.rs`
  (four helpers)
- Test: unit tests in `node.rs` and `paths.rs`

**Interfaces:**
- Produces, in `yard_core::node`:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)] pub struct NodeVersion { pub major: u32, pub minor: u32, pub patch: u32 }`
    - It implements `FromStr` (an optional leading `v`) with `Err = YardError`
      (`INVALID_ARGUMENT`), and `Display` as `22.17.1`.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum NodeSpec { Major(u32), Minor(u32, u32), Exact(NodeVersion), Lts(String), LtsLatest }`
    - `Display` writes the normalized form: `22`, `22.17`, `16.14.0`, `lts/iron`, `lts/*`.
    - `#[serde(try_from = "String", into = "String")]` (it implements `TryFrom<String>`
      with `Error = String`, and `From<NodeSpec> for String`), so config structs can hold it.
  - `pub const MIN_MAJOR: u32 = 16;` and `pub const DEFAULT_SPEC: NodeSpec = NodeSpec::Major(22);`
  - `pub fn parse_spec(s: &str) -> Result<NodeSpec, String>`. It trims, and the message
    says why a spec is refused.
  - `pub fn spec_line(text: &str) -> Option<(usize, &str)>`. It gives the 1-based line and
    the trimmed text of the first line that is neither blank nor a `#` comment.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct LtsMap(pub BTreeMap<String, u32>)`, with:
    - `pub fn builtin() -> LtsMap`;
    - `pub fn major(&self, spec: &NodeSpec) -> Option<u32>`, which answers `Lts` and
      `LtsLatest` (the highest major in the map) and is `None` for the other forms;
    - `pub fn parse(text: &str) -> Option<LtsMap>` (the lts.toml format; `None` when it
      does not parse);
    - `pub fn render(&self) -> String`.
  - `pub const LTS_HEADER: &str = "# Generated by Yard from nodejs.org's release list; rewritten by yard node install and yard node list --available.\n";`
  - `pub fn matches(spec: &NodeSpec, v: NodeVersion, lts: &LtsMap) -> bool`.
  - `pub fn pick(spec: &NodeSpec, installed: &[NodeVersion], lts: &LtsMap) -> Option<NodeVersion>`.
    It gives the newest match.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Release { pub version: NodeVersion, pub lts: Option<String> }`
  - `pub fn parse_ls_remote(text: &str) -> Vec<Release>`. Each line is `v<x.y.z>`, optionally
    followed by `(<Codename>)`. The codename is lowercased, and other lines are skipped.
  - `pub fn lts_from(releases: &[Release]) -> LtsMap`: the built-in map overlaid with every
    codename's major from `releases`.
  - `pub fn newest(spec: &NodeSpec, releases: &[Release], lts: &LtsMap) -> Option<NodeVersion>`.
    It ignores majors below 16.
  - `pub fn newest_per_line(releases: &[Release], major: Option<u32>) -> Vec<Release>`. It
    gives the newest release of each major ≥ 16, newest major first, only `major` when given.
  - `pub fn installed(versions_dir: &Path, list: &dyn Fn(&Path) -> Vec<String>, exists: &dyn Fn(&Path) -> bool) -> Vec<NodeVersion>`.
    It reads the `v<x.y.z>` directory names whose `installation/bin/node` exists, sorted
    ascending.
- Produces, in `yard_core::Paths`:
  - `pub fn node_dir(&self) -> PathBuf`: `<root>/node`.
  - `pub fn node_versions_dir(&self) -> PathBuf`: `<root>/node/node-versions`.
  - `pub fn node_bin(&self, v: NodeVersion, name: &str) -> PathBuf`:
    `<root>/node/node-versions/v<v>/installation/bin/<name>`.
  - `pub fn lts_toml(&self) -> PathBuf`: `<root>/node/lts.toml`.

- [ ] **Step 1: Write the failing tests** in `node.rs`'s test module:

```rust
#[test]
fn specs_parse_to_their_normal_form() {
    for (input, shown) in [
        ("22", "22"), ("v22", "22"), (" 22.17 ", "22.17"), ("16.14.0", "16.14.0"),
        ("v16.14.0", "16.14.0"), ("lts/iron", "lts/iron"), ("lts/Iron", "lts/iron"), ("lts/*", "lts/*"),
    ] {
        assert_eq!(parse_spec(input).unwrap().to_string(), shown, "{input}");
    }
}

#[test]
fn specs_yard_refuses_say_why() {
    for (input, says) in [
        ("node", "not a version"), ("stable", "not a version"), ("latest", "not a version"),
        ("system", "not a version"), ("iojs", "not a version"),
        (">=18", "range"), ("^20", "range"), ("20.x", "range"), ("18 || 20", "range"),
        ("14", "older than 16"), ("14.21.3", "older than 16"), ("banana", "not a Node version"), ("", "not a Node version"),
    ] {
        let e = parse_spec(input).unwrap_err();
        assert!(e.contains(says), "{input}: {e}");
    }
}

#[test]
fn spec_line_skips_blanks_comments_and_crlf() {
    assert_eq!(spec_line("v16.14.0\r\n"), Some((1, "v16.14.0")));
    assert_eq!(spec_line("# pinned for node-sass\n\n16.14.0\n"), Some((3, "16.14.0")));
    assert_eq!(spec_line("# nothing\n\n"), None);
}

#[test]
fn pick_takes_the_newest_match() {
    let installed: Vec<NodeVersion> =
        ["16.14.0", "20.19.4", "22.17.1", "22.18.0", "24.5.0"].iter().map(|v| v.parse().unwrap()).collect();
    let lts = LtsMap::builtin();
    let at = |s: &str| pick(&parse_spec(s).unwrap(), &installed, &lts).map(|v| v.to_string());
    assert_eq!(at("22").as_deref(), Some("22.18.0"));
    assert_eq!(at("22.17").as_deref(), Some("22.17.1"));
    assert_eq!(at("16.14.0").as_deref(), Some("16.14.0"));
    assert_eq!(at("16.20.2"), None); // an exact pin means exactly that version
    assert_eq!(at("lts/iron").as_deref(), Some("20.19.4"));
    assert_eq!(at("lts/*").as_deref(), Some("24.5.0")); // krypton, the built-in map's highest
    assert_eq!(at("lts/argon"), None);
    assert_eq!(at("18"), None);
}

#[test]
fn lts_toml_round_trips_and_a_broken_one_is_none() {
    let map = lts_from(&parse_ls_remote("v26.1.0 (Lithium)\n"));
    let text = map.render();
    assert!(text.starts_with(LTS_HEADER), "{text}");
    assert_eq!(LtsMap::parse(&text), Some(map.clone()));
    assert_eq!(map.major(&NodeSpec::LtsLatest), Some(26));
    assert_eq!(map.major(&NodeSpec::Lts("iron".into())), Some(20));
    assert_eq!(LtsMap::parse("iron = "), None);
}

#[test]
fn ls_remote_listing_parses_and_newest_resolves() {
    let text = "v14.21.3 (Fermium)\nv16.14.0\nv16.20.2 (Gallium)\nv22.17.1\nv22.18.0 (Jod)\nv25.1.0\nsome banner\n";
    let releases = parse_ls_remote(text);
    assert_eq!(releases.len(), 6);
    assert_eq!(releases[4].lts.as_deref(), Some("jod"));
    let lts = lts_from(&releases);
    let newest = |s: &str| newest(&parse_spec(s).unwrap(), &releases, &lts).map(|v| v.to_string());
    assert_eq!(newest("22").as_deref(), Some("22.18.0"));
    assert_eq!(newest("16.14.0").as_deref(), Some("16.14.0"));
    assert_eq!(newest("lts/gallium").as_deref(), Some("16.20.2"));
    assert_eq!(newest("19"), None);
    let lines: Vec<String> = newest_per_line(&releases, None).iter().map(|r| r.version.to_string()).collect();
    assert_eq!(lines, ["25.1.0", "22.18.0", "16.20.2"]); // 14 is below the floor
    assert_eq!(newest_per_line(&releases, Some(16)).len(), 1);
}

#[test]
fn installed_needs_a_node_binary() {
    let dir = Path::new("/r/node/node-versions");
    let names = vec!["v22.18.0".to_string(), "v16.14.0".into(), "v20.0.0".into(), ".DS_Store".into()];
    let list = |_: &Path| names.clone();
    let exists = |p: &Path| !p.starts_with("/r/node/node-versions/v20.0.0"); // half-installed
    let got: Vec<String> = installed(dir, &list, &exists).iter().map(|v| v.to_string()).collect();
    assert_eq!(got, ["16.14.0", "22.18.0"]);
}
```

  In `paths.rs`, add one assertion each for the four helpers, e.g.
  `Paths::new("/r").node_bin("16.14.0".parse().unwrap(), "npm") == Path::new("/r/node/node-versions/v16.14.0/installation/bin/npm")`.

- [ ] **Step 2: Run** `cargo test -p yard-core node`. Expected: FAIL to compile (`node` module missing).

- [ ] **Step 3: Implement** `node.rs` and the `Paths` helpers to the interfaces above.
  - **lts.toml format:** `LTS_HEADER`, then one `<codename> = <major>` per line, sorted
    by major.
  - **Range detection** looks for any of `<>=^~| *` (a space or `*`), or an `x`/`X`
    component.
  - **What `parse_spec` says** (the tests match on these phrases):
    - for a refused word: `"<s>" is not a version: name one, such as 22`;
    - for a range: `"<s>" is a range; Yard runs one version, such as 22`;
    - below 16: `Node <major> is older than 16, the oldest Node built for Apple Silicon`;
    - otherwise: `"<s>" is not a Node version`.

- [ ] **Step 4: Run** `cargo test -p yard-core node paths`. Expected: PASS.

- [ ] **Step 5: Stage** both files and `lib.rs`. Commit:
  `add the Node version, spec and LTS logic Yard resolves pins with`.

---

### Task 2: The `node` key in `yard.toml`, `sites.toml` and `.yard`

**Files:**
- Modify: `crates/yard-core/src/config/yard.rs` (`default_node`)
- Modify: `crates/yard-core/src/config/sites.rs` (`SiteEntry.node`)
- Modify: `crates/yard-core/src/project.rs`:
  - `ProjectFile.node` and `Raw.node`;
  - `Detected.node`, `detect`, `render_keys` and `rewrite`;
  - the `unknown_key_names_file_and_line` test, which used `node` as its unknown key.
- Test: unit tests in those three files

**Interfaces:**
- Consumes: `NodeSpec`, `parse_spec` (Task 1).
- Produces:
  - `YardConfig.default_node: Option<NodeSpec>`. A bad value is `CONFIG_INVALID` at its line,
    through the existing `config_error` path.
  - `SiteEntry.node: Option<NodeSpec>`, the same way.
  - `ProjectFile.node: Option<NodeSpec>`. A bad value is `PROJECT_FILE_INVALID` at the
    value's line: `Raw.node` is `Option<Spanned<String>>` and goes through `parse_spec`
    and `invalid(Some(span), …)`.
  - `Detected.node: Option<NodeSpec>`. `detect` takes it from `entry.node` only, and never
    from `.nvmrc` (spec §8).
  - `render_keys` writes `node = "<spec>"` right after `php` when it is set, and `rewrite`
    sets `node` only when it is detected, so `--force` keeps a `node` already in the file.

- [ ] **Step 1: Write the failing tests:**
  - In `project.rs`:
    - `node_key_parses_and_a_bad_one_names_its_line`: `"php = \"7.3\"\nnode = \"16.14.0\"\n"`
      gives `node == Some(Exact(16.14.0))`. `"php = \"7.3\"\nnode = \"latest\"\n"` is
      `PROJECT_FILE_INVALID` with `data.line == 2` and a message containing `latest`.
    - Replace `unknown_key_names_file_and_line`'s unknown key with `nodejs = "20"`.
    - `render_and_rewrite_carry_node`: `render` of a `Detected` with `node: Some(Major(16))`
      contains `php = "7.4"\nnode = "16"\ndocroot`. `rewrite` of `"node = \"18\"\n"` with
      `node: None` keeps `node = "18"`, and with `Some(Major(16))` makes it `16`.
    - `detect_takes_node_from_the_site_only`: with an entry `node = "16"` it gives
      `Some(Major(16))`, and with none, `None`.
  - In `yard.rs`: `default_node_parses_and_a_bad_one_is_config_invalid`
    (`default_node = "14"` is `CONFIG_INVALID` naming `yard.toml` and line 1).
  - In `sites.rs`: `site_node_is_parsed` (`node = "lts/iron"`).

- [ ] **Step 2: Run** `cargo test -p yard-core project config`. Expected: FAIL to compile.

- [ ] **Step 3: Implement** the fields and the parsing. Fix every `Detected { … }` and
  `SiteEntry { … }` literal the compiler names, including `carry` in
  `crates/yard-daemon/src/rpc/project.rs`, which gains `node: other.node.clone().or(old_file.and_then(|f| f.node.clone()))`.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.

- [ ] **Step 5: Stage.** Commit: `read a node spec from yard.toml, sites.toml and .yard`.

---

### Task 3: Which spec a directory gets

**Files:**
- Modify: `crates/yard-core/src/shim.rs`
- Test: unit tests in `shim.rs`

**Interfaces:**
- Consumes: Task 1 and Task 2's fields.
- Produces, in `yard_core::shim`:
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum NodeSource { Project(PathBuf), Managed(PathBuf), Site(String), Nvmrc(PathBuf), NodeVersionFile(PathBuf), Default }`.
    `Display` gives the path for the four file variants, `site <name> uses it` and
    `the default`.
  - `pub fn resolve_node(cwd: &Path, yard: &YardConfig, sites: &SitesConfig, paths: &Paths, read: &dyn Fn(&Path) -> Option<Result<String, YardError>>, canonical: &dyn Fn(&Path) -> PathBuf) -> Result<(NodeSpec, NodeSource), YardError>`.
  - `pub fn uses_node(source: &NodeSource, has_package_json: bool) -> bool`: true when
    `has_package_json`, or the source is not `Default`.
  - A private `fn innermost<'a>(cwd: &Path, sites: &'a SitesConfig, canonical: &dyn Fn(&Path) -> PathBuf) -> Vec<(&'a String, &'a SiteEntry, &'a Path)>`,
    extracted from `resolve_version` and used by both. `resolve_version`'s behaviour and
    tests do not change.

The order, which follows spec §5:
1. **The nearest `.yard`**, walking from `cwd` up, whose `node` is set. A `.yard` without
   `node` is passed over.
2. **The innermost linked site's own setting**, skipped when `<site path>/.yard` exists
   (`read` gives `Some`):
   - when the site's managed file is for the site's path (its `project` matches), that
     file's `node`, or nothing at all when the file has no `node`. `sites.toml` is then
     not read, so the walk goes on to step 3.
   - else the site's `node` in `sites.toml` (`Site`).
3. **The nearest `.nvmrc` or `.node-version`**, walking up. In one directory `.nvmrc`
   wins, and a `.nvmrc` that is a directory is skipped (`read` gives `None`).
4. **The default:** `yard.default_node`, else `DEFAULT_SPEC`.

Errors:
- `read` giving `Err` stops the walk with that error.
- A `.yard` that does not parse is its parse error.
- An `.nvmrc`/`.node-version` whose `spec_line` is `None`, or whose spec `parse_spec`
  refuses, is `INVALID_ARGUMENT`:
  - message `<file> line <n>: <parse_spec message>`, or `<file> names no version`;
  - hint `put a version such as 22 in <file>`.

- [ ] **Step 1: Write the failing tests**, reusing the module's `configs`, `same`, `SITES`
  and a `BTreeMap` read closure:
  - `node_resolution_order`. Sites: `old` at `/code/old` with `node = "18"`, and `plain` at
    `/code/plain`. Files:
    - `/code/.nvmrc` = `16.14.0`;
    - `/code/old/.yard` = `php = "7.1"` (no node);
    - `/r/sites/plain/.yard` = managed, `project = "/code/plain"`, `node = "20"`.

    Assert:
    - `/code/old/app` gives `(Exact(16.14.0), Nvmrc("/code/.nvmrc"))`, not `old`'s
      `node = "18"`. The `.yard` without `node` passes over in step 1, and its presence
      skips step 2. A comment in the test says so.
    - The same cwd with `/code/old/.yard` removed gives `(Major(18), Site("old"))`.
    - `/code/plain/src` gives `(Major(20), Managed(..))`.
    - `/elsewhere` gives `(Major(22), Default)`.
    - With `default_node = "24"` in `yard.toml`, `/elsewhere` gives `Major(24)`.
  - `a_yard_with_node_beats_everything`: `/code/old/.yard` = `node = "25"` gives
    `Project(..)`, 25.
  - `nvmrc_beats_node_version_in_one_dir_and_nearer_wins`.
  - `an_unusable_nvmrc_stops_the_search`. A `.nvmrc` holding `lts/*\n` works. One holding
    `node\n`, or only `# x\n`, is `INVALID_ARGUMENT` naming `/code/.nvmrc`. A read closure
    returning `Some(Err(..))` for `/code/.nvmrc` returns that error, even though
    `/.node-version` exists.
  - `uses_node_table`: `Default` + no package.json is false; `Default` + package.json is
    true; `Nvmrc` + none is true.

- [ ] **Step 2: Run** `cargo test -p yard-core shim`. Expected: FAIL to compile.

- [ ] **Step 3: Implement** `resolve_node`, `NodeSource`, `uses_node` and the shared
  `innermost`.

- [ ] **Step 4: Run** `cargo test -p yard-core shim`. Expected: PASS, including
  `resolve_version`'s existing tests.

- [ ] **Step 5: Stage.** Commit: `resolve which Node spec a directory asks for`.

---

### Task 4: The `node`, `npm` and `npx` shims

**Files:**
- Modify: `crates/yard-core/src/shim.rs`:
  - `PHP_SHIMS`, `NODE_SHIMS` and `SHIMS`;
  - `node_command`.
- Modify: `crates/yard-proto/src/error.rs` (`NodeMissing` and `NodeInUse`, documented).
- Modify: `crates/yard-shim/src/main.rs`:
  - a dispatch on the name, then `run_node`;
  - `read_small`'s error code for `.nvmrc`/`.node-version`;
  - the module doc.
- Modify: `crates/yard-daemon/src/reconcile.rs` (`link_shims`'s warning says "the shims").
- Test: `crates/yard-shim/tests/node.rs` (new), unit tests in `shim.rs`,
  `crates/yard-daemon/tests/reconcile.rs`

**Interfaces:**
- Consumes: `resolve_node`, `NodeSource` (Task 3), `pick`, `installed`, `LtsMap` and the
  `Paths` helpers (Task 1).
- Produces:
  - `pub const PHP_SHIMS: [&str; 5] = ["php", "composer", "pecl", "phpize", "php-config"];`
  - `pub const NODE_SHIMS: [&str; 3] = ["node", "npm", "npx"];`
  - `pub const SHIMS: [&str; 8]`, which is both. Reconcile's `link_shims` already
    iterates `SHIMS`.
  - `pub fn node_command(name: &str, v: NodeVersion, paths: &Paths, inherited: &OsStr, extra_ca: Option<&OsStr>, exists: &dyn Fn(&Path) -> bool) -> ShimCommand`:
    - the program is `paths.node_bin(v, name)`, with no pre-args;
    - `PATH` is the version's `bin`, then `paths.bin_dir()`, then `child_path(None, inherited)`
      without either of those two;
    - `NODE_EXTRA_CA_CERTS` is set to `paths.ca_cert()` only when `extra_ca` is `None` or
      empty and `exists(ca_cert)`.
  - `ErrorCode::NodeMissing` ("a Node version a project or command asks for is not
    installed") and `ErrorCode::NodeInUse` ("a Node version is the only one the default or
    a linked site resolves to, so it cannot be removed"). Both get a serde round-trip
    assertion in `error.rs`'s tests.

In `yard-shim`:
- **Dispatch:** `run` dispatches by name. `NODE_SHIMS` goes to `run_node`, everything else
  to the existing PHP path.
- **Config `run_node` can't read:** a `yard.toml` that does not parse warns
  `yard: <message>; using Node 22` and uses the built-in default. A `sites.toml` that
  does not parse warns `yard: <message>; ignoring site settings`.
- **The LTS map:** `run_node` reads `paths.lts_toml()` with `read_small`, through
  `LtsMap::parse`, else `LtsMap::builtin()`.
- **Installed versions:** `installed(&paths.node_versions_dir(), …)`, listing with
  `std::fs::read_dir`.
- **`YARD_SHIM_DEBUG=1`** prints
  `yard-shim: <name> runs Node <version> (<spec> from <source>)`.
- **No match** is `NODE_MISSING`:
  - the message is `Node <spec> is not installed (<source>)`;
  - the hint is `yard node install <spec>`;
  - `main` prints both in the existing format.
- **`read_small`'s unusable-file code** is `PROJECT_FILE_INVALID` for a file named
  `.yard` and `INVALID_ARGUMENT` for every other name.

- [ ] **Step 1: Write the failing tests.**
  - In `shim.rs`: `node_command_puts_the_version_then_yard_first_and_drops_herd`.
    - Inherited is `/Users/x/Library/Application Support/Herd/bin:/usr/bin`.
    - The PATH entries are `[<root>/node/node-versions/v16.14.0/installation/bin, <root>/bin, /usr/bin]`.
    - With the CA present and `extra_ca` `None`, `NODE_EXTRA_CA_CERTS` is `<root>/ca/yard-ca.pem`.
    - With `Some("/mine.pem")` there is no such env entry, and with `Some("")` it is set.
    - With the CA absent, it is not set.
  - `crates/yard-shim/tests/node.rs`, modelled on `tests/shim.rs`. It sets up a temp root
    with `node-versions/v16.14.0/installation/bin/{node,npm}` scripts that print `argv0:$0`,
    each arg, `PATH:$PATH` and `CA:$NODE_EXTRA_CA_CERTS`. The project dir has
    `.nvmrc` = `v16.14.0\r\n`, and the shim is linked as `node` and `npm`.
    - `node_shim_runs_the_pinned_version_with_args_unchanged`: `node -e "x" "a b"`
      reaches `v16.14.0/installation/bin/node` with both args.
    - `npm_shim_runs_npm_with_the_version_bin_first`: the first PATH entry is the version's
      `bin`, and `HERD` never appears.
    - `a_missing_pin_names_the_file_and_the_install`: `.nvmrc` = `16.20.2`. The exit code is
      1 and stderr is exactly
      `yard: Node 16.20.2 is not installed (<project>/.nvmrc)\n  hint: yard node install 16.20.2\n`.
    - `a_half_installed_version_is_missing`: `v22.18.0/` exists without `installation/bin/node`,
      and `.nvmrc` = `22`. Expect the `NODE_MISSING` message, not an exec error.
    - `the_default_is_22_and_says_so`: with no pin and no 22 installed, the message ends
      `(the default)`.
    - `a_users_extra_ca_is_kept`: with `NODE_EXTRA_CA_CERTS=/mine.pem` set and the CA file
      present, the script prints `CA:/mine.pem`.
    - `debug_names_the_spec_and_its_file`.
  - In `crates/yard-daemon/tests/reconcile.rs`: extend the existing shim-link test (find
    it by `bin_dir`) so `bin/node`, `bin/npm` and `bin/npx` link to the shim too.

- [ ] **Step 2: Run** `cargo test -p yard-shim -p yard-core shim` and the reconcile test.
  Expected: FAIL.

- [ ] **Step 3: Implement** the constants, `node_command`, the error codes and `run_node`.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.

- [ ] **Step 5: Stage.** Commit: `add node, npm and npx shims that run the pinned version and never download`.

---

### Task 5: Protocol 2.7: the `node.*` types and a site's Node

**Files:**
- Modify: `crates/yard-proto/src/lib.rs` (`PROTOCOL_VERSION = "2.7"`),
  `crates/yard-proto/src/methods.rs`, `crates/yard-proto/src/types.rs`
- Modify: every exhaustive `match` on `SettingSource` the compiler names (the CLI's
  `info::source_name` gains `.nvmrc` and `.node-version`)
- Test: serde tests in `types.rs` and `methods.rs`

**Interfaces:**
- Produces, in `yard_proto::methods`:
  - `NODE_LIST = "node.list"`, `NODE_INSTALL = "node.install"`, `NODE_UNINSTALL = "node.uninstall"`,
    `NODE_DEFAULT = "node.default"`, `NODE_USE = "node.use"` and `NODE_UNUSE = "node.unuse"`.
  - `#[derive(Default)] pub struct NodeListParams { #[serde(default)] pub available: bool, #[serde(default)] pub major: Option<u32> }`.
    `null` deserializes to the defaults: the handler parses `Value::Null` as `Default`.
  - `pub struct NodeInstallParams { #[serde(default)] pub spec: Option<String>, #[serde(default)] pub path: Option<String> }`.
  - `pub struct NodeVersionParams { pub version: String }` (uninstall).
  - `pub struct NodeSpecParams { pub spec: String }` (default).
  - `pub struct NodeUseParams { pub site: String, pub spec: String }`.
  - `node.unuse` takes the existing `SiteParams`.
- Produces, in `yard_proto` types:
  - `SettingSource` gains `Nvmrc` and `NodeVersionFile` (serialized `nvmrc`,
    `node_version_file`), before `Other`.
  - `pub struct SiteNode`:
    - `pub spec: Option<String>`;
    - `pub source: SettingSource`;
    - `#[serde(default, skip_serializing_if = "Option::is_none")] pub file: Option<String>`;
    - `pub version: Option<String>`;
    - `#[serde(default, skip_serializing_if = "Option::is_none")] pub error: Option<YardError>`.

    Docs: `spec` is `None` only with `error`, and `version` is `None` while nothing
    installed matches.
  - `SiteInfo` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub node: Option<SiteNode>`,
    documented as "the Node the site root resolves to, for a site that uses Node; filled
    per request, so a `site_changed` event does not carry it".
  - `pub struct NodeInfo { pub version: String, pub default: bool, pub sites: Vec<String> }`.
  - `pub struct NodeRelease { pub version: String, pub lts: Option<String> }`.
  - `pub struct NodeList`:
    - `pub default: String` (the spec);
    - `pub versions: Vec<NodeInfo>`, newest first;
    - `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub available: Vec<NodeRelease>`.

- [ ] **Step 1: Write the failing tests.**
  - `site_node_round_trips_and_an_old_answer_has_none`: a `SiteInfo` JSON without `node`
    deserializes with `node: None`. A `SiteNode` with `source: Nvmrc` serializes
    `"source":"nvmrc"`.
  - `an_unknown_source_is_other` stays green.
  - `node_list_params_accept_null`.

- [ ] **Step 2: Run** `cargo test -p yard-proto`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.** Fix every `SiteInfo { … }` literal and `SettingSource`
  match across the workspace, then add the method list to main spec §6.2 (the
  `node.list|install|use` entry becomes the six methods).

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.

- [ ] **Step 5: Stage.** Commit: `add the node methods and a site's Node to protocol 2.7`.

---

### Task 6: yardd: a site's Node, `node.list`, `node.default`, `node.use`, `node.unuse`, and `init`

**Files:**
- Create: `crates/yard-daemon/src/node.rs`, the side effects: reading versions, `lts.toml`
  and pin files, and filling `SiteInfo.node`.
- Create: `crates/yard-daemon/src/rpc/node.rs`, the handlers.
- Modify:
  - `crates/yard-daemon/src/lib.rs`, `crates/yard-daemon/src/rpc/mod.rs`;
  - `crates/yard-daemon/src/rpc/handlers.rs`: dispatch; `SITES_LIST` and `site()` fill
    the Node part;
  - `crates/yard-daemon/src/rpc/info.rs`: `site_detail` fills it;
  - `crates/yard-daemon/src/rpc/project.rs`: `init` drops `node` from `sites.toml`.
- Modify: `crates/yard-daemon/tests/support/mod.rs` (`Fixture::add_node`)
- Test: `crates/yard-daemon/tests/node.rs` (new), `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces, in `yard_daemon::node`:
  - `pub fn installed(paths: &Paths) -> Vec<NodeVersion>`.
  - `pub fn lts(paths: &Paths) -> LtsMap`: `lts.toml` when it parses, else built-in.
  - `pub fn read_pin(file: &Path) -> Option<Result<String, YardError>>`. It is
    `crate::project::read_project_file`, except that a directory is `None` and any file
    other than `.yard` reports `INVALID_ARGUMENT`.
  - `pub fn resolve_at(dir: &Path, yard: &YardConfig, sites: &SitesConfig, paths: &Paths) -> Result<(NodeSpec, NodeSource), YardError>`:
    `shim::resolve_node` with `read_pin` and `std::fs::canonicalize`.
  - `pub fn fill_node(sites: &mut [SiteInfo], paths: &Paths)`:
    - It loads `yard.toml` and `sites.toml` and leaves every `node` as `None` when either
      fails.
    - It only fills a `SiteKind::Php` site whose path is a directory.
    - The site's `node` is `None` unless `uses_node(source, <path>/package.json is a file)`.
      A resolve error always fills `node` with `error` set.
    - `source` maps `NodeSource` onto `SettingSource`: `Project`→`ProjectFile`,
      `Managed`→`ManagedFile`, `Site`→`SitesToml`, `Nvmrc`, `NodeVersionFile`, `Default`.
      `file` is the path for the file variants.
  - `pub fn uses(site: &SiteInfo) -> Option<NodeSpec>`: the parsed spec of a site whose
    `node` has one.
- Produces, in `yard_daemon::rpc::node`:
  - `list(ctx, NodeListParams) -> Result<NodeList, YardError>`. Without `available` it
    never runs fnm. With it, it calls Task 7's `available`.
  - `set_default(ctx, NodeSpecParams) -> Result<(), YardError>`.
  - `use_spec(ctx, NodeUseParams) -> Result<SiteInfo, YardError>`.
  - `unuse(ctx, SiteParams) -> Result<SiteInfo, YardError>`.

What the handlers do:
- **`node.default`:**
  - It parses the spec, and a refused spec is `INVALID_ARGUMENT` with hint
    `yard node default 22`.
  - It needs `pick(spec, installed, lts)` to be `Some`, else `NODE_MISSING`
    `Node <spec> is not installed`, hint `yard node install <spec>`.
  - It sets `default_node` with `TomlFile::edit`. No reconcile is needed, but it is
    harmless.
- **`node.use` / `node.unuse`:**
  - They are `php::isolate`'s shape over `project_file_target(ctx, site, "node = \"<spec>\""`
    or `"remove node")`.
  - A committed `.yard` is `SITE_HAS_PROJECT_FILE`.
  - `Target::Managed` is edited with `edit_managed`.
  - Otherwise `sites.<name>.node` is set or removed. A proxy is `INVALID_ARGUMENT`
    `<name> is a proxy, so no Node runs it`.
  - The spec need not be installed.
  - They return `site(ctx, name)`, whose `node` is filled.
- **`node.list`:**
  - `versions` is the installed versions, newest first.
  - `default` is true on the version `pick(default spec)` gives.
  - `sites` are the names of filled sites whose `node.version` equals it.

Wiring:
- **The fill points:** `handlers::site` and `SITES_LIST` call `fill_node` on what they
  return, and `info::site_detail` calls it on its clone.
- **`init`:** the `for key in ["php", "secure", "docroot"]` loop in `rpc/project.rs`
  gains `"node"`. `detect` already carries `entry.node` (Task 2).
- **The fixture:** `Fixture::add_node(&self, v: &str)` creates
  `<root>/node/node-versions/v<v>/installation/bin/{node,npm,npx}` as empty files,
  mode 0755.

- [ ] **Step 1: Write the failing tests** in `crates/yard-daemon/tests/node.rs`. Use
  `prepared("").await`, then `d.fx.add_node(..)`, then `.start()`. Link projects through
  `sites.link` as `tests/project.rs` does.
  - `a_site_with_an_nvmrc_reports_its_node`:
    - Project `maker` has `.nvmrc` = `16.14.0`; `16.14.0` is installed.
    - `sites.info {site:"maker"}` gives `site.node`: `spec "16.14.0"`, `source "nvmrc"`,
      `file` ending `/maker/.nvmrc`, `version "16.14.0"`.
    - After `.nvmrc` is rewritten to `22` with no 22 installed, `sites.list` reports
      `version: null` with no reconcile wait.
  - `a_php_only_site_has_no_node`: a project with no `package.json` and no pin has no
    `node` key in `sites.list`. Add a `package.json` and `node.spec` is `"22"` with source
    `default`.
  - `a_deleted_project_does_not_break_the_list`: link two sites, delete one's directory.
    `sites.list` still succeeds and the other keeps its `node`.
  - `an_unusable_nvmrc_is_the_sites_node_error`: an `.nvmrc` holding `latest` gives
    `node.error.code == "INVALID_ARGUMENT"`.
  - `use_writes_the_managed_file_or_sites_toml_and_unuse_removes_it`:
    - On a site with a managed `.yard` (from `sites.init`), `node.use {spec:"22"}` puts
      `node = "22"` into `sites/<name>/.yard`, and its comment line survives. A `node` in
      that site's `sites.toml` entry is then ignored by `sites.info`: write one by hand
      first, and the node's source stays `managed_file`.
    - On a plain linked site, `sites.toml` gets `node = "22"`.
    - `node.unuse` removes both.
    - With a committed `.yard`, the error is `SITE_HAS_PROJECT_FILE` with a hint containing
      `node = "22"`.
  - `default_needs_an_installed_match`: `node.default {spec:"24"}` with only 16 and 22
    installed is `NODE_MISSING`. `{spec:"22"}` writes `default_node = "22"` to
    `yard.toml`. `{spec:"latest"}` is `INVALID_ARGUMENT`.
  - `list_shows_versions_default_and_sites`: with 16.14.0 and 22.18.0 installed and
    `maker` pinned to 16.14.0, the versions are `["22.18.0","16.14.0"]`, 22.18.0 is the
    default, and 16.14.0's sites are `["maker"]`.
  - In `tests/project.rs`, `init_moves_node_into_the_managed_file`: a site with
    `sites.toml` `node = "18"`, after `sites.init`, has `node = "18"` in the managed file
    and no `node` in `sites.toml`.

- [ ] **Step 2: Run** `cargo test -p yard-daemon --test node --test project`. Expected: FAIL.

- [ ] **Step 3: Implement** `node.rs`, `rpc/node.rs`, the dispatch, the fill points and
  the `init` change.

- [ ] **Step 4: Run** `cargo test -p yard-daemon`. Expected: PASS.

- [ ] **Step 5: Stage.** Commit: `show each site's Node and add node list, default, use and unuse`.

---

### Task 7: yardd: `node.install`, `node.uninstall` and `list --available`, through fnm

**Files:**
- Modify: `crates/yard-daemon/src/node.rs` (fnm helpers), `crates/yard-daemon/src/rpc/node.rs`,
  `crates/yard-daemon/src/rpc/handlers.rs`
- Modify: `crates/yard-core/src/brew.rs` (`fnm_bin`)
- Modify: `crates/yard-daemon/src/bin/yard-fake-unit.rs`:
  - a `fnm` fake;
  - the fake brew's `install fnm`;
  - the module doc.
- Modify: `crates/yard-daemon/tests/support/mod.rs` (`Fixture::add_fnm`)
- Test: `crates/yard-daemon/tests/node.rs`

**Interfaces:**
- Consumes: Task 1 (`parse_ls_remote`, `lts_from`, `newest`, `newest_per_line`,
  `LTS_HEADER`) and Task 6.
- Produces:
  - `Brew::fnm_bin(&self) -> PathBuf`: `<prefix>/bin/fnm`.
  - `node::fnm_env(paths: &Paths) -> Vec<(String, String)>`: `FNM_DIR` and
    `FNM_LOGLEVEL=info`.
  - `node::write_lts(paths: &Paths, map: &LtsMap) -> std::io::Result<()>`:
    `generated::write_atomic` of `map.render()`.
  - `node::available(brew: &Brew, paths: &Paths, major: Option<u32>) -> Result<Vec<NodeRelease>, YardError>`.
    - It runs `fnm ls-remote` outside a job, modelled on `sources::brew_stdout`, with
      `fnm_env` and `BREW_TIMEOUT`.
    - It refreshes `lts.toml` and returns `newest_per_line`.
    - With no fnm it is `FORMULA_MISSING` `fnm is not installed`, hint `brew install fnm`.
  - `rpc::node::install(ctx, NodeInstallParams) -> Result<JobStarted, YardError>`.
  - `rpc::node::uninstall(ctx, NodeVersionParams) -> Result<JobStarted, YardError>`.
  - `node::in_use(v: NodeVersion, mut sites: Vec<SiteInfo>, paths: &Paths) -> Result<(), YardError>`.
    - The caller passes the snapshot's `resolution.sites`: the handler through
      `snapshot(ctx)`, and the job through `reconciler.snapshot()`.
    - It runs `fill_node` on them itself.
    - It is `NODE_IN_USE` when `v` is the only installed match of the default spec, or of
      the spec of a linked site that uses Node.

What `node.install` does:
- **Which spec:**
  - It is `spec` when given.
  - Else `path` (an absolute directory) is resolved with `resolve_at`, and a resolve error
    is returned before any job starts.
  - Neither given is `INVALID_ARGUMENT`, hint `yard node install 22`.
  - A refused spec is `INVALID_ARGUMENT`.
- **The job (`node-install`):**
  1. When `fnm_bin` is missing, `job.run(brew_bin, ["install", "fnm"], Brew::job_env())`,
     then `fnm_bin` must exist.
  2. Create `paths.node_dir()`.
  3. `job.capture(fnm, ["ls-remote"], fnm_env)`.
     - On success: parse, write `lts.toml` from `lts_from`, and `target = newest(spec, …)`.
       `None` is `INVALID_ARGUMENT` `nodejs.org has no Node <spec>`, hint
       `yard node list --available`.
     - On failure: for an `Exact` spec, emit the line
       `could not list nodejs.org's releases; installing <v> anyway` and use it as the
       target. Otherwise fail with `JOB_FAILED` whose tail is the captured lines.
  4. When `target` is in `installed(paths)`, emit `Node <target> is already installed` and
     finish.
  5. Otherwise emit `Installing Node <target>`, then
     `job.run(fnm, ["install", "--progress", "never", "v<target>"], fnm_env)`.
- **An exact spec already installed** finishes at step 0 with the same line, without
  fnm or brew.

What `node.uninstall` does:
- `version` must parse as a `NodeVersion`, else `INVALID_ARGUMENT` hint `yard node list`.
- It must be installed, else `NODE_MISSING`, hint
  `yard node list shows the installed versions`.
- `in_use` runs before the job and again when it starts. The job is `node-uninstall` and
  runs `job.run(fnm, ["uninstall", "v<v>"], fnm_env)`.
- **The `NODE_IN_USE` messages:**
  - For the default: `Node <v> is the only Node the default (<spec>) runs`. The hint is
    `yard node default <newest other installed>` when there is one, else
    `yard node list --available`.
  - For sites: `Node <v> runs <names>`. The hint is `edit <file>` for a file source, or
    `yard node unuse --site <first>` for `sites.toml` and the managed file.

The fake fnm (argv0 `fnm`, prefix = argv0's grandparent):
- It appends `<args joined> FNM_DIR=<value>` to `<prefix>/fake-fnm.log`.
- **`ls-remote`:** it fails (exit 1, stderr `error: fake network failure`) when
  `<prefix>/fake-fnm-offline` exists. Else it prints `<prefix>/fake-fnm-remote` when that
  file exists, else this list:

```
v14.21.3 (Fermium)
v16.14.0
v16.20.2 (Gallium)
v20.19.4 (Iron)
v22.17.1
v22.18.0 (Jod)
v24.5.0 (Krypton)
v25.1.0
```

- **`install --progress never v<x>`:** when `<prefix>/fake-fnm-fail` holds `v<x>`, it
  creates `$FNM_DIR/node-versions/v<x>/` empty and exits 1. Otherwise it creates
  `installation/bin/{node,npm,npx}` (empty, 0755) and prints `Installing Node v<x>`.
- **`uninstall v<x>`:** it removes that directory.
- **The fake brew's `install fnm`** symlinks `<prefix>/bin/fnm` to itself.
- **`Fixture::add_fnm()`** places that symlink without brew.

- [ ] **Step 1: Write the failing tests** in `tests/node.rs`, reading the fake fnm log as
  `brew_log` does in `tests/php.rs`:
  - `install_a_major_installs_its_newest_and_writes_lts_toml`:
    - Setup: `add_fnm`. `node.install {spec:"22"}` passes with no error.
    - The log has `install --progress never v22.18.0` and `FNM_DIR=<root>/node`.
    - `lts.toml` starts with the header and has `jod = 22`.
    - `node.list` versions are `["22.18.0"]`.
  - `install_installs_fnm_with_brew_first`: no `add_fnm`. `fake-brew.log` has `install fnm`,
    then the install goes on.
  - `install_without_spec_uses_the_directorys_pin`: `{path: <dir with .nvmrc lts/iron>}`
    installs `v20.19.4`.
  - `an_installed_exact_version_is_not_reinstalled`: `add_node("16.14.0")`. Installing
    `16.14.0` gives the line `Node 16.14.0 is already installed`, and the fnm log is empty.
  - `an_exact_pin_installs_when_listing_fails_and_a_major_does_not`: with
    `fake-fnm-offline`, `16.14.0` installs after the `could not list` line, and `22` ends
    `JOB_FAILED`.
  - `a_failed_download_leaves_nothing_installed_and_a_retry_works`: with `fake-fnm-fail` =
    `v22.18.0`, the job is `JOB_FAILED` and `node.list` shows no 22. Remove the fail file,
    retry, and 22.18.0 is listed.
  - `uninstall_is_refused_for_the_only_match_of_a_linked_pin_or_the_default`:
    - Setup: installed 16.14.0 and 22.18.0; `maker` linked with `.nvmrc` 16.14.0 and a
      `package.json`.
    - Uninstalling 16.14.0 is `NODE_IN_USE` with hint `edit <maker>/.nvmrc`.
    - Uninstalling 22.18.0 is `NODE_IN_USE` (the default 22), with hint
      `yard node default 16.14.0`.
    - After `add_node("22.17.1")`, uninstalling 22.18.0 succeeds and the directory is gone.
    - `uninstall 22` is `INVALID_ARGUMENT`.
  - `list_available_shows_the_newest_of_each_line`: `node.list {available:true}`'s
    `available` versions are `["25.1.0","24.5.0","22.18.0","20.19.4","16.20.2"]`, with
    `lts` `krypton` on 24. With `major:16` it is one entry. With no fnm it is
    `FORMULA_MISSING`.

- [ ] **Step 2: Run** `cargo test -p yard-daemon --test node`. Expected: FAIL.

- [ ] **Step 3: Implement** the fakes, `fnm_bin`, the helpers and the two job handlers,
  and dispatch `NODE_INSTALL`/`NODE_UNINSTALL`.

- [ ] **Step 4: Run** `cargo test -p yard-daemon`. Expected: PASS.

- [ ] **Step 5: Stage.** Commit: `install and remove Node versions with fnm in Yard's own directory`.

---

### Task 8: Doctor's Node checks

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs`
- Test: `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Consumes: `fill_node`, `installed` and `lts` (Task 6).
- Produces: `fn node_checks(sites: &[SiteInfo], default: &NodeSpec, installed: &[NodeVersion], lts: &LtsMap) -> Vec<Check>`.
  Doctor calls it with a clone of `View`'s sites passed through `fill_node`, and with
  `default_node` or `DEFAULT_SPEC`.

The checks:
- **`node-spec:<site>`** (warn): the site's `node.error`, with its message and hint.
- **`node:<site>`** (warn): `version` is `None` and there is no error.
  - Message: `<site> needs Node <spec> (<file or "the default">), which is not installed`.
  - Hint: `yard node install <spec>`.
- **`node-default`** (warn), at most once: `pick(default)` is `None`, and some site's
  `node.source` is `default`.
  - Message: `the default Node (<spec>) is not installed, and <names> use it`.
  - Hint: `yard node install <spec>`.
- **No ok-checks are added for Node.** Doctor's report stays the same for machines and
  sites without Node.

- [ ] **Step 1: Write the failing tests** with the file's existing harness:
  - `a_node_site_missing_its_version_warns_with_the_install`: `.nvmrc` 16.14.0 with nothing
    installed gives `node:maker` warn, hint `yard node install 16.14.0`.
  - `a_php_only_site_gets_no_node_check`: no check id starts with `node`.
  - `an_unusable_nvmrc_is_node_spec`.
  - `a_missing_default_used_by_a_site_warns_once`: two `package.json` sites, no pins, no 22
    installed. Exactly one `node-default` check, plus `node:<each>`.

- [ ] **Step 2: Run** `cargo test -p yard-daemon --test doctor node`. Expected: FAIL.

- [ ] **Step 3: Implement** `node_checks` and call it. It runs only file and directory
  reads, never fnm.

- [ ] **Step 4: Run** `cargo test -p yard-daemon --test doctor`. Expected: PASS.

- [ ] **Step 5: Stage.** Commit: `warn in yard doctor when a site's Node is missing or its pin is unusable`.

---

### Task 9: The CLI: `yard node`, the install offer, `yard info` and `install-shims`

**Files:**
- Modify: `crates/yard-cli/src/cli.rs` (`Command::Node`, `NodeAction`),
  `crates/yard-cli/src/commands/mod.rs`
- Create: `crates/yard-cli/src/commands/node.rs`
- Modify:
  - `crates/yard-cli/src/commands/init.rs` (`settle`);
  - `crates/yard-cli/src/commands/info.rs` (the `node` line);
  - `crates/yard-cli/src/commands/shims.rs` (the `node` warning, `first_node`).
- Test: `crates/yard-cli/tests/node.rs` (new, `mod support;`), and the existing
  `settle`/`info`/`install-shims` tests in `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 5's methods and types.
- Produces the commands:

```rust
/// Node versions: list, install, remove, choose the default, or set a site's.
Node { #[command(subcommand)] action: Option<NodeAction> },

pub enum NodeAction {
    /// Installed versions, the default and the sites on each; --available asks nodejs.org.
    List { #[arg(long, value_name = "MAJOR", num_args = 0..=1)] available: Option<Option<u32>> },
    /// Install a Node version (22, 22.17, 16.14.0, lts/iron, lts/*) with fnm; with none,
    /// the version this directory asks for. Ctrl-C cancels.
    Install { spec: Option<String> },
    /// Remove an installed version, e.g. 16.14.0; refused while the default or a linked
    /// site has no other version to run.
    Uninstall { version: String },
    /// Run sites and directories with no pin on this version; it must be installed.
    Default { spec: String },
    /// Pin a site to a version, in its managed .yard or sites.toml.
    Use { spec: String, #[arg(long)] site: Option<String> },
    /// Remove a site's pin set with yard node use.
    Unuse { #[arg(long)] site: Option<String> },
}
```

What the commands do (`commands/node.rs`):
- **`list`:**
  - A table `VERSION DEFAULT SITES` followed by `for_agents("yard node list")`, as
    `php::list` does.
  - With `--available`, a second table `VERSION LTS`.
  - With nothing installed, the line is
    `No Node installed. yard node install <default> installs the default.`
- **`install`:**
  - `run_job(NODE_INSTALL, …)` with `{spec}`, or `{path: cwd}` when none is given.
  - It ends with `Installed Node <spec or "for this directory">.`
- **`uninstall`** and **`default`** mirror `php.rs`.
- **`use`/`unuse`:**
  - The site comes from `--site`, else `sites::containing(&sites, &cwd, hint)`, as
    `sites::isolate` does.
  - Success prints `<site> now runs Node <spec><set_in>`, where `set_in` is the existing
    helper's wording.
  - When the returned `node.version` is `None` it adds the line
    `Node <spec> is not installed: yard node install <spec>`.
  - `unuse` prints `<site> runs Node <spec> again (<source label>)`.

`settle`:
- **Which Node is missing:** `site.node` with a `spec`, no `error` and `version: None`.
- **Listing it:** the line `Node <spec>  (yard node install <spec>)` is added to the
  `needs:` list.
- **When it is the only thing missing:**
  - `yes`, or an interactive yes, installs it.
  - Otherwise it prints `<site> runs Node <spec>, which is not installed: yard node install <spec>`
    to stderr, prints `Serving <url>`, and returns `Ok`.
- **With other needs:**
  - When the user agrees, Node installs last, after PHP, extensions and services, with
    `run_job(NODE_INSTALL, {spec})`.
  - When they refuse, it is `NEEDS_INSTALL` as today; `data.needs` is unchanged.

`yard info` adds a line after `php`:
- installed, spec equal to the version: `node 16.14.0 (.nvmrc)`;
- installed, spec different: `node 22.18.0 (22, .nvmrc)`;
- missing: `node 16.14.0 — not installed (yard node install 16.14.0)`;
- error: `node <error message>`, plus its hint in the problems block.

The label is `source_name`'s, applied to `node.source`. No `node` line appears when
`site.node` is `None`.

`install-shims`:
- `ShimsInfo` gains `first_node: Option<String>`.
- If the first `node` on PATH is not `<bin>/node`, it warns
  `warning: node on your PATH is <path><whose> — Yard's bin must come before it`.
- `whose` comes from `fn whose(path: &str) -> &'static str`:
  - ` (Herd's)` when the path contains `/Herd/`;
  - ` (nvm's)` for `/.nvm/`;
  - ` (mise's)` for `/mise/`;
  - ` (fnm's)` for `fnm_multishells`;
  - ` (brew's)` when it starts with `/opt/homebrew/`;
  - else empty.

- [ ] **Step 1: Write the failing tests** with `MockDaemon`:
  - `node_list_prints_versions_default_and_sites`, and the JSON `--json` output is the
    `NodeList`.
  - `node_install_without_spec_sends_the_cwd`: `params("node.install")[0]["path"]` is the
    canonical cwd. With `22`, `spec` is `"22"`.
  - `node_use_picks_the_site_containing_the_cwd_and_says_when_not_installed`.
  - `link_with_only_node_missing_and_no_tty_serves_and_hints`: the exit code is 0, stderr
    has `yard node install 16.14.0`, and no `node.install` call was made.
  - `link_yes_installs_node_after_php`: `params` order is `php.install` then `node.install`.
  - `info_shows_the_node_line` for the three states, and none for `node: None`.
  - `whose_names_the_managers`: a unit test of `whose` over the five path shapes.

- [ ] **Step 2: Run** `cargo test -p yard-cli`. Expected: FAIL.

- [ ] **Step 3: Implement**, keeping the cli-output rule: human text on stdout, warnings on
  stderr, and `--json` giving the proto type unchanged.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS. Then check
  `yard node --help` and `yard node install --help` read as above.

- [ ] **Step 5: Stage.** Commit: `add yard node and offer a site's Node when it is linked`.

---

### Task 10: Docs and the real-machine check

**Files:**
- Modify:
  - `docs/superpowers/specs/2026-10-19-yard-design.md`: §3.2's `node/` folder, §6.2 if
    Task 5 left anything, and §8.1, which now points to the M4c spec; §5.4's
    `NODE_EXTRA_CA_CERTS` advice is now for Node outside the shims.
  - `docs/superpowers/specs/2026-10-22-yard-project-file-design.md`: the `node` row is a
    spec rather than a major, and §5's Node line.
  - `docs/superpowers/specs/2026-10-24-yard-managed-project-file-design.md`: the
    `yard node use --site` row.
  - `AGENTS.md`:
    - the Status entry;
    - a "Never nvm's or Herd's Node" key decision next to "Never Herd's PHP";
    - the `e2e-m4c.sh` entry under Build & test;
    - the fnm facts under Environment findings, once the e2e has run.
  - `README.md`: Node is no longer "later".
- Modify `.hydra/rules`:
  - `config-ownership.md`: `node/` is fnm's; `lts.toml` is generated; `default_node`,
    `sites.toml` `node` and `.yard` `node` are user-owned; `.nvmrc`/`.node-version` are
    read only.
  - `homebrew.md`: fnm by absolute path; brew installs fnm only in a `node.install` job.
  - `rpc-protocol.md` and `cli-output.md`, where they list methods or commands.
  - Then run `hydra sync` and `hydra doctor`.
- Create: `scripts/e2e-m4c.sh`

- [ ] **Step 1: Update the docs.** Grep them for `node`, `Node`, `nvm`, `fnm`,
  `NODE_EXTRA_CA_CERTS`, `2.6` and `SHIMS`, and fix each stale line.

- [ ] **Step 2: Write `scripts/e2e-m4c.sh`**, modelled on `scripts/e2e-m4a1.sh`, with
  `YARD="$HOME/.yard"` and the release `yard`.
  - **Preconditions:**
    - fnm is installed (`$(brew --prefix)/bin/fnm`), else exit with `brew install fnm`.
    - Record `ls -la ~/.nvm/versions/node` and
      `ls -la "$HOME/Library/Application Support/Herd/config/nvm"` into the scratchpad.
      Either may be missing; record that too.
  - **Backups:** back up `yard.toml` and `sites.toml`, and restore them with a trap,
    checked with `cmp`.
  - **Record installed versions:** note which Node versions are installed before the run,
    so teardown removes only the ones the script added.
  - **Pinned 16:** in a scratch project under `$TMPDIR` (never `~/Herd`) with `.nvmrc`
    `16.14.0` and a `package.json` whose `scripts.v` is `node -v`:
    - `yard link --yes` installs 16.14.0;
    - `"$YARD/bin/node" -v` is `v16.14.0`;
    - `"$YARD/bin/npm" run -s v` prints `v16.14.0`.
  - **A native build:** `"$YARD/bin/npm" install --build-from-source bufferutil` succeeds.
  - **`use` wins over `.nvmrc`:** after `yard init` (managed file), `yard node install 22`
    and `yard node use 22`, `node -v` starts with `v22.`.
  - **A missing pin:** `.nvmrc` `16.20.2` with that version not installed makes
    `node -v` exit 1 with `yard node install 16.20.2` in stderr. Restore `.nvmrc`
    afterwards.
  - **`NODE_IN_USE`:** `yard node uninstall 16.14.0`, while the site still pins it after
    `yard node unuse`, fails with `NODE_IN_USE`.
  - **Teardown:**
    - `yard unlink`, and remove the scratch override and managed directory;
    - uninstall only the versions this run added;
    - restore the configs;
    - fail if either nvm listing changed.
  - `shellcheck scripts/e2e-m4c.sh` stays clean.
  - Do not run it: the controller asks the maintainer, since it needs `brew install fnm` and
    downloads Node.

- [ ] **Step 3: Run** `cargo test --workspace`, clippy and `cargo fmt --check`. Expected:
  all clean.

- [ ] **Step 4: Stage** the docs, rules and script. Commit:
  `document Yard's Node and add its real-machine check`.
