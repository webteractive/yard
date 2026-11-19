# Explicit Runtime Defaults Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every runtime's default is an exact version written in `yard.toml`, filled in by yardd
when missing, and never moved by an install; old-PHP projects carry their own Composer pin.

**Architecture:** `yard-core` loses its compiled-in fallbacks (`PhpVersion::DEFAULT` as a
default, `node::DEFAULT_SPEC`, the recipe `[[default]]` rules) and gains pure helpers that say
what to fill in and which sites need a pin. `yard-daemon` runs one fill-in check at the start of
every reconcile, writing through `TomlFile::edit`, and keeps the last good defaults in
`run/defaults.toml` for the shims. Display and protocol 2.15 carry where each version came from.

**Tech Stack:** Rust 1.92 workspace (`yard-core`, `yard-daemon`, `yard-proto`, `yard-cli`,
`yard-shim`), Yard.app on Rust 1.98.1 in `app/`.

**Spec:** `docs/superpowers/specs/2026-11-19-yard-explicit-defaults-design.md` (approved
2026-11-19), amending `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md`.

## Global Constraints

- Exact means: PHP a line (`"8.4"`), Node a full version (`"22.23.5"`), Composer a full version (`"2.11.2"`).
- Keys stay: `default_php`, `default_node`, `[runtimes.<key>] default`; `yard.toml` still refuses `[runtimes.php]` and `[runtimes.node]`.
- A written default is never rewritten by Yard; only `yard runtime default` (and `yard php|node default`) moves it.
- Fill-in values: PHP `8.4` when installed else the newest installed line; Node the newest installed 22.x else the newest installed; Composer/recipe the newest installed version at or above the recipe's newest `[[requires]] from` (2.3.0), else nothing; a recipe without `[[requires]]` the newest installed.
- An old-PHP pin is the newest installed version whose `requires_php` the site's PHP meets (2.2.32 today), written to the managed `.yard` or `sites.toml` (where `yard runtime use` writes); a committed `.yard` is never edited.
- A hand-written non-exact default is read as a prefix and warned about by doctor as `runtime-default-not-exact:<key>`; it is never a config error.
- Doctor `info` check `runtime-default-written:<key>` per value written since yardd started, message `wrote <key line> to yard.toml; yard runtime default <key> <v> changes it`.
- Shims fall back to `run/defaults.toml` (written by yardd after each good `yard.toml` load) when `yard.toml` does not parse, saying so on stderr; with no such file an unpinned project fails `CONFIG_INVALID`.
- Protocol `2.14` → `2.15`.
- Commit messages: lowercase prose subject, prose paragraphs, no type prefix, no trailers (no `Co-Authored-By`, no `Claude-Session`). Commit only with the maintainer's approval; work on main.
- After builds: `cargo clean --profile dev`, never plain `cargo clean`.

## Review Focus

1. A PHP 7.1 project linked before any Composer exists: 2.2.32 installs and is pinned on that site, and no default is written (the default stays unset until a 2.3+ install). Test in Task 4.
2. The user deletes `default_php` from `yard.toml` while sites serve: the next reconcile writes it back and no site goes to its setup page in between. Test in Task 4.
3. A hand edit leaves `yard.toml` unparsable while `composer` and `php` run in an unpinned project: they run the last good defaults with a warning, not a compiled-in version. Test in Task 3.
4. The fill-in's own write triggers the watcher: the next reconcile writes nothing and does not loop. Test in Task 4.
5. A committed `.yard` on PHP 7.1 with a `composer.json` and no pin: never edited, the shim refuses with the line to add, doctor names it. Test in Task 6.

---

### Task 1: no compiled-in defaults in yard-core's config and resolution

**Files:**
- Modify: `crates/yard-core/src/config/yard.rs`, `crates/yard-core/src/php.rs` (rename the const), `crates/yard-core/src/node.rs`, `crates/yard-core/src/shim.rs`, `crates/yard-core/src/sites.rs`
- Test: the same files' `#[cfg(test)]` modules

**Interfaces:**
- Produces: `YardConfig.default_php: Option<PhpVersion>`; `PhpVersion::FIRST_DEFAULT` (8.4, used only for the fill-in and as the setup page's offer when nothing is written); `yard_core::node::FIRST_DEFAULT: NodeSpec` (`Major(22)`, fill-in only); `YardConfig::php_default(&self) -> Result<PhpVersion, YardError>` (`PHP_MISSING`-style error: code `RuntimeMissing`, message `no default PHP is written in yard.toml`, hint `yard php default <version>`); `resolve_version` and `resolve_node` return that error for an unpinned cwd with no default; `resolve_node`'s `None` default is `NODE_MISSING` with hint `yard node default <version>`.

- [ ] **Step 1: Write the failing tests**
  - `config::yard`: `an_unset_default_php_is_none` (`parse(Some("")).unwrap().default_php == None`), `a_written_default_php_parses` (`"8.4"` → `Some(8.4)`).
  - `shim`: `an_unpinned_cwd_with_no_default_php_is_runtime_missing` (empty yard.toml, no `.yard`, no site: `resolve_version` errs with code `RuntimeMissing` and hint `yard php default <version>`); `an_unpinned_cwd_with_no_default_node_is_node_missing`.
  - `sites`: `a_site_with_no_php_and_no_default_offers_first_default` (site resolves `php == "8.4"`, so the setup page asks for 8.4 when it is not installed).
- [ ] **Step 2: Run** `cargo test -p yard-core --locked` — Expected: the new tests fail to compile or fail.
- [ ] **Step 3: Implement.** `Raw.default_php` already optional; keep it `Option` in `YardConfig`. Every `yard.default_php` reader moves to `php_default()?` except `sites.rs`, which uses `cfg.default_php.unwrap_or(PhpVersion::FIRST_DEFAULT)` (the daemon fills before resolving, so this only ever offers 8.4 on a Mac with no PHP). Rename `PhpVersion::DEFAULT` → `FIRST_DEFAULT` and `node::DEFAULT_SPEC` → `FIRST_DEFAULT`, fixing callers in `yard-daemon` (`doctor.rs`, `rpc/runtimes.rs`, `rpc/node.rs`, `node.rs`) to read the written value and treat `None` as "no default".
- [ ] **Step 4: Run** `cargo test --workspace --locked 2>&1 | tail -30` — Expected: all pass.
- [ ] **Step 5: Commit** — `git commit -m "yard-core reads defaults only from yard.toml"` (prose body per Global Constraints).

### Task 2: the recipe format without `[[default]]` rules

**Files:**
- Modify: `crates/yard-core/src/runtimes/recipe.rs`, `crates/yard-core/src/runtimes/select.rs`, `crates/yard-core/src/runtimes/shim.rs`, `crates/yard-core/runtimes/composer.toml`, `crates/yard-core/src/runtimes/recipe_tests.rs`, `crates/yard-core/src/runtimes/testdata/*`
- Test: `recipe_tests.rs`, `select.rs` and `shim.rs` test modules

**Interfaces:**
- Consumes: Task 1's `YardConfig`.
- Produces (all in `yard_core::runtimes::select`):
  - `requires_boundary(recipe: &RuntimeRecipe) -> Option<RuntimeVersion>` — the newest `[[requires]] from`.
  - `fill_candidate(recipe: &RuntimeRecipe, installed: &[RuntimeVersion]) -> Option<RuntimeVersion>` — newest installed `>=` the boundary, or newest installed when there is no boundary.
  - `runs_on(recipe: &RuntimeRecipe, v: &RuntimeVersion, php_full: &RuntimeVersion) -> bool` — `requires_php(recipe, v)` is `None` or `<= php_full`.
  - `needs_pin(recipe: &RuntimeRecipe, default: Option<&VersionSpec>, installed: &[RuntimeVersion], php_full: &RuntimeVersion) -> bool` — with a default: the version it picks (`pick`) does not run on `php_full`; with none: `php_full` is below the boundary's `requires_php`.
  - `fitting(recipe: &RuntimeRecipe, installed: &[RuntimeVersion], php_full: &RuntimeVersion) -> Option<RuntimeVersion>` — newest installed that `runs_on`.
  - `offer_spec(recipe: &RuntimeRecipe, php_full: &RuntimeVersion, below: bool) -> Option<VersionSpec>` — what a site with nothing installed is offered: with an index, `composer_channel` of the newest `[versions]` entry that runs on `php_full` when `below`, else of the boundary (`2`); without an index, that `[versions]` entry exact.
  - `pick_composer(recipe, pin: Option<(VersionSpec, PinSource)>, php_full, php_from, installed, brew_composer, paths)` keeps its signature; the caller passes `yard.toml`'s default as the pin. With no pin: `BrewComposer` when brew's runs on the PHP, else `RUNTIME_MISSING` naming `yard runtime install composer` (no spec).
  - Removed: `DefaultRule`, `RuntimeRecipe.defaults`, `select::default_spec`, `exclude`.

- [ ] **Step 1: Write the failing tests**
  - `a_recipe_with_a_default_table_is_refused` — `[[default]]` in a recipe parses to a `RecipeError` whose message contains `[[default]] is no longer read`.
  - `fill_candidate_skips_versions_below_the_boundary` — installed `[2.2.32]` → `None`; `[2.2.32, 2.11.2]` → `2.11.2`.
  - `needs_pin_without_a_default_follows_the_boundary` — PHP 7.1.33 → true, 8.4.15 → false.
  - `needs_pin_with_a_default_follows_what_it_picks` — default `2.11.2` with PHP 7.1.33 → true; default `2.2.32` with PHP 7.1.33 → false.
  - `fitting_takes_the_newest_the_php_runs` — installed `[2.2.32, 2.11.2]`, PHP 7.1.33 → `2.2.32`.
  - `offer_spec_is_the_lts_channel_below_and_the_main_one_above` — `below` true → `2.2`, false → `2`.
  - `pick_composer_with_no_pin_and_installs_is_missing_not_a_rule` — installed `[2.11.2]`, no pin, PHP 8.4 → `RUNTIME_MISSING` (the shim never picks by rule).
- [ ] **Step 2: Run** `cargo test -p yard-core --locked runtimes` — Expected: FAIL.
- [ ] **Step 3: Implement** the functions above; drop the `[[default]]` blocks and comment from `composer.toml`; replace every `default_spec` caller in `yard-daemon` (`runtimes.rs` `spec_for`/`in_use`, `rpc/runtimes.rs` `default_words`/`spec_at`/`list`) with the written default or `offer_spec`, and drop `in_use`'s exclude branch.
- [ ] **Step 4: Run** `cargo test --workspace --locked 2>&1 | tail -30` — Expected: all pass (update any test that asserted rule text such as `2.2 below PHP 7.2.5, else 2`).
- [ ] **Step 5: Commit** — `"runtime recipes lose their default rules"`.

### Task 3: shims run the last good defaults when yard.toml is broken

**Files:**
- Modify: `crates/yard-core/src/config/yard.rs`, `crates/yard-core/src/paths.rs`, `crates/yard-shim/src/main.rs`
- Test: `config::yard` tests, `crates/yard-shim` tests (or `yard-daemon/tests/` if the shim's are there; follow where `run_node`'s fallback is tested today)

**Interfaces:**
- Produces: `Paths::defaults_toml(&self) -> PathBuf` (`<root>/run/defaults.toml`); `config::yard::render_defaults(&YardConfig) -> String` (only `default_php`, `default_node`, `[runtimes.<k>] default`); `config::yard::load_yard_for_shim(paths: &Paths) -> Result<(YardConfig, Option<String>), YardError>` — `yard.toml` when it parses; else `run/defaults.toml` parsed with the same parser, with `Some("yard: <error>; using the last good defaults from <file>")`; else the original `CONFIG_INVALID`.

- [ ] **Step 1: Write the failing tests**
  - `render_defaults_round_trips` — parse `render_defaults(c)` and compare the three default fields.
  - `a_broken_yard_toml_falls_back_to_the_last_good_defaults` — write `default_php = "7.4"` to `run/defaults.toml` and `tld = ` (broken) to `yard.toml`: `load_yard_for_shim` gives `default_php == Some(7.4)` and a warning naming `run/defaults.toml`.
  - `a_broken_yard_toml_with_no_last_good_is_config_invalid`.
- [ ] **Step 2: Run** `cargo test -p yard-core --locked config::yard` — Expected: FAIL.
- [ ] **Step 3: Implement**, and switch the three shim entry points (`Php::resolve`, `run_composer`, `run_node`) to `load_yard_for_shim`, printing its warning; delete the `parse_yard(None, …)` fallbacks and their "using PHP 8.4" / "using Node 22" text.
- [ ] **Step 4: Run** `cargo test --workspace --locked 2>&1 | tail -30` — Expected: all pass.
- [ ] **Step 5: Commit** — `"shims use the last good defaults, never a built-in one"`.

### Task 4: yardd fills in missing defaults and old-PHP pins on every reconcile

**Files:**
- Create: `crates/yard-daemon/src/defaults.rs` (and `pub mod defaults;` in `lib.rs`)
- Modify: `crates/yard-daemon/src/reconcile.rs` (call it first in `run`, write `run/defaults.toml` after a good load, keep what was written in `State`), `crates/yard-daemon/src/rpc/runtimes.rs` and `rpc/node.rs` (end install jobs with a reconcile, as `php.install` does)
- Test: `crates/yard-daemon/tests/runtimes.rs` (and `node.rs` for Node)

**Interfaces:**
- Consumes: Task 1 (`FIRST_DEFAULT`s), Task 2 (`fill_candidate`, `needs_pin`, `fitting`), Task 3 (`render_defaults`, `Paths::defaults_toml`).
- Produces: `pub struct Written { pub key: String, pub line: String }` (`line` as written, e.g. `default_php = "8.4"`, `[runtimes.composer] default = "2.11.2"`, `site php71: [runtimes] composer = "2.2.32"`); `pub fn fill(paths: &Paths, runtimes: &Runtimes, brew: &Brew) -> Vec<Written>` — reads `yard.toml` and `sites.toml` itself and does nothing when either does not parse; each write is its own `TomlFile::edit` (sites through the same managed-file/`sites.toml` target `set_site_runtime` uses, factored out as `pub(crate) fn site_runtime_target(...)` in `rpc/runtimes.rs` or moved beside `fill`); errors are logged and skipped, never fail the reconcile. `State.defaults_written: Vec<Written>` accumulates since start.

- [ ] **Step 1: Write the failing tests** (daemon fixtures with fake brew kegs and fake installs, as `tests/runtimes.rs` already builds them)
  - `a_fresh_root_with_php_installed_gets_default_php_written` — kegs 7.4 and 8.4, empty `yard.toml`: after start, `yard.toml` contains `default_php = "8.4"`; with only 7.4, `"7.4"`.
  - `a_deleted_default_php_is_written_back` — remove the line, wait for the reconcile: it is back; `sites.list` never shows a site on its setup page meanwhile (poll during the wait).
  - `a_written_default_is_never_moved_by_an_install` — default `2.11.2`, install fake `2.12.0`: `yard.toml` still says `2.11.2`.
  - `linking_an_old_php_project_first_pins_it_and_writes_no_default` — only `2.2.32` installed, a linked 7.1 site with `composer.json`: its managed `.yard` (or `sites.toml`) gains `composer = "2.2.32"`, and `yard.toml` has no `[runtimes.composer]`.
  - `isolating_below_7_2_5_pins_composer` — a managed site on 8.4 with `composer.json`, `2.2.32` and `2.11.2` installed, default `2.11.2`: `yard isolate 7.1` leaves its managed `.yard` with `php = "7.1"` and `composer = "2.2.32"` after the reconcile.
  - `the_composer_default_needs_the_main_line` — `2.2.32` and `2.11.2` installed: default `2.11.2`.
  - `the_fill_in_does_not_loop` — after one fill, count reconciles over 5 s: no further writes (the file's mtime and `defaults_written` stay put).
  - `node_fills_the_newest_22` — installed 20.x, 22.x, 24.x, no `default_node`: written as the 22.x exact version.
  - `run_defaults_toml_follows_a_good_yard_toml` — after start, `run/defaults.toml` holds the written defaults.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --locked --test runtimes` — Expected: FAIL.
- [ ] **Step 3: Implement** `fill` in this order: PHP, Node, each recipe runtime, then the old-PHP pins (linked PHP sites whose root has a `uses_when` file, no pin from `resolve_at`, no committed `.yard`, `needs_pin` true, `fitting` some). Log each `Written` with `tracing::info!`.
- [ ] **Step 4: Run** `cargo test --workspace --locked 2>&1 | tail -30` — Expected: all pass (existing tests that assumed no `default_php` in `yard.toml` after start may need their expected file text updated: that is the new behaviour, not a regression).
- [ ] **Step 5: Commit** — `"yardd writes missing defaults and old projects' composer pins"`.

### Task 5: guarding and changing the default

**Files:**
- Modify: `crates/yard-daemon/src/rpc/runtimes.rs` (`set_default`, `list_all`, `list`, `spec_at`), `crates/yard-daemon/src/rpc/node.rs` (`set_default`), `crates/yard-daemon/src/runtimes.rs` (`in_use`), `crates/yard-daemon/src/node.rs` (`in_use`), `crates/yard-proto/src/methods.rs`/`types.rs`, `crates/yard-cli/src/commands/runtime.rs`, `crates/yard-cli/src/commands/node.rs`
- Test: `crates/yard-daemon/tests/runtimes.rs`, `tests/node.rs`, `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Produces: `runtime.default` answers `RuntimeDefaultSet { unfit: Vec<UnfitSite> }`, `UnfitSite { site: String, hint: String }` (hint `yard runtime use composer <fitting>` or, for a committed `.yard`, `add [runtimes] composer = "<fitting>" to <file>`); `RuntimeSummary.default: Option<String>` is the written value or `None`; `RuntimeList.default` likewise (`""` → `None` is a proto change, so 2.15).

- [ ] **Step 1: Write the failing tests**
  - `runtime_default_refuses_a_prefix` — `yard runtime default composer 2` → `INVALID_ARGUMENT`, message lists the installed `2.x` versions, hint `yard runtime default composer 2.11.2`.
  - `node_default_refuses_a_prefix` — `yard node default 22` → same shape with the installed 22.x.
  - `php_default_takes_a_line` — `yard php default 8.4` still works.
  - `the_default_version_cannot_be_uninstalled` — default `2.11.2`, `2.12.0` also installed: uninstall `2.11.2` → `RUNTIME_IN_USE` naming `yard.toml` (exact default, no "another matching version" escape).
  - `moving_the_default_lists_sites_that_cannot_run_it` — a 7.1 site pinned nowhere (committed `.yard`) and default moved to `2.11.2`: `unfit` names it with the `.yard` line.
  - CLI: `yard runtimes` prints `none` in DEFAULT for a runtime with nothing written.
- [ ] **Step 2: Run** `cargo test --workspace --locked runtime_default node_default the_default_version moving_the_default` — Expected: FAIL.
- [ ] **Step 3: Implement**; bump `PROTOCOL_VERSION` to `2.15` and follow `.hydra/rules/rpc-protocol.md` for the method and type changes (an older CLI must still read the answer or fail clearly).
- [ ] **Step 4: Run** `cargo test --workspace --locked 2>&1 | tail -30` — Expected: all pass.
- [ ] **Step 5: Commit** — `"defaults are exact, and moving one says who it leaves behind"`.

### Task 6: seeing where each version comes from

**Files:**
- Modify: `crates/yard-proto/src/types.rs` (`SiteInfo.php_source: Option<SettingSource>`), `crates/yard-core/src/sites.rs` (fill it), `crates/yard-daemon/src/doctor.rs`, `crates/yard-cli/src/commands/sites.rs`, `crates/yard-cli/src/commands/info.rs`, `crates/yard-cli/src/commands/init.rs` (settle's message for a committed `.yard`), `crates/yard-shim/src/main.rs` (composer refusal hint), `app/yard-app/src/sites/detail.rs`
- Test: `crates/yard-daemon/tests/doctor.rs`, `crates/yard-cli/tests/cli.rs` (insta snapshots), `app/yard-app/src/sites/tests.rs`

**Interfaces:**
- Consumes: Task 4's `State.defaults_written`, Task 2's `fitting`.
- Produces: doctor checks `runtime-default-written:<key>` (info), `runtime-default-not-exact:<key>` (warn, message names the exact version it resolves to now, hint `yard runtime default <key> <exact>`), `composer-pin:<site>` (warn, for a committed `.yard` that needs a pin: hint `add [runtimes] composer = "<fitting>" to <file>`).

- [ ] **Step 1: Write the failing tests**
  - doctor: `written_defaults_show_as_info`, `a_prefix_default_warns_with_its_exact_version` (`default_node = "22"` with 22.23.5 installed → warn naming `22.23.5`), `a_committed_old_project_without_a_pin_is_named` (Review Focus 5: the `.yard` file's bytes are unchanged afterwards).
  - shim: `composer_in_an_unpinned_old_project_names_the_pin` — committed `.yard` on 7.1, default `2.11.2`: exits non-zero with `Composer 2.11.2 needs PHP 7.2.5 or later` and the `.yard` line as hint.
  - CLI snapshots: `yard sites` marks a defaulted PHP with `*` and prints `* the default in yard.toml`; `yard info` shows `php  8.4  default (yard.toml)` and `composer  2.11.2  default (yard.toml)`.
  - app: the site detail's PHP row reads `8.4 · default` for a defaulted site.
- [ ] **Step 2: Run** the new tests — Expected: FAIL.
- [ ] **Step 3: Implement.** Settle (`init.rs`) prints, for a committed `.yard` site that doctor would name, `add [runtimes] composer = "<v>" to <file>` and still serves.
- [ ] **Step 4: Run** `cargo test --workspace --locked`, then in `app/`: `cargo test --workspace --locked` — Expected: all pass. Then `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo fmt --check` in both workspaces.
- [ ] **Step 5: Commit** — `"yard shows where each version comes from"`.

### Task 7: docs and the real-machine script

**Files:**
- Modify: `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md` (§7's default line, §9.2, §21's `in_use` note; point at the new spec), `docs/superpowers/specs/2026-10-19-yard-design.md` §4.5, `docs/superpowers/specs/2026-10-25-yard-m4c-node-design.md` (its default), `README.md`, `.hydra/rules/config-ownership.md` (yardd's fill-in writes to `yard.toml`, managed `.yard` and `sites.toml`; `run/defaults.toml`), `.hydra/rules/rpc-protocol.md` (2.15), `AGENTS.md` Status and build section, `scripts/e2e-composer.sh`
- Test: `shellcheck scripts/e2e-composer.sh`

- [ ] **Step 1: Update `e2e-composer.sh`:** after the 8.4 project's install, assert `yard.toml` has `[runtimes.composer] default` equal to the installed 2.x; after the 7.1 project's link, assert its pin is `2.2.32` (exact) and no default was written by it; install a newer exact 2.x if the index lists one (else skip with a note) and assert the 8.4 project still runs the default; back up and restore `yard.toml` as today. Add `run/defaults.toml` to nothing it restores (yardd rewrites it).
- [ ] **Step 2: Run** `shellcheck scripts/e2e-composer.sh` — Expected: clean. (The script itself runs only with the maintainer's approval.)
- [ ] **Step 3: Update the docs listed above;** grep for `DEFAULT_SPEC`, `PhpVersion::DEFAULT`, `[[default]]`, `exclude`, `2.2 below PHP 7.2.5`, `else 22`, `else 8.4` across `docs/`, `README.md`, `AGENTS.md`, `.hydra/` and `--help` text; each hit is updated or deliberately historical.
- [ ] **Step 4: Run** `cargo test --workspace --locked 2>&1 | tail -5` — Expected: all pass (help-text snapshots included).
- [ ] **Step 5: Commit** — `"explicit defaults documented, and e2e-composer.sh checks them"`.
