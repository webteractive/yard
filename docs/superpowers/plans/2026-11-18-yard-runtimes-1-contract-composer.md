# Runtimes plan 1: the contract and Composer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every runtime (PHP, Composer, Node) is defined by a carried recipe behind one `Runtime` contract, and `composer` runs the Composer each project's PHP can run, downloaded by Yard from getcomposer.org.

**Architecture:** A new `yard_core::runtimes` module holds the contract, the runtime recipe format (TOML, compiled-in carried recipes `php.toml`, `node.toml`, `composer.toml`), version specs, the generic pin walk (M4c's Node walk, generalized; Node keeps its behaviour on it) and Composer's selection rules. `yard-shim` runs `composer` from the recipe. yardd installs Composer versions as jobs (the engine recipes' download code, generalized), answers `runtime.*`/`runtimes.list`, and fills each site's runtimes; the CLI gets `yard runtime …`/`yard runtimes` and offers Composer in `link`/`init`; Yard.app shows a site's Composer. PHP and Node keep every code path they have; the contract describes them and `runtime.*` delegates to `php.*`/`node.*` for them.

**Tech Stack:** Rust 1.92 workspace (`yard-proto`, `yard-core`, `yard-daemon`, `yard-cli`, `yard-shim`), serde + toml (`Spanned` for lines), toml_edit for user files, `/usr/bin/curl` for downloads; Yard.app in `app/` (Rust 1.98.1, GPUI).

**Spec:** `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md` (approved 2026-11-18). This plan is its §17 row 1. Read §2, §4–§9, §11–§16 before starting.

## Global Constraints

- Carried recipes only this plan: `~/.yard/runtimes/*.toml` is not loaded (plan 3). `runtimes_dir()` exists but nothing reads it.
- Drivers are a closed set: `driver = "php"` and `source.kind = "fnm"`/`"brew"` load only with `Origin::Carried` (spec §4).
- An index (`[source.index]`) loads only in a carried recipe (spec §6.2, maintainer 2026-11-18).
- The shims never download, install or run fnm/brew (spec §2). A missing version is `RUNTIME_MISSING`, `<Name> <spec> is not installed (<where the spec came from>)`, hint `yard runtime install <key> <spec>`.
- A runtime never puts a site on its setup page and is never a `Need` (spec §7.1).
- No behaviour change for PHP or Node: every existing test in `yard-core/src/shim.rs`, `yard-daemon/tests/*node*`, `yard-cli/tests` passes unchanged (spec §11, §16). Renaming a test is a change.
- Composer: no pin → `2.2` when the project's PHP is below `7.2.5`, else `2` excluding `2.2.x`; `2.3.0` and later require PHP `7.2.5` (spec §9).
- Installs live in `<root>/installs/<key>/<version>/` with a generated `.yard-install.toml`; built in `installs/<key>/.stage-<pid>-<n>` and renamed into place (spec §6, §15).
- Downloads: `https://` only (`file://` only with `DaemonOptions.allow_file_urls`), SHA-256 checked; a mismatch is `DOWNLOAD_HASH_MISMATCH` (spec §6, recipes spec §5).
- `.yard` `[runtimes]`: keys are recipe runtime keys, values specs; `php`/`node` under `[runtimes]` are `PROJECT_FILE_INVALID` ("write php at the top level"); an unknown key is `RUNTIME_UNKNOWN` at use, never an invalid file (spec §7.1).
- Specs: major, major.minor or exact, leading `v` accepted; ranges, `latest`, `stable` refused naming file and line (spec §6.1).
- Protocol `2.14`. New error codes: `RUNTIME_MISSING`, `RUNTIME_IN_USE`, `RUNTIME_RECIPE_INVALID`, `RUNTIME_UNKNOWN`. New `SettingSource::PinFile`.
- `composer self-update`/`selfupdate` refused, hint `yard runtime install composer <version>` (spec §9.1).
- Brew Composer fallback for one release: no Yard Composer installed at all, brew's `bin/composer` exists, project PHP ≥ 7.2.5 → run it with one stderr line `yard: using brew's composer; yard runtime install composer for per-project versions` (spec §9.3).
- There is no `yard composer …` command (spec §12).
- Commit messages: lowercase prose subject, prose body, no type prefix, no trailers (no `Co-Authored-By`, no `Claude-Session`). Commit only with the maintainer's go-ahead; work on `main`.
- After builds: `cargo clean --profile dev`; never touch `target/release`.

## Review Focus

1. `composer -n self-update`, `composer --no-ansi selfupdate --2.2`: global options before the command would bypass a "starts with" check and let Composer overwrite Yard's phar. Refusal matches the first argument that does not start with `-` (Task 7 test `refuse_matches_the_first_non_option_argument`).
2. PHP 7.2.0–7.2.4, and a PHP whose full version cannot be read (keg link unreadable): `php_below = "7.2.5"` must give 2.2 for the first, and the second compares as `x.y.0` (so 7.2 unknown → 2.2) (Task 6 tests `php_below_compares_the_full_version`, `an_unreadable_full_version_counts_as_patch_zero`).
3. getcomposer.org unreachable, a `.sha256sum` that is not 64 hex, a hash mismatch, a cancel mid-download: the job fails naming the URL, `installs/composer/` holds no stage and no new version (Task 8 tests `a_bad_hash_leaves_nothing`, `a_garbled_sha256sum_is_refused`).
4. A version directory without `.yard-install.toml` (a hand-made or half-deleted one) is not installed, so the shim never execs a phar Yard did not check (Task 6 test `a_version_without_its_record_is_not_installed`).
5. `composer` in a subdirectory of a linked site whose managed `.yard` sets `[runtimes] composer` while a nearer `.yard` in the subdirectory does not: the managed file still wins at the site root, and a managed file without the key hides `sites.toml` (Task 5 tests `a_managed_file_without_the_key_hides_sites_toml`, `a_nearer_yard_without_the_key_passes_over`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/yard-proto/src/{error,lib,methods,types}.rs` | codes, `2.14`, `runtime.*` methods/params, `RuntimeSummary`, `RuntimesList`, `RuntimeList`, `SiteRuntime`, `SettingSource::PinFile` |
| `crates/yard-core/src/runtimes/mod.rs` | `Runtime` trait, `RuntimeKind`, `PinKey`, `Origin`, `Runtimes` registry |
| `crates/yard-core/src/runtimes/version.rs` | `RuntimeVersion`, `VersionSpec`, matching and picking |
| `crates/yard-core/src/runtimes/recipe.rs` | runtime recipe TOML format, placeholders, load checks, `RecipeError` |
| `crates/yard-core/src/runtimes/carried.rs` + `crates/yard-core/runtimes/{php,node,composer}.toml` | carried recipes, compiled in |
| `crates/yard-core/src/runtimes/resolve.rs` | generic pin walk, `PinSource`, `uses_runtime` |
| `crates/yard-core/src/runtimes/select.rs` | default rules, `requires`, installed listing, `.yard-install.toml`, composer index parsing |
| `crates/yard-core/src/runtimes/shim.rs` | the command a recipe shim execs, `refuse`, brew fallback decision |
| `crates/yard-core/src/{project,config/sites,config/yard,paths,shim}.rs` | `[runtimes]` keys; `Paths` for installs; `resolve_node`/`uses_node` delegate; composer leaves `PHP_SHIMS`/`command` |
| `crates/yard-shim/src/main.rs` | dispatch `composer` to the recipe path |
| `crates/yard-daemon/src/programs.rs` | download generalized: target root and expected hash given by caller |
| `crates/yard-daemon/src/runtimes.rs` | registry, install/uninstall jobs, index fetch, `fill_runtimes`, in-use |
| `crates/yard-daemon/src/rpc/runtimes.rs` (+ `rpc/mod.rs` dispatch) | `runtime.*`, `runtimes.list` |
| `crates/yard-daemon/src/doctor.rs` | runtime checks |
| `crates/yard-cli/src/{cli.rs,commands/runtime.rs,commands/info.rs,commands/init.rs}` | commands, info lines, settle offer |
| `app/yard-app-model/src/settle.rs`, `app/yard-app/src/sites/detail.rs` | Composer row and offer |
| `scripts/e2e-composer.sh` | real-machine check |
| docs | main spec §3.3/§4.5, AGENTS.md, README, DEVELOPMENT.md, `.hydra/rules/{cli-output,config-ownership,rpc-protocol,homebrew,rust-workspace}.md` |

---

### Task 1: Protocol additions

**Files:**
- Modify: `crates/yard-proto/src/error.rs`, `crates/yard-proto/src/lib.rs:17`, `crates/yard-proto/src/methods.rs`, `crates/yard-proto/src/types.rs`

**Interfaces:**
- Produces:
  - `ErrorCode::{RuntimeMissing, RuntimeInUse, RuntimeRecipeInvalid, RuntimeUnknown}` ↔ `"RUNTIME_MISSING"`, `"RUNTIME_IN_USE"`, `"RUNTIME_RECIPE_INVALID"`, `"RUNTIME_UNKNOWN"`.
  - `PROTOCOL_VERSION = "2.14"`.
  - `methods::{RUNTIMES_LIST = "runtimes.list", RUNTIME_LIST = "runtime.list", RUNTIME_INSTALL = "runtime.install", RUNTIME_UNINSTALL = "runtime.uninstall", RUNTIME_DEFAULT = "runtime.default", RUNTIME_USE = "runtime.use", RUNTIME_UNUSE = "runtime.unuse"}`.
  - Params: `RuntimeKeyParams { key: String }` (`runtime.list`), `RuntimeInstallParams { key: String, spec: Option<String>, path: Option<String> }`, `RuntimeVersionParams { key: String, version: String }`, `RuntimeSpecParams { key: String, spec: String }`, `RuntimeUseParams { key: String, site: String, spec: String }`, `RuntimeSiteParams { key: String, site: String }` (`runtime.unuse`).
  - Types: `RuntimeSummary { key: String, name: String, origin: String /* "carried" | "user" */, file: Option<String>, shims: Vec<String>, installed: Vec<String>, default: Option<String> }`; `RuntimesList { runtimes: Vec<RuntimeSummary>, errors: Vec<YardError> }`; `RuntimeVersionInfo { version: String, default: bool, sites: Vec<String> }`; `RuntimeList { key: String, default: String, versions: Vec<RuntimeVersionInfo> }`; `SiteRuntime { key: String, name: String, spec: Option<String>, source: SettingSource, file: Option<String>, version: Option<String>, error: Option<YardError> }`.
  - `SiteInfo.runtimes: Vec<SiteRuntime>` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`; `SiteDetail` unchanged (it carries `site`).
  - `SettingSource::PinFile` (before `Other`).

- [ ] **Step 1: Write the failing tests** in the existing test modules:
  - `error.rs`: extend the code table test with the four pairs above.
  - `methods.rs`: `runtime_method_names()` asserts the seven names; `runtime_install_params_accept_a_path()` deserializes `{"key":"composer","path":"/code/shop"}` to `spec: None, path: Some(..)`.
  - `types.rs`: `site_info_without_runtimes_omits_them()` (serialized `SiteInfo` with empty `runtimes` has no `runtimes` key, and JSON without it deserializes); `setting_source_pin_file_round_trips()` (`"pin_file"`).
- [ ] **Step 2: Run** `cargo test -p yard-proto` — Expected: FAIL (unknown variants/consts).
- [ ] **Step 3: Implement** the items above; doc comments as the neighbouring Node items have them. Fix every `SiteInfo { .. }` literal in the workspace (`cargo build --workspace --all-targets` lists them) with `runtimes: vec![]`; same in `app/` (`cargo build` there).
- [ ] **Step 4: Run** `cargo test -p yard-proto && cargo build --workspace --all-targets` and in `app/` `cargo build --workspace --all-targets` — Expected: PASS, builds.
- [ ] **Step 5: Commit** `git add crates/yard-proto app crates && git commit` — subject `protocol 2.14 names runtimes`.

---

### Task 2: Versions and specs

**Files:**
- Create: `crates/yard-core/src/runtimes/mod.rs` (module declarations only for now), `crates/yard-core/src/runtimes/version.rs`
- Modify: `crates/yard-core/src/lib.rs` (`pub mod runtimes;`)

**Interfaces:**
- Produces:
  - `RuntimeVersion(Vec<u32>)`: `FromStr` (dotted numbers, 1–4 parts, leading `v` stripped), `Display` (dotted), `Ord` numeric with missing parts as 0 (`2.2` < `2.2.1`), `prefix_matches(&self, spec: &VersionSpec) -> bool`.
  - `VersionSpec { parts: Vec<u32> }` (1–3 parts): `pub fn parse(s: &str) -> Result<VersionSpec, String>`; `Display`.
  - `pub fn pick(spec: &VersionSpec, exclude: &[VersionSpec], installed: &[RuntimeVersion]) -> Option<RuntimeVersion>` — newest installed matching `spec` and matching none of `exclude`.
  - `pub fn spec_line(text: &str) -> Option<(usize, &str)>` reused from `crate::node::spec_line` (re-export, do not copy).

- [ ] **Step 1: Write the failing tests** in `version.rs`:
  - `parses_majors_minors_and_exact_versions`: `"2"`, `"2.2"`, `"2.2.32"`, `"v2.11.2"`, `" 2.2 "` ok; `"^2"`, `">=1"`, `"latest"`, `"stable"`, `"2.x"`, `""`, `"2.2.3.4"` err, and the error for `"^2"` contains `"a version such as 2.2"`.
  - `orders_numerically`: `2.11.2 > 2.2.32`, `2.2 < 2.2.1`.
  - `pick_takes_the_newest_match_and_honours_exclude`: installed `[2.2.32, 2.11.2, 2.10.0]`; `pick(2, [], ..) = 2.11.2`; `pick(2, [2.2], ..) = 2.11.2`; with installed `[2.2.32]`, `pick(2, [2.2], ..) = None` and `pick(2, [], ..) = 2.2.32`; `pick(2.2.32, [], ..) = 2.2.32`.
- [ ] **Step 2: Run** `cargo test -p yard-core runtimes::version` — Expected: FAIL.
- [ ] **Step 3: Implement** in `version.rs`.
- [ ] **Step 4: Run** the same — Expected: PASS.
- [ ] **Step 5: Commit** — `runtime versions and specs`.

---

### Task 3: Runtime recipes, the carried three, the registry

**Files:**
- Create: `crates/yard-core/src/runtimes/recipe.rs`, `crates/yard-core/src/runtimes/carried.rs`, `crates/yard-core/runtimes/php.toml`, `crates/yard-core/runtimes/node.toml`, `crates/yard-core/runtimes/composer.toml`
- Modify: `crates/yard-core/src/runtimes/mod.rs`, `crates/yard-core/src/paths.rs`
- Test: `crates/yard-core/src/runtimes/recipe_tests.rs` (`#[cfg(test)] mod recipe_tests;` in `mod.rs`), testdata in `crates/yard-core/src/runtimes/testdata/`

**Interfaces:**
- Consumes: Task 2's `RuntimeVersion`, `VersionSpec`.
- Produces (in `runtimes/mod.rs`):
  - `pub enum Origin { Carried, User(PathBuf) }`.
  - `pub enum RuntimeKind { Php, Node, Recipe }`.
  - `pub enum PinKey { TopLevel(&'static str) /* "php" | "node" */, Runtimes(String) }`.
  - `pub trait Runtime: Send + Sync + Debug { fn key(&self) -> &str; fn display(&self) -> &str; fn origin(&self) -> &Origin; fn kind(&self) -> RuntimeKind; fn shims(&self) -> Vec<String>; fn pin_key(&self) -> PinKey; fn pin_files(&self) -> &[String]; fn uses_when(&self) -> &[String]; fn wakes(&self, shim: &str) -> bool; fn recipe(&self) -> &RuntimeRecipe; }` — one struct `RecipeRuntime { recipe: RuntimeRecipe }` implements it; `kind()` comes from the recipe's driver/source (`driver = "php"` → `Php`, `kind = "fnm"` → `Node`, else `Recipe`). (Spec §19 leaves the trait's shape to the builder; one struct over a closed `kind` is the shape this plan takes.)
  - `pub struct Runtimes { list: Vec<Arc<dyn Runtime>> }` with `pub fn carried() -> Runtimes`, `pub fn get(&self, key: &str) -> Option<&Arc<dyn Runtime>>`, `pub fn by_shim(&self, name: &str) -> Option<&Arc<dyn Runtime>>`, `pub fn iter(&self)`.
- Produces (in `recipe.rs`):
  - `pub const SCHEMA: i64 = 1;`
  - `pub struct RuntimeRecipe { pub key, pub name, pub description: String, pub origin: Origin, pub driver: Option<Driver>, pub pin_files: Vec<String>, pub uses_when: Vec<String>, pub runs_on: Option<RunsOn>, pub wake: bool, pub defaults: Vec<DefaultRule>, pub requires: Vec<Requires>, pub shims: BTreeMap<String, ShimDef>, pub path: Vec<String>, pub env: BTreeMap<String, String>, pub source: SourceDef, pub versions: BTreeMap<RuntimeVersion, VersionEntry> }`.
  - `Driver::Php`; `RunsOn::{Php, Node}`; `DefaultRule { spec: VersionSpec, php_below: Option<RuntimeVersion>, exclude: Vec<VersionSpec> }`; `Requires { from: RuntimeVersion, php: RuntimeVersion }`; `ShimDef { exec: Vec<String>, refuse: Vec<Refuse>, wake: Option<bool> }`; `Refuse { args: Vec<String>, hint: String }`; `SourceDef::{Download { url: String, strip: u32, file: Option<String>, index: Option<IndexDef> }, Fnm, Brew}`; `IndexDef { kind: IndexKind /* Composer */, url: String, sha256_url: String }`; `VersionEntry::{Sha256(String), Path(PathBuf)}`.
  - `pub fn parse_runtime_recipe(text: &str, file: &Path, origin: Origin, opts: &LoadOptions) -> Result<RuntimeRecipe, RecipeError>` (reuse `services::engines::LoadOptions` for `allow_file_urls`).
  - `pub struct RecipeError { file, line: Option<usize>, message }` with `to_yard_error()` → `RUNTIME_RECIPE_INVALID`, message `<file>:<line>: <why>`, hint `fix <file>`.
- Produces (in `paths.rs`): `installs_dir()` = `<root>/installs`, `install_root(key, version: &str)` = `installs/<key>/<version>`, `install_record(key, version)` = `…/.yard-install.toml`, `runtimes_dir()` = `<root>/runtimes`, `runtime_cache(key)` = `<root>/cache/<key>`.

Recipe format is spec §5 exactly, with one addition this plan rules on: `[shims.<name>] wake = false` overrides the recipe's `wake` for that shim (PHP's `pecl`, `phpize`, `php-config` never wake, as `WAKE_SHIMS` says today). Placeholders: `{home}`, `{version}`, `{cache}`, `{php}`, `{node}`, `{root}` (spec §5.1).

The carried recipes:
- `php.toml`: `driver = "php"`, shims `php` (wake), `pecl`, `phpize`, `php-config` (each `wake = false`), `pin_files = [".yard-php"]`, `uses_when = []`, `[source] kind = "brew"`. Its shims' `exec` are unused (the driver runs them); give each `exec = ["{home}/bin/<name>"]` for documentation.
- `node.toml`: `[source] kind = "fnm"`, shims `node`, `npm`, `npx` (no wake), `pin_files = [".nvmrc", ".node-version"]`, `uses_when = ["package.json"]`.
- `composer.toml`: spec §9.1 verbatim (including `exclude`, `[[requires]]`, the `2.2.32` pin with hash `241a3d13b7c6d35d970b1f69a000298b66943541302148a438a932f5dd44a9fc`).

- [ ] **Step 1: Write the failing tests** in `recipe_tests.rs`:
  - `every_carried_recipe_loads` — `Runtimes::carried()` has keys `composer`, `node`, `php`, and each `texts()` entry parses.
  - `carried_shims_match_the_shim_constants` — the union of carried shims equals `shim::SHIMS` as a set, `php.toml`'s are `PHP_SHIMS` minus `composer` (Task 7 removes `composer` from `PHP_SHIMS`; until then compare with `composer` filtered out), `node.toml`'s are `NODE_SHIMS`, and the shims whose `wakes()` is true are exactly `WAKE_SHIMS`.
  - `a_user_recipe_cannot_name_a_driver` — `driver = "php"` with `Origin::User(..)` errors, message contains `driver`.
  - `a_user_recipe_cannot_have_an_index` — composer.toml text loaded as `Origin::User` errors naming `index`.
  - `fnm_and_brew_sources_are_carried_only`.
  - `unknown_fields_fail_with_their_line` — an extra `colour = "red"` on line 4 gives `line == Some(4)`.
  - `a_bad_placeholder_fails_at_load` — `exec = ["{hoem}/bin/x"]` errors naming `{hoem}`.
  - `exec_must_start_under_home_or_at_php` — `exec = ["/bin/sh", "-c", "x"]` errors.
  - `php_below_and_requires_need_runs_on`.
  - `a_versions_entry_needs_a_hash_or_a_path` — `sha256 = "abc"` errors; 64 hex ok; `path = "relative"` errors.
  - `http_urls_are_refused_and_file_urls_only_when_allowed`.
  - `the_stem_must_be_the_key` and `php_and_node_keys_are_reserved_for_user_recipes`.
  - `shim_names_are_checked` — `"Bad Name"` errors; `yard`, `yardd`, `yard-shim` error.
- [ ] **Step 2: Run** `cargo test -p yard-core runtimes::` — Expected: FAIL.
- [ ] **Step 3: Implement** `recipe.rs` (Raw structs with `deny_unknown_fields` and `toml::Spanned` for line numbers, as `services/engines/recipe.rs` does), `carried.rs` (`texts()` with `include_str!`, `all()`), the three TOMLs, the registry and `Paths` methods.
- [ ] **Step 4: Run** the same — Expected: PASS.
- [ ] **Step 5: Commit** — `runtime recipes and the carried php, node and composer`.

---

### Task 4: `[runtimes]` in `.yard`, `sites.toml` and `yard.toml`

**Files:**
- Modify: `crates/yard-core/src/project.rs`, `crates/yard-core/src/config/sites.rs`, `crates/yard-core/src/config/yard.rs`

**Interfaces:**
- Consumes: Task 2's `VersionSpec::parse`.
- Produces:
  - `ProjectFile.runtimes: BTreeMap<String, VersionSpec>` (committed and managed `.yard`).
  - `SiteEntry.runtimes: BTreeMap<String, VersionSpec>` (`[sites.<s>.runtimes]`).
  - `YardConfig.runtime_defaults: BTreeMap<String, VersionSpec>` read from `[runtimes.<key>] default = "<spec>"`.
  - `Detected.runtimes: BTreeMap<String, VersionSpec>` (what `yard init` moves from `sites.toml` into the managed file, as it moves `node`) and `init`'s render writes them under `[runtimes]`.

- [ ] **Step 1: Write the failing tests**:
  - `project.rs`: `runtimes_table_parses` (`[runtimes]\ncomposer = "2.2"\n` → `{"composer": 2.2}`); `php_or_node_under_runtimes_is_invalid` (error `PROJECT_FILE_INVALID`, message contains `write php at the top level`); `a_bad_runtime_spec_names_its_line` (`composer = "^2"` → line 2); `an_unknown_runtime_key_is_kept` (`bun = "1"` parses: unknown-ness is decided at use); `a_runtime_value_must_be_a_string` (`composer = 2`).
  - `config/sites.rs`: `site_runtimes_are_parsed`.
  - `config/yard.rs`: `runtime_defaults_parse` (`[runtimes.composer]\ndefault = "2"\n`) and `a_bad_runtime_default_is_config_invalid`.
  - `project.rs`: `init_moves_site_runtimes_into_the_file` — a site with `runtimes = { composer = "2.2" }` renders `[runtimes]\ncomposer = "2.2"`.
- [ ] **Step 2: Run** `cargo test -p yard-core project:: config::` — Expected: FAIL.
- [ ] **Step 3: Implement**; keep `deny_unknown_fields`; the managed-file parser accepts `[runtimes]` too.
- [ ] **Step 4: Run** the same, then `cargo test -p yard-core` — Expected: PASS.
- [ ] **Step 5: Commit** — `.yard, sites.toml and yard.toml carry runtime pins`.

---

### Task 5: The generic pin walk, with Node on it

**Files:**
- Create: `crates/yard-core/src/runtimes/resolve.rs`
- Modify: `crates/yard-core/src/shim.rs` (`resolve_node`, `site_node`, `uses_node`, `failed_pin` delegate)

**Interfaces:**
- Consumes: Task 3's `PinKey`, Task 4's `runtimes` fields.
- Produces:
  - `pub enum PinSource { Project(PathBuf), Managed(PathBuf), Site(String), PinFile(PathBuf), Default }` with `Display` as `NodeSource`'s.
  - `pub fn resolve_pin<S: Clone>(cwd: &Path, key: &PinKey, pin_files: &[String], sites: &SitesConfig, paths: &Paths, read: &dyn Fn(&Path) -> Option<Result<String, YardError>>, canonical: &dyn Fn(&Path) -> PathBuf, from_file: &dyn Fn(&ProjectFile) -> Option<S>, from_site: &dyn Fn(&SiteEntry) -> Option<S>, from_pin_file: &dyn Fn(&str, &Path) -> Result<S, YardError>) -> Result<Option<(S, PinSource)>, YardError>` — M4c §5's walk exactly as `resolve_node` does it today (nearest directory wins; in one directory `.yard` with the key, then the site's setting at the innermost site root — the managed file for the site's path alone when it exists, else `sites.toml` — then `pin_files` in order; a committed `.yard` at the site root skips the site's setting; the canonical-only root fallback after the loop). `None` means nothing pinned; the caller applies the default.
  - `pub fn uses_runtime(source: &PinSource, root: &Path, has_uses_file: bool) -> bool` — `uses_node`'s rule.
  - `resolve_node` keeps its signature and `NodeSource` (mapping `PinFile(f)` to `Nvmrc`/`NodeVersionFile` by file name, `None` to `(default_node or DEFAULT_SPEC, Default)`); `uses_node` and `failed_pin` delegate.

- [ ] **Step 1: Write the failing tests** in `resolve.rs` for a recipe key `composer` with `pin_files = []` (fixtures built like `shim.rs`'s Node tests):
  - `the_nearest_yard_with_the_key_wins`, `a_nearer_yard_without_the_key_passes_over`, `the_site_setting_beats_a_yard_above_the_site`, `a_managed_file_without_the_key_hides_sites_toml`, `a_committed_yard_skips_the_site_setting`, `an_unusable_file_stops_the_walk` (a FIFO-like read error names the file), `nothing_pinned_is_none`, `a_pin_file_is_read_with_its_parser` (a recipe with `pin_files = [".bun-version"]`).
- [ ] **Step 2: Run** `cargo test -p yard-core runtimes::resolve` — Expected: FAIL.
- [ ] **Step 3: Implement** `resolve_pin` by moving `resolve_node`'s body into it; rewrite `resolve_node`, `site_node`, `uses_node` and `failed_pin` over it.
- [ ] **Step 4: Run** `cargo test -p yard-core` — Expected: PASS, every existing `shim::tests` Node test unchanged.
- [ ] **Step 5: Commit** — `one pin walk for every runtime, node's`.

---

### Task 6: Composer's selection, install records and the index

**Files:**
- Create: `crates/yard-core/src/runtimes/select.rs`
- Test: in-file tests; `crates/yard-core/src/runtimes/testdata/composer-versions.json` (a copy of `https://getcomposer.org/versions` as of 2026-11-18)

**Interfaces:**
- Consumes: Tasks 2, 3, 5.
- Produces:
  - `pub struct InstallRecord { pub version: RuntimeVersion, pub url: String, pub sha256: String, pub requires_php: Option<RuntimeVersion>, pub installed_at: String }` with `pub fn render(&self) -> String` (generated header `# Generated by Yard — do not edit`) and `pub fn parse(text: &str) -> Option<InstallRecord>`.
  - `pub fn installed(paths: &Paths, key: &str, list: &dyn Fn(&Path) -> Vec<String>, read: &dyn Fn(&Path) -> Option<String>) -> Vec<(RuntimeVersion, InstallRecord)>` — directories of `installs/<key>/` whose name parses as a version and whose `.yard-install.toml` parses and names that version; `.stage-*` and the rest ignored.
  - `pub fn default_spec(recipe: &RuntimeRecipe, php_full: Option<&RuntimeVersion>) -> Option<(VersionSpec, Vec<VersionSpec>)>` — the first `[[default]]` rule whose `php_below` is absent or above `php_full`; the spec and its `exclude`. `php_full` `None` (no `runs_on`) skips rules with `php_below`.
  - `pub fn requires_php(recipe: &RuntimeRecipe, v: &RuntimeVersion) -> Option<RuntimeVersion>` — the `php` of the last `[[requires]]` whose `from <= v`.
  - `pub fn php_full_or_zero(full: Option<&str>, v: PhpVersion) -> RuntimeVersion` — the keg's full version, else `<major>.<minor>.0`.
  - `pub fn composer_channels(json: &str) -> Result<BTreeMap<String, RuntimeVersion>, String>` — for each of the keys `"1"`, `"2"`, `"2.2"`, the highest `version` in its array.
  - `pub fn index_version(spec: &VersionSpec, channels: &BTreeMap<String, RuntimeVersion>) -> Result<Option<RuntimeVersion>, String>` — exact spec (3 parts) → itself; a spec naming a channel → that channel's version; any other major.minor → `Err("name an exact version, such as 2.7.7")`; a channel the index lacks → `Ok(None)` (caller falls back to `[versions]`).
  - `pub fn parse_sha256sum(text: &str) -> Result<String, String>` — first field, 64 lowercase hex, else error.

- [ ] **Step 1: Write the failing tests**:
  - `php_below_compares_the_full_version`: composer recipe, PHP `7.2.4` → `2.2`; `7.2.5` → `2` with exclude `[2.2]`; `7.1.33` → `2.2`; `8.4.27` → `2`.
  - `an_unreadable_full_version_counts_as_patch_zero`: `php_full_or_zero(None, 7.2)` = `7.2.0`, giving `2.2`.
  - `requires_follow_from`: `2.2.32` → `None`; `2.3.0` and `2.11.2` → `7.2.5`.
  - `a_version_without_its_record_is_not_installed`: dirs `2.2.32` (record ok), `2.11.2` (no record), `.stage-1-0`, `junk`; only `2.2.32` returned; a record naming another version is ignored.
  - `install_records_round_trip`.
  - `composer_channels_read_the_saved_index`: from testdata, `"2"` → `2.11.2`, `"2.2"` → `2.2.32`, `"1"` → `1.10.28`.
  - `index_versions_follow_the_spec_rules`: `2` → 2.11.2, `2.2` → 2.2.32, `2.7.7` → 2.7.7, `2.7` → error containing `exact`, channels without `"2.2"` → `Ok(None)` for `2.2`.
  - `a_garbled_sha256sum_is_refused`: `"abc  composer.phar"` and `""` error; the real format parses.
- [ ] **Step 2: Run** `cargo test -p yard-core runtimes::select` — Expected: FAIL.
- [ ] **Step 3: Implement** in `select.rs` (`serde_json` is already a `yard-core` dependency).
- [ ] **Step 4: Run** the same — Expected: PASS.
- [ ] **Step 5: Commit** — `composer's rules, install records and getcomposer.org's index`.

---

### Task 7: The `composer` shim runs from the recipe

**Files:**
- Create: `crates/yard-core/src/runtimes/shim.rs`
- Modify: `crates/yard-core/src/shim.rs` (`PHP_SHIMS` loses `composer`; `command`'s `composer` arm goes; `SHIMS` and `WAKE_SHIMS` keep it), `crates/yard-shim/src/main.rs`, `crates/yard-core/src/runtimes/recipe_tests.rs` (drop Task 3's temporary filter)

**Interfaces:**
- Consumes: Tasks 3, 5, 6; `shim::{resolve_version, command, child_path, site_at}`, `Brew::php_opt`, `php::full_version`.
- Produces:
  - `pub fn refused<'a>(def: &'a ShimDef, args: &[OsString]) -> Option<&'a Refuse>` — matches when the first argument not starting with `-` equals `refuse.args[0]` and the following arguments start with the rest.
  - `pub fn recipe_command(recipe: &RuntimeRecipe, shim: &str, home: &Path, version: &RuntimeVersion, php: Option<&ShimCommand>, paths: &Paths, inherited: &OsStr) -> Result<ShimCommand, YardError>` — expands `exec` (`{php}` is the PHP shim command's program, and with `runs_on = "php"` the PHP command's env — `PHPRC`, `PHP_INI_SCAN_DIR` — is merged in); PATH is `{home}`'s `path` dirs, then the PHP keg's `bin` (with `runs_on`), then Yard's `bin`, then `child_path(None, inherited)`'s entries.
  - `pub enum Pick { Run(RuntimeVersion, PathBuf /*home*/), BrewComposer(PathBuf) }` and `pub fn pick_composer(recipe, pin: Option<(VersionSpec, PinSource)>, php_full: &RuntimeVersion, installed: &[(RuntimeVersion, InstallRecord)], brew_composer: Option<PathBuf>, paths: &Paths) -> Result<Pick, YardError>` — pin wins (plain prefix, no exclude); no pin → `default_spec`. `RUNTIME_MISSING` is only for nothing installed matching; a picked version whose `requires_php` exceeds `php_full` is `ErrorCode::InvalidArgument` with message `Composer 2.11.2 needs PHP 7.2.5 or later; this project runs PHP 7.1.33 (<php source>)` and hint `yard runtime install composer 2.2, or pin composer = "2.2" in the project's .yard`. Nothing matching and `installed` empty and `brew_composer` is `Some` and `php_full >= 7.2.5` → `BrewComposer`; otherwise `RUNTIME_MISSING` per Global Constraints.
- `yard-shim`: `run()` sends `composer` to `run_composer(args)`: load `yard.toml`/`sites.toml` as `run()` does; resolve PHP exactly as `run()` (same errors); build the PHP `command("php", …)`; `php_full_or_zero(full_version(opt link target))`; resolve the Composer pin with `resolve_pin` (`PinKey::Runtimes("composer")`, `from_file` = `runtimes["composer"]`, `from_site` = `runtimes["composer"]`), default from `yard.runtime_defaults["composer"]` before the recipe rules; list installs; `pick_composer`; `refused` → exit 1 `yard: composer <arg> is managed by Yard` + hint; the wake ask as today; `BrewComposer` prints the one stderr line from Global Constraints and runs `<keg>/bin/php <brew>/bin/composer` (today's behaviour); `YARD_SHIM_DEBUG=1` prints `yard-shim: composer runs Composer <v> (<spec> from <source>) on PHP <v>`.

- [ ] **Step 1: Write the failing tests** in `runtimes/shim.rs`:
  - `refuse_matches_the_first_non_option_argument`: `["self-update"]`, `["-n","self-update"]`, `["--no-ansi","selfupdate","--2.2"]` refused; `["install"]`, `["require","self-update/x"]`, `[]` not.
  - `the_phar_runs_under_the_projects_php`: program is the PHP keg's `bin/php`, pre is `[<home>/composer.phar]`, env has `PHPRC` and `PHP_INI_SCAN_DIR` when the ini exists, PATH order as above, no `Herd` entry.
  - `no_pin_picks_by_php`: installed `[2.2.32, 2.11.2]`; PHP 7.1.33 → 2.2.32; 8.4.27 → 2.11.2.
  - `only_the_lts_installed_is_missing_for_a_new_php`: installed `[2.2.32]`, PHP 8.4.27 → `RUNTIME_MISSING`, message `Composer 2 is not installed (the default)`, hint `yard runtime install composer 2`.
  - `a_pin_beats_the_rules`: pin `2.2` from a `.yard` on PHP 8.4 → 2.2.32.
  - `a_pin_too_new_for_the_php_is_refused`: pin `2` with only 2.11.2, PHP 7.1.33 → error message as above.
  - `brews_composer_runs_only_when_yard_has_none`: no installs, brew composer present, PHP 8.4 → `BrewComposer`; PHP 7.1 → `RUNTIME_MISSING` for 2.2; one Yard version installed → never `BrewComposer`.
  - In `shim.rs`: replace `composer_runs_the_phar_under_php` with `composer_is_not_a_php_command` (`command("composer", …)` errors) — this is the one existing test this plan changes, because `command` loses that arm.
- [ ] **Step 2: Run** `cargo test -p yard-core runtimes::shim shim::` — Expected: FAIL.
- [ ] **Step 3: Implement** both files and `yard-shim`'s `run_composer`.
- [ ] **Step 4: Run** `cargo test -p yard-core && cargo test -p yard-shim` — Expected: PASS. Then by hand against a scratch root: `YARD_ROOT=$(mktemp -d) target/debug/yard-shim` invoked through a `composer` symlink in a temp project with no installs prints the `RUNTIME_MISSING` message (or the brew line where brew's composer exists).
- [ ] **Step 5: Commit** — `composer runs the version its project's php can run`.

---

### Task 8: yardd installs and uninstalls runtime versions

**Files:**
- Modify: `crates/yard-daemon/src/programs.rs` (download generalized), `crates/yard-daemon/src/daemon.rs` (registry on `Context`), `crates/yard-daemon/src/lib.rs`
- Create: `crates/yard-daemon/src/runtimes.rs`
- Test: `crates/yard-daemon/tests/runtimes.rs`

**Interfaces:**
- Consumes: Tasks 3, 5, 6.
- Produces:
  - In `programs.rs`: `pub async fn fetch(job: &JobCtx, stage: &Path, url: &str, sha256: &str, strip: u32, file: Option<&str>, opts: &LoadOptions) -> Result<PathBuf, YardError>` (today's `fetch_into`, with `file` naming a bare download) and `pub fn stage_in(dir: &Path) -> PathBuf`. The engine path keeps calling them; its behaviour is unchanged.
  - In `runtimes.rs`:
    - `pub fn registry() -> Runtimes` (carried, this plan).
    - `pub fn installed(paths: &Paths, key: &str) -> Vec<(RuntimeVersion, InstallRecord)>` (Task 6's over the real filesystem).
    - `pub async fn resolve_install(job: &JobCtx, recipe: &RuntimeRecipe, spec: &VersionSpec, opts: &LoadOptions) -> Result<(RuntimeVersion, String /*url*/, String /*sha256*/), YardError>` — with an index: `curl -fsSL --proto =https` the index (via `JobCtx::capture_stdout`), `composer_channels`, `index_version`; `Ok(None)` or an unreachable index falls back to the newest `[versions]` entry matching the spec; the hash comes from `[versions]` when the version is listed there, else `sha256_url` fetched and `parse_sha256sum`'d. Without an index: `[versions]` only.
    - `pub async fn install(job: &JobCtx, paths: &Paths, recipe: &RuntimeRecipe, spec: VersionSpec, opts: &LoadOptions) -> Result<RuntimeVersion, YardError>` — an installed version matching exactly → line `Composer 2.2.32 is installed`, done; else stage in `installs/<key>/`, `fetch`, write `InstallRecord` (with `requires_php`) into the stage root, rename to `install_root`, line `Installed Composer 2.2.32`.
    - `pub fn in_use(paths: &Paths, recipe: &RuntimeRecipe, v: &RuntimeVersion, sites: &[SiteInfo], default: Option<&VersionSpec>) -> Result<(), YardError>` — `RUNTIME_IN_USE` when `v` is the only installed version matching the default spec or a linked site's resolved spec (Task 9's `SiteRuntime`), naming the site and the edit (`edit <path>/.yard`, or `yard runtime unuse composer --site <s>`).
    - `pub async fn uninstall(job, paths, recipe, v)` — `remove_dir_all(install_root)`; refuses a `[versions]` `path` entry.
  - `Context` gains `runtimes: Runtimes`; `DaemonOptions` unchanged (`allow_file_urls` covers tests; the test recipe is a carried-shaped composer recipe whose URLs point at `file://` fixtures, built with `Origin::Carried` so it may have an index).

- [ ] **Step 1: Write the failing tests** in `tests/runtimes.rs` (fixture dir with `versions` JSON, `2.2.32/composer.phar`, `2.2.32/composer.phar.sha256sum`, `2.11.2/…`; a test-only recipe text with `file://` URLs loaded with `allow_file_urls`):
  - `installs_a_channel_from_the_index` — spec `2.2` → `installs/composer/2.2.32/composer.phar` exists, record has `requires_php = None` and the fixture's hash.
  - `installs_the_current_line_with_its_requirement` — spec `2` → 2.11.2, record `requires_php = 7.2.5`.
  - `an_installed_version_is_not_fetched_again` — second install's job lines contain `is installed`, the fixture file's mtime/a removed fixture does not matter.
  - `a_bad_hash_leaves_nothing` — fixture sha256sum wrong → `DOWNLOAD_HASH_MISMATCH`, `installs/composer/` has no `.stage-*` and no `2.2.32`.
  - `an_unreachable_index_falls_back_to_the_pinned_lts` — index URL to a missing file, spec `2.2` → installs 2.2.32 with the recipe's pinned hash; spec `2` → error naming the index URL.
  - `a_partial_minor_is_refused_before_downloading` — spec `2.7` → `INVALID_ARGUMENT` containing `exact`.
  - `uninstall_removes_only_that_version`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test runtimes` — Expected: FAIL.
- [ ] **Step 3: Implement**; keep `programs.rs`'s engine tests green.
- [ ] **Step 4: Run** `cargo test -p yard-daemon` — Expected: PASS.
- [ ] **Step 5: Commit** — `yardd installs composer versions from getcomposer.org`.

---

### Task 9: `runtime.*`, `runtimes.list` and each site's runtimes

**Files:**
- Create: `crates/yard-daemon/src/rpc/runtimes.rs`
- Modify: `crates/yard-daemon/src/rpc/mod.rs` (dispatch), `crates/yard-daemon/src/runtimes.rs` (`fill_runtimes`), the places that call `crate::node::fill_node` (`rpc/info.rs`, `doctor.rs`, `rpc/node.rs`, the sites list) also call `fill_runtimes`, `crates/yard-daemon/src/managed.rs` use (no change expected)
- Test: `crates/yard-daemon/tests/runtimes.rs`

**Interfaces:**
- Consumes: Tasks 1, 4–8.
- Produces:
  - `pub fn fill_runtimes(sites: &mut [SiteInfo], paths: &Paths, runtimes: &Runtimes, brew: &Brew)` — for each linked site and each `Recipe`-kind runtime the site uses (`uses_runtime` with `uses_when` files in its root): a `SiteRuntime` with the resolved spec (pin, else `yard.toml` default, else `default_spec` on the site's PHP full version), its `SettingSource` (`ProjectFile`, `ManagedFile`, `SitesToml`, `PinFile`, `Default`), file, the installed version `pick` gives, or the error; plus one `SiteRuntime` with `error: RUNTIME_UNKNOWN` (hint `add ~/.yard/runtimes/<key>.toml`) for each `[runtimes]` key no runtime defines. PHP and Node do not appear in `runtimes` (they have `php` and `node`).
  - Handlers:
    - `runtimes.list` → `RuntimesList` (every runtime, `origin` `"carried"`, `installed` from the driver: brew kegs for php, `node::installed` for node, Task 8's for recipes; `errors: []` this plan).
    - `runtime.list {key}` → `RuntimeList`; `php`/`node` built from `php.list`/`node.list`'s answers.
    - `runtime.install {key, spec|path}` → job; `php` → `php.install`, `node` → `node.install`, recipes → Task 8 (`path` given: the spec that directory resolves to, as `node.install` does; neither: `INVALID_ARGUMENT`, hint `yard runtime install composer 2`; with neither spec nor a pin in a path-less call from outside a project, the CLI sends `spec = "2"` per spec §9.3).
    - `runtime.uninstall {key, version}` → job, `in_use` checked before and at job start; php/node delegate.
    - `runtime.default {key, spec}` → writes `[runtimes.<key>] default` into `yard.toml` with toml_edit (keeps comments); the spec must match an installed version; php → `php.default`, node → `node.default`.
    - `runtime.use {key, site, spec}` / `runtime.unuse {key, site}` → committed `.yard` refused `SITE_HAS_PROJECT_FILE` (hint `edit <path>/.yard and set [runtimes] <key>`); managed file → `[runtimes] <key>` via `edit_managed`; else `[sites.<s>.runtimes] <key>` in `sites.toml`. php → `isolate`/`unisolate`, node → `node.use`/`node.unuse`.
    - An unknown key → `RUNTIME_UNKNOWN`, hint `yard runtimes lists them`.

- [ ] **Step 1: Write the failing tests** (daemon fixture as the Node RPC tests use):
  - `runtimes_list_names_the_three_carried`.
  - `a_site_with_composer_json_shows_its_composer` — linked project with `composer.json` on PHP 7.1 (fake keg `7.1.33`) shows `spec "2.2"`, `source Default`, `version None` until 2.2.32 is installed, then `"2.2.32"`.
  - `a_site_without_composer_json_shows_none`.
  - `use_writes_the_managed_file_and_unuse_removes_it`, `use_is_refused_for_a_committed_yard`, `use_writes_sites_toml_without_a_managed_file`.
  - `default_writes_yard_toml_and_needs_an_installed_match`.
  - `uninstall_refuses_the_only_version_a_site_runs` (`RUNTIME_IN_USE`, message names the site).
  - `an_unknown_runtime_in_yard_is_reported_not_fatal` — `[runtimes] bun = "1"`: the site serves, `runtimes` has `bun` with `RUNTIME_UNKNOWN`.
  - `runtime_install_node_delegates` — `runtime.install {key:"node", spec:"22"}` starts the same job kind as `node.install` (`node-install`).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test runtimes` — Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test -p yard-daemon` — Expected: PASS.
- [ ] **Step 5: Commit** — `runtime methods and each site's composer`.

---

### Task 10: Doctor

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs`
- Test: `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Consumes: Task 9's `fill_runtimes`.
- Produces checks (names exact):
  - `runtime-missing:<site>:<key>` (warn): the site's runtime resolves to a spec nothing installed meets; hint `yard runtime install <key> <spec>`.
  - `runtime-spec:<site>:<key>` (warn): the pin cannot be used (its `error` that is not `RUNTIME_UNKNOWN`); hint from the error.
  - `runtime-unknown:<site>:<key>` (warn): hint `add ~/.yard/runtimes/<key>.toml`.
  - `composer-php:<site>` (warn): the site's Composer version needs a newer PHP than the site runs; hint as the shim's.
  - `composer-brew` (ok, with hint `yard runtime install composer`): brew's `bin/composer` exists and no Yard Composer is installed.

- [ ] **Step 1: Write the failing tests**: `doctor_warns_a_sites_missing_composer`, `doctor_warns_a_composer_too_new_for_the_sites_php`, `doctor_names_an_unknown_runtime`, `doctor_notes_brews_composer_while_yard_has_none` (status `ok`, hint present), `doctor_is_quiet_for_a_site_without_composer_json`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test doctor` — Expected: FAIL.
- [ ] **Step 3: Implement** beside `node_checks`.
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test doctor` — Expected: PASS.
- [ ] **Step 5: Commit** — `doctor checks each site's composer`.

---

### Task 11: CLI

**Files:**
- Modify: `crates/yard-cli/src/cli.rs`, `crates/yard-cli/src/commands/mod.rs`, `crates/yard-cli/src/commands/info.rs`, `crates/yard-cli/src/commands/init.rs`
- Create: `crates/yard-cli/src/commands/runtime.rs`
- Test: `crates/yard-cli/tests/runtimes.rs` (+ snapshots), existing `crates/yard-cli/tests/cli.rs` help snapshot

**Interfaces:**
- Consumes: Task 1's methods and types.
- Produces:
  - `yard runtimes` — table `RUNTIME  FROM  SHIMS  INSTALLED  DEFAULT` (`FROM` is `carried` or the file), `--json` the `RuntimesList`.
  - `yard runtime list <key>`; `yard runtime install <key> [<spec>]` (no spec: `path` = cwd; for `composer` outside any linked project and with no pin, `spec = "2"`); `yard runtime uninstall <key> <version>`; `yard runtime default <key> <spec>`; `yard runtime use <key> <spec> [--site <name>]`; `yard runtime unuse <key> [--site <name>]`. Help text names `yard node` and `yard php` as the same for those keys. Output and errors follow `.hydra/rules/cli-output.md` (human text, `--json` for agents, `yard:`/`hint:` errors).
  - `yard info`: one line per `SiteRuntime`, `Composer 2.2 → 2.2.32 (default)` / `Composer 2 → not installed: yard runtime install composer 2` / `bun: RUNTIME_UNKNOWN …`.
  - `settle` (`link`/`init`): a site's `SiteRuntime` with `version: None` and no `error` is listed as `Composer 2.2  (yard runtime install composer 2.2)` beside Node's line and installed after Node, with Node's warning-not-failure rule (a failed install warns, the command succeeds, since the site serves without it).

- [ ] **Step 1: Write the failing tests**: snapshot `runtimes_lists_the_carried_three`; `runtime_install_composer_runs_a_job` (fake daemon fixture as the node CLI tests use); `info_shows_a_sites_composer`; `link_offers_composer_and_installs_it_with_yes`; `link_without_yes_and_no_tty_serves_and_hints_composer` (exit 0, stderr contains `yard runtime install composer 2.2`); `there_is_no_yard_composer_command` (`yard composer install` exits 2, usage error).
- [ ] **Step 2: Run** `cargo test -p yard-cli --test runtimes` — Expected: FAIL.
- [ ] **Step 3: Implement**; update the help snapshot in `cli.rs` tests.
- [ ] **Step 4: Run** `cargo test -p yard-cli` — Expected: PASS.
- [ ] **Step 5: Commit** — `yard runtime and yard runtimes, and link offers composer`.

---

### Task 12: Yard.app shows a site's Composer

**Files:**
- Modify: `app/yard-app-model/src/settle.rs`, `app/yard-app/src/sites/detail.rs`
- Test: `app/yard-app-model/src/settle.rs` tests, `app/yard-app/src/sites/tests.rs`

**Interfaces:**
- Consumes: `SiteInfo.runtimes`, `methods::RUNTIME_INSTALL`, `RuntimeInstallParams`.
- Produces: in the site detail, below Node, one row per `SiteRuntime`: `Composer` and `2.2.32 (2.2, default)`, or `2.2 — not installed` with an Install button that runs `runtime.install {key, spec}` as a job (as the Node row's install does); an error row shows its message. `settle` lists a missing runtime as an offered install after Node. An older yardd (no `runtimes`) shows no row.

- [ ] **Step 1: Write the failing tests**: `settle_offers_a_missing_composer_after_node`; `detail_rows_show_composer_installed_and_missing` (the model-level rows the detail renders).
- [ ] **Step 2: Run** in `app/`: `cargo test --workspace --locked` — Expected: FAIL.
- [ ] **Step 3: Implement**, following `.hydra/rules/app.md` (accessibility labels on the new row and button).
- [ ] **Step 4: Run** in `app/`: `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` — Expected: PASS.
- [ ] **Step 5: Commit** — `yard.app shows a site's composer`.

---

### Task 13: Docs and the real-machine script

**Files:**
- Create: `scripts/e2e-composer.sh`
- Modify: `docs/superpowers/specs/2026-10-19-yard-design.md` (§3.3 data layout: `installs/`, `runtimes/`, `cache/`; §4.5: the `composer` shim paragraph points at the runtimes spec §9), `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md` (Status: plan 1 built, what changed), `AGENTS.md` (Status, Layout's `yard-core` line names `runtimes/`, Build & test lists `e2e-composer.sh`), `README.md` (Composer per project, `yard runtime`, `yard runtimes`), `DEVELOPMENT.md` (`e2e-composer.sh`), `.hydra/rules/cli-output.md`, `.hydra/rules/config-ownership.md` (`installs/`, `.yard-install.toml` generated, `[runtimes]` in managed `.yard` edited by `runtime use`), `.hydra/rules/rpc-protocol.md` (2.14 and its methods), `.hydra/rules/homebrew.md` (Composer no longer from brew; the one-release fallback), `.hydra/rules/rust-workspace.md` (`runtimes/` in yard-core)

**Interfaces:**
- Consumes: everything above.
- Produces: `scripts/e2e-composer.sh`, shellcheck-clean, bash 3.2, in the shape of `e2e-m4c.sh`: backs up and restores `yard.toml` and `sites.toml`; refuses when `installs/composer/` already has versions it did not install (so it never removes the maintainer's); in two scratch projects under `$TMPDIR` whose `.yard` pins `php = "7.1"` and `php = "8.4"` (needs both installed; installs no PHP), each with a `composer.json` requiring nothing: `yard link --yes` offers and installs Composer (2.2.x for 7.1, 2.x for 8.4); `~/.yard/bin/composer -V` prints `Composer version 2.2.` and `Composer version 2.` + not `2.2.` respectively; `composer install` succeeds in both; `[runtimes] composer = "2.2"` added to the 8.4 project's `.yard` makes `-V` print 2.2; `composer self-update` and `composer -n self-update` exit 1 with `managed by Yard`; `yard runtime uninstall composer <v>` is refused while a site runs it; cleanup unlinks the scratch sites, uninstalls only the versions it installed, and fails if Herd's resolver changed.

- [ ] **Step 1: Write the script** and run `shellcheck scripts/e2e-composer.sh` — Expected: no output.
- [ ] **Step 2: Update the docs** listed above; then grep for every touched name and fix what no longer matches: `grep -rn "brew install composer\|composer is not installed\|PHP_SHIMS\|2\.13" README.md DEVELOPMENT.md AGENTS.md docs .hydra crates app --include=*.md --include=*.rs`. Expected: only historical mentions in older specs' status notes.
- [ ] **Step 3: Full verification**: root `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` (Expected: all pass, test count above 1458) and `cargo deny check` only if `Cargo.lock` changed.
- [ ] **Step 4: Commit** — `runtimes plan 1 documented, and e2e-composer.sh`.
- [ ] **Step 5: Ask the maintainer** before running `scripts/e2e-composer.sh` on a real machine (it downloads Composer and links scratch sites against the real yardd built from this checkout: `cargo build --release`, `yard stop && yard start`). Afterwards `cargo clean --profile dev`.
