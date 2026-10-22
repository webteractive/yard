# Yard M4a (the `.yard` project file) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A project that commits a `.yard` naming its PHP version, docroot, HTTPS and extensions is served that way, and `yard init` writes that file, links the project, and installs what is missing after a yes.

**Architecture:** `yard-core` gains a pure `project` module (parse, detect, render, needs). yardd reads each linked project's `.yard` (bounded, regular files only) before it resolves sites. A site whose file asks for something not installed, or does not parse, carries `needs` or `project_error` in its `SiteInfo`, and nginx serves it a 503 setup page instead of PHP. The watcher also watches each linked project directory for `.yard`. `sites.init` writes the file and moves the site's settings out of `sites.toml`. The CLI asks before running the existing `php.install` and `ext.add` jobs.

**Tech Stack:** Rust 2024; `toml` and `toml_edit` (already in `yard-core`); `insta`; no new crates.

**Spec:** `docs/superpowers/specs/2026-10-22-yard-project-file-design.md` (all of it, except `node` and `[services]`), with `docs/superpowers/specs/2026-10-19-yard-design.md` §4.3, §4.5, §5.1, §6.1–6.3.

## Global Constraints

- The file is `.yard`, TOML, at the directory `yard link` was run in. For serving only that one is read; the shims use the nearest one in the cwd or its parents.
- Keys in M4a: `php` (`"7.0"`–`"8.5"`), `docroot` (relative to the project), `secure` (bool), `extensions` (list of names). Every other key, `node` and `[services]` included, is `PROJECT_FILE_INVALID` naming the file, line and key (`deny_unknown_fields`).
- With a `.yard`, each setting comes from the file. A key it leaves out takes the machine default (default PHP, `secure_by_default`, the detected docroot), never `sites.toml`.
- Yard writes `.yard` only in `yard init`, and never overwrites one without `--force`. It never reads `.env`.
- Nothing is installed without a yes (`--yes`, or `y` at the prompt). Reconcile never installs.
- Extensions are per PHP version. Yard never disables or removes one because a `.yard` stopped listing it.
- Error codes (new, never renamed): `SITE_HAS_PROJECT_FILE`, `NEEDS_INSTALL`, `PROJECT_FILE_INVALID`.
- Protocol: additive, so `PROTOCOL_VERSION` moves `2.3` → `2.4`.
- `yard-core` stays pure: reads, spawns and the clock are injected (`.hydra/rules/rust-workspace.md`).
- Tests never bind 53, 80, 443, 8080, 8443 or 53535, never touch Herd, and never run real brew.
- Docs move with each task: both specs, README, `--help`, AGENTS.md, the hydra rules.
- Commits: lowercase prose subject, no type prefix, no trailers. Commit only after the maintainer has approved this plan (decision 10).

## Decisions (rulings made while planning; the maintainer reviews them)

1. **M4 is split.** M4a is `.yard` with `php`, `docroot`, `secure` and `extensions`, which the `.yard` spec §8 allows. M4b (services, `yard env`) and M4c (Node) get their own plans and add `[services]` and `node` to the file. Cost if wrong: one parser change in M4b.
2. **`yard init` writes no commented `[services]` block until M4b.** Uncommenting it in M4a would only produce `PROJECT_FILE_INVALID`.
3. **`secure` is written only when it is true**, matching the spec's eepower example. An absent key leaves HTTPS to each teammate's `secure_by_default`.
4. **`docroot` must be relative and stay inside the project** (no absolute path, no `..`). A cloned repo must not be able to serve `/` or a sibling directory. `yard init` writes `"."` when the project root is the docroot.
5. **Extensions in `.yard` are names, never `.so` paths** (a path is machine-specific). They take `yard ext add`'s name forms, with pinned releases (`xdebug-2.9.8`) included.
6. **An extension counts as present** when `yard.toml` manages it for that version (enabled or not), when brew's `conf.d` loads it, or when the version's PHP has it compiled in. The compiled-in list comes from `<keg>/bin/php -n -m`, cached in `state/php-modules.json` by the canonical keg path, so a `brew upgrade` (a new Cellar path) runs it again.
7. **When the PHP version is missing, `needs` lists the PHP version plus every listed extension `yard.toml` does not manage.** Compiled-in extensions cannot be known yet. The CLI installs PHP first, fetches the site again, and adds only the extensions still needed.
8. **The setup page is a generated file `nginx/setup/<site>.html`**, served with `error_page 503`, not nginx `return` text. `return` text would interpolate a `$` in a path. Its header names the site's nginx override, like `nginx/sites/<site>.conf`.
9. **Commands refused with `SITE_HAS_PROJECT_FILE`:** `isolate`, `unisolate`, `secure`, `unsecure`, `docroot`, whether or not the file sets that key. `yard link --yes` exists too, so `link` can install without a prompt the way `init` does.
10. **Plan approval.** the maintainer approving this plan approves executing it, with the commit at the end of each task. The e2e run on the real data root (Task 11) still waits for his go-ahead at that point.
11. **`yard init` with an existing `.yard` that does not parse** fails with `PROJECT_FILE_INVALID` before it links or moves anything.
12. **`yard sites` gains a `STATUS` column**: `serving`, or `needs setup`.

## Review Focus

1. **A cloned `.yard` that points `docroot` outside the project** (`"/"`, `"../other"`, `"/etc"`) is refused with `PROJECT_FILE_INVALID`, and the site serves the setup page. Pinned by Task 2's `docroot_must_stay_inside_the_project`.
2. **A `.yard` that is a FIFO, a directory or a huge file** never blocks or bloats yardd. It reads as invalid, and every other site keeps reconciling. Pinned by Task 5's `fifo_project_file_does_not_block_reconcile`.
3. **Deleting `.yard`** returns the site to `sites.toml` resolution: after `init` moved the settings out, that means the defaults. Pinned by Task 6's `deleting_the_project_file_reverts_the_site`.
4. **An unmet site that is also `secure = true`** gets its certificate and the setup page on https. No pool starts for the missing version, and other sites keep serving. Pinned by Task 5's `unmet_site_serves_setup_page_and_others_keep_serving`.
5. **`yard init` run twice** writes nothing the second time and leaves `sites.toml` as it is. Pinned by Task 8's `init_twice_is_a_no_op`.

---

### Task 1: Protocol types, method and error codes

**Files:**
- Modify: `crates/yard-proto/src/types.rs`, `methods.rs`, `error.rs`, `lib.rs`
- Modify: every `SiteInfo { .. }` literal (`yard-core/src/sites.rs`, `yard-core/src/render/mod.rs` tests, `yard-cli/tests/cli.rs`, `yard-proto/src/types.rs` tests)
- Docs: main spec §6.2 (method list gains `sites.init`), §6.3 (the three codes)

**Interfaces:**
- Produces: `types::Need` (`#[serde(tag = "kind", rename_all = "snake_case")]`): `Php { php: String }`, `Extension { name: String, php: String }`. `name` is exactly as written in `.yard`, e.g. `xdebug-2.9.8`.
- `Need::describe(&self) -> String`: `"PHP 7.3"` / `"the redis extension for PHP 7.3"`. `Need::command(&self) -> String`: `"yard php install 7.3"` / `"yard ext add redis --php 7.3"`.
- `SiteInfo` gains `project_file: Option<String>`, `needs: Vec<Need>` and `project_error: Option<YardError>`, each `#[serde(default, skip_serializing_if = …)]`. Also `SiteInfo::needs_setup(&self) -> bool`, which is `project_error.is_some() || !needs.is_empty()`.
- `YardError` derives `Eq` too (`serde_json::Value` is `Eq`), so `SiteInfo` keeps `Eq`.
- `methods::SITES_INIT = "sites.init"`. `methods::InitParams { path: String, name: Option<String>, force: bool }`, with `#[serde(default)]` on `name` and `force`.
- `types::InitResult { file: String, text: String, written: bool, site: SiteInfo }`.
- `ErrorCode::{SiteHasProjectFile, NeedsInstall, ProjectFileInvalid}`. `PROTOCOL_VERSION = "2.4"`.

- [ ] **Step 1: Write the failing tests** in `yard-proto`: `site_info_without_project_fields_still_parses` (a 2.3 `SiteInfo` gives `project_file: None`, empty `needs`, no error); `need_is_tagged` (`Need::Extension` → `{"kind":"extension","name":"redis","php":"7.3"}`); `need_describe_and_command` for both variants; `init_params_default_force_false`; extend the method-name test with `sites.init`, and the error-code test with the three codes.
- [ ] **Step 2: Run** `cargo test -p yard-proto` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; add the new fields to every literal; bump the protocol and the CLI test that expects `"2.3"`.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs** §6.2 and §6.3.
- [ ] **Step 6: Commit** `project file types: SiteInfo needs, sites.init and three error codes, protocol 2.4`

### Task 2: Parsing `.yard` (yard-core)

**Files:**
- Create: `crates/yard-core/src/project.rs` (registered in `lib.rs`)
- Modify: `crates/yard-core/src/config/yard.rs` (move the name-form parsing out of `yard-daemon/src/rpc/ext.rs`), `crates/yard-core/src/config/mod.rs` (`config_error` takes the code), `crates/yard-daemon/src/rpc/ext.rs` (use it)

**Interfaces:**
- Produces: `project::FILE = ".yard"`.
- `project::ProjectFile { php: Option<PhpVersion>, docroot: Option<PathBuf>, secure: Option<bool>, extensions: Vec<ExtName> }` (`Debug, Clone, PartialEq, Eq, Default`).
- `config::yard::ExtName { written: String, module: String, release: Option<String> }`.
- `config::yard::parse_ext_name(name: &str) -> Result<ExtName, YardError>`: `xdebug-2.9.8` → module `xdebug`, release `Some("xdebug-2.9.8")`; `redis` → module `redis`, release `None`; an invalid module is `INVALID_ARGUMENT`. `rpc/ext.rs` `add` calls it for the non-`.so` case and keeps its `.so` branch.
- `project::parse(text: &str, file: &Path) -> Result<ProjectFile, YardError>`: errors are `PROJECT_FILE_INVALID` with `data { file, line, column }` and hint `edit <file>`.
- `config::config_error(code: ErrorCode, file, text, span, message)`: existing callers pass `ConfigInvalid`.
- `project::Projects = BTreeMap<String, Result<ProjectFile, YardError>>`, keyed by site name.
- `project::load(sites: &SitesConfig, read: &dyn Fn(&Path) -> Option<String>) -> Projects`: an entry for each linked site whose `<path>/.yard` reads; no entry when it does not exist.

- [ ] **Step 1: Write the failing tests** in `project.rs`:
  - `parses_every_key`: the spec §4 example without `node`/`[services]` gives `php 7.3`, `docroot public`, `secure true`, and modules `redis`, `imagick`.
  - `empty_file_is_all_defaults`.
  - `unknown_key_names_file_and_line`: `node = "20"` on line 2 gives `PROJECT_FILE_INVALID`, `data.line == 2`, and a message containing `node` and the file path.
  - `unsupported_php_is_invalid` (`"5.6"`).
  - `docroot_must_stay_inside_the_project`: `"/"`, `"/etc"`, `"../other"` and `"public/../../x"` are each invalid; `"."`, `"public"` and `"web/dist"` parse.
  - `extensions_are_names_not_paths`: `"/x/redis.so"` and `"redis.so"` are invalid; `"xdebug-2.9.8"` parses with module `xdebug`.
  - `load_reads_only_linked_sites`: a proxy entry and a linked entry with no file give no entry, and a linked entry with a file gives `Ok`.
  - In `config/yard.rs`: `parse_ext_name_forms`.
- [ ] **Step 2: Run** `cargo test -p yard-core project` — Expected: FAIL to compile.
- [ ] **Step 3: Implement** with a `#[serde(deny_unknown_fields)]` raw struct and `toml::Spanned` on `docroot` and the extension names, so their errors point at the line.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS (the ext tests prove `add` behaves as before).
- [ ] **Step 5: Commit** `parse the .yard project file`

### Task 3: Resolution and needs (yard-core)

**Files:**
- Modify: `crates/yard-core/src/sites.rs`, `crates/yard-core/src/project.rs`, `crates/yard-core/src/constraint.rs`, `crates/yard-daemon/src/doctor.rs` (`php_constraint` uses `best_version`)
- Modify: callers of `resolve` (`yard-daemon/src/reconcile.rs`, `doctor.rs`, tests). Pass `&Projects::new()` for now; Task 5 wires the real map.

**Interfaces:**
- Consumes: Task 2's `Projects`, `ProjectFile`.
- Produces: `sites::resolve(cfg, sites, projects: &Projects, tld, http_port, https_port, is_dir)`. For a linked site with `Ok(file)`, use the file's php, docroot (`path.join`), secure, and the defaults; ignore the entry's `php`/`docroot`/`secure`; set `project_file`. For `Err(e)`, use the defaults with `project_error = Some(e)`.
- `constraint::best_version(wanted: &str, installed: &[PhpVersion]) -> Option<(PhpVersion, bool)>`: the highest installed version that satisfies `wanted`, else the lowest supported one (`bool` = installed).
- `project::needs(file: &ProjectFile, v: PhpVersion, installed: &[PhpVersion], has_ext: &dyn Fn(&str) -> bool) -> Vec<Need>`. When `v` is installed: an `Extension` for each listed module `has_ext` rejects. When it is not: `Php` first, then an `Extension` for every listed extension (decision 7; the caller's `has_ext` then answers only for `yard.toml`).

- [ ] **Step 1: Write the failing tests**:
  - `project_file_settings_win_over_sites_toml`: entry `php = 7.1`, `secure = true`, `docroot = "web"`; file `php = "7.3"` → the site runs 7.3, docroot `<path>/public` (detected), not secure (`secure_by_default` false), and `project_file` is `<path>/.yard`.
  - `invalid_project_file_marks_the_site`: `needs_setup()` is true, and the site still resolves with the default PHP.
  - `best_version_prefers_installed`: `^7.1` with 7.1, 7.4 installed → `(7.4, true)`; `^7.2` with none installed → `(7.2, false)`; `>=9` → `None`.
  - `needs_table`: missing PHP → `[Php 7.3, Extension redis 7.3]`; installed PHP with `has_ext("redis")` true → `[]`; installed PHP with `has_ext` false → `[Extension redis]`.
- [ ] **Step 2: Run** `cargo test -p yard-core` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; update the callers; the doctor hint logic now calls `best_version`.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS (doctor's `php-constraint` tests are unchanged).
- [ ] **Step 5: Commit** `resolve sites from their .yard, and work out what a file needs`

### Task 4: The setup page (yard-core render)

**Files:**
- Modify: `crates/yard-core/src/render/nginx.rs`, `crates/yard-core/src/render/mod.rs` (`overrides_for`), `crates/yard-core/src/paths.rs`

**Interfaces:**
- Produces: `Paths::nginx_setup_dir()` (`nginx/setup`), `Paths::nginx_setup_page(name)` (`nginx/setup/<name>.html`), `Paths::php_modules_cache()` (`state/php-modules.json`).
- `nginx::setup_page(ctx: &RenderCtx, site: &SiteInfo) -> String`: HTML-escaped, headed `<!-- Generated by Yard — edit <nginx override> instead -->`. The body is a `<pre>` with exactly this text for needs:
  ```
  eepower needs PHP 7.3.
  Run yard init in /Users/me/Yard/eepower to install it.
  ```
  Several needs join with `, ` and ` and `, and the second line ends `to install them.` For a `project_error` the text is `<name>'s .yard does not parse: <message>` and then `Edit <file> to fix it.`
- `nginx_site` for a site where `needs_setup()` is true replaces the PHP body with `root <setup dir>`, `error_page 503 /<name>.html`, a `location /` that does `return 503`, and `location = /<name>.html { internal; }`. The http/https and redirect structure stays as it is.
- `overrides_for(nginx/setup/<name>.html)` is `nginx_override(name)`.

- [ ] **Step 1: Write the failing tests**: snapshots `nginx_site_setup` and `nginx_site_setup_secure`; `setup_page_text_for_needs` (asserts the two lines above, and `them` for two needs); `setup_page_for_parse_error`; `setup_page_escapes_html` (a path containing `<b>&`); extend the `overrides_for` header test with a setup page.
- [ ] **Step 2: Run** `cargo test -p yard-core render` — Expected: FAIL.
- [ ] **Step 3: Implement**; review each new snapshot against the spec before you accept it.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `render a setup page for a site whose .yard is not met`

### Task 5: Reconcile serves `.yard` sites (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/project.rs` (reads, needs), `crates/yard-daemon/src/modules.rs` (the `php -n -m` cache)
- Modify: `crates/yard-daemon/src/reconcile.rs`, `state.rs` (`Snapshot.projects: Projects`), `doctor.rs` (`read_composer_json` → `project::read_small`, and `View::from_files` calls `read_projects`, passes them to `resolve` and fills needs), `src/bin/yard-fake-unit.rs` (`php -n -m`)
- Test: `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Consumes: Tasks 2–4.
- Produces: `project::read_small(file: &Path, max: u64) -> Option<String>`: regular files only, at most `max` bytes. `.yard` uses 64 KiB and `composer.json` keeps 1 MiB.
- `project::read_projects(sites: &SitesConfig) -> Projects`: `project::load` over `read_small`. A `.yard` that exists but is not a readable regular file of sane size is `Err(PROJECT_FILE_INVALID)` ("is not a regular file" / "is larger than 64 KiB").
- `project::fill_needs(sites: &mut [SiteInfo], projects: &Projects, yard: &YardConfig, brew: &Brew, paths: &Paths)`, with `has_ext` per decision 6.
- `modules::compiled(paths: &Paths, opt: &Path) -> BTreeSet<String>`: reads the cache keyed by `canonicalize(opt)`. On a miss it runs `<opt>/bin/php -n -m` (with the `fds::cloexec_inherited` hook, as `holder.rs` runs lsof), parses it with `ext::loaded_modules`, and saves the result by temp file then rename. A failed run returns empty and caches nothing.
- Reconcile: `read_projects` → `resolve` → `fill_needs`. Pools come only from sites without `needs_setup()`. Each site that needs setup renders its setup page. `nginx/setup` joins the `managed` dirs, so a page is removed once its site is met. `Snapshot.projects` is kept. `docroots_stale` treats a site whose project file sets `docroot` as pinned.
- The fake `php`: with `-n -m` it prints `[PHP Modules]` followed by `Core`, `date`, `json`, `mbstring` and `pcre`.

- [ ] **Step 1: Write the failing tests** in `tests/project.rs` (each starts a `support::daemon::prepared` daemon):
  - `project_file_php_is_served`: `add_php("7.3")`; `.yard` `php = "7.3"` → `sites.list` shows 7.3; `php-fpm-7.3` runs.
  - `unmet_site_serves_setup_page_and_others_keep_serving`: `.yard` `php = "7.3"`, `secure = true`, not installed. Its `needs` is `[Php 7.3]`; `nginx/setup/<name>.html` exists; its cert is issued; no `php-fpm-7.3` unit runs; a second plain site still resolves and its pool runs.
  - `compiled_in_extension_is_not_a_need`: `extensions = ["mbstring", "redis"]` on the default PHP → `needs == [Extension redis]`.
  - `module_cache_is_read_not_rerun`: after a first reconcile, `state/php-modules.json` has the keg. Rewrite its list to include `sentinel` and add `"sentinel"` to `.yard` → after a reconcile, no need for `sentinel`.
  - `managed_extension_counts_even_when_disabled`: `yard.toml` has `redis` with `enabled = false` → no need.
  - `fifo_project_file_does_not_block_reconcile`: `mkfifo <project>/.yard`, then `sites.list` answers within 5 s, the site has `project_error`, and another site is served.
  - `setup_page_goes_once_met`: after `add_php("7.3")` and a reconcile, the setup page is gone and the pool runs.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test project` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `serve linked projects from their .yard, with a setup page when it is not met`

### Task 6: Watching `.yard` (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/watch.rs`
- Test: `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Consumes: `load_sites`, `project::FILE`.
- Produces: the watcher keeps a set of linked project directories (from `load_sites`, read at spawn and again after each `sites.toml` event), watched `NonRecursive`. `change_watches(debouncer, add, remove)` adds and removes in one batch. An event whose file name is `.yard` and whose parent is in the set reconciles.

- [ ] **Step 1: Write the failing tests**: `editing_the_project_file_reconciles` (change `php` from the default to an installed `7.3`; `sites.list` shows 7.3 within 2 s); `deleting_the_project_file_reverts_the_site` (delete `.yard`; within 2 s the site runs the default PHP with `project_file: None`); `a_newly_linked_project_is_watched` (link over RPC, then write `.yard`; within 2 s the site follows it).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test project` — Expected: the three FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the suite three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `watch each linked project's .yard`

### Task 7: Refusals and doctor (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/rpc/handlers.rs` (`docroot`, `secure`), `crates/yard-daemon/src/rpc/php.rs` (`isolate`), `crates/yard-daemon/src/doctor.rs`
- Test: `crates/yard-daemon/tests/rpc.rs`, `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Produces: `handlers::refuse_project_file(ctx, site: &str, change: &str) -> Result<(), YardError>`, called first in each refused handler. It fails when the `sites.toml` entry has a `path` whose `.yard` exists, with `SITE_HAS_PROJECT_FILE`, message `<site> is configured by <path>/.yard`, and hint `edit <path>/.yard: <change>`. The `change` values are `php = "7.1"`, `remove php`, `secure = true`, `secure = false` and `docroot = "web"`.
- Doctor checks, for each site with `project_file`:
  - `project-file:<site>`: `fail` with the error's message and hint for a `project_error`.
  - `project-file:<site>`: `fail` for needs, message `<site> needs <describe…>` and hint `cd <path> && yard init`.
  - `project-file:<site>`: `warn` when a listed extension is in `yard.toml` with `enabled = false`, with hint `yard ext enable <module> --php <v>`.
  - `project-file:<site>`: `ok` otherwise.
  - `yard-php-ignored:<site>`: `warn` when `<path>/.yard-php` exists too, with hint `delete <path>/.yard-php`.

- [ ] **Step 1: Write the failing tests**: `commands_are_refused_with_a_project_file` (each of the five methods → `SITE_HAS_PROJECT_FILE`, and `sites.toml` is unchanged); `doctor_reports_project_files` (one site met, one with a need, one invalid, one with both `.yard` and `.yard-php`, one with a disabled extension → the checks above).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test rpc --test doctor` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `refuse settings a .yard owns, and check project files in doctor`

### Task 8: `sites.init` (yard-core and yard-daemon)

**Files:**
- Modify: `crates/yard-core/src/project.rs`
- Create: `crates/yard-daemon/src/rpc/project.rs`, registered in `rpc/mod.rs` and dispatched from `handlers::handle`
- Test: `crates/yard-daemon/tests/project.rs`

**Interfaces:**
- Consumes: Tasks 2, 3 and 5.
- Produces, in core:
  - `project::Detected { php: Option<PhpVersion>, docroot: PathBuf, secure: bool }`.
  - `project::detect(entry: Option<&SiteEntry>, project: &Path, composer_json: Option<&str>, installed: &[PhpVersion], secure_by_default: bool, is_dir: &dyn Fn(&Path) -> bool) -> Result<Detected, YardError>`:
    - `php`: the entry's `php`, else `best_version` of `require.php`, else `None`.
    - `docroot`: the entry's relative docroot as it is; an absolute one inside the project made relative; one outside the project is `INVALID_ARGUMENT` with hint `yard docroot <path inside the project>`; else `detect_docroot` made relative, with `"."` for the root.
    - `secure`: the entry's value, else `secure_by_default`.
  - `project::render(d: &Detected) -> String`: `# .yard: how Yard serves this project. Commit it.`, then `php` when set, `docroot`, and `secure = true` only when true.
  - `project::rewrite(existing: &str, d: &Detected) -> Result<String, YardError>`: through `toml_edit`, it sets or removes `php`, `docroot` and `secure`, and keeps every other key and comment.
- Produces, in the daemon: `rpc::project::init(ctx, InitParams) -> Result<InitResult, YardError>`. In order:
  1. `directory(path)`.
  2. The linked name: the `sites.toml` entry whose `path` is the directory. When `name` is given and differs, `INVALID_ARGUMENT` with hint `yard unlink <linked>`. When there is no link: `name`, else `host_label` of the directory name.
  3. An existing `.yard` without `force` is read and parsed; an invalid one returns its `PROJECT_FILE_INVALID` (decision 11). Otherwise detect, then `render` (or `rewrite` with `force`), and write it.
  4. One `TomlFile::edit` of `sites.toml`: remove `php`, `secure` and `docroot` from the entry, and set `path` when it is not linked.
  5. Reconcile, then return the file, its text, whether it was written, and the site.

- [ ] **Step 1: Write the failing tests**:
  - Core: `render_for_eepower` (php 7.3, docroot public, not secure → exactly the spec §6 text without the services lines); `detect_table` (isolation wins over composer; composer `^7.3` with 7.3 not installed → 7.3; root docroot → `"."`; an absolute docroot outside the project is an error); `rewrite_keeps_extensions_and_comments`.
  - Daemon: `init_writes_links_and_moves_settings` (an isolated, secured site with a docroot override → the file has all three, and the `sites.toml` entry keeps only `path`, still serving the same); `init_twice_is_a_no_op`; `init_keeps_an_existing_file_without_force`; `init_force_keeps_extensions`; `init_refuses_an_invalid_file`; `init_returns_needs` (`php = "7.3"` not installed).
- [ ] **Step 2: Run** `cargo test -p yard-core project && cargo test -p yard-daemon --test project` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `sites.init writes .yard and moves a site's settings into it`

### Task 9: Shims read `.yard` (yard-core and yard-shim)

**Files:**
- Modify: `crates/yard-core/src/shim.rs`, `crates/yard-shim/src/main.rs`
- Test: `crates/yard-core/src/shim.rs` tests, `crates/yard-shim/tests/shim.rs`

**Interfaces:**
- Produces: `shim::Source::Project(PathBuf)` (displays the file). `resolve_version` order: the nearest `.yard` in the cwd or its parents (its `php`, else the default), then the containing site's isolation, then the nearest `.yard-php`, then the default. A `.yard` that does not parse is its `PROJECT_FILE_INVALID`.
- `main.rs`: when the source is `Project` and `php_opt` fails, the error is `FORMULA_MISSING`, `PHP <v> is not installed (<file>)`, hint `yard init`.

- [ ] **Step 1: Write the failing tests**: `project_file_beats_isolation_and_yard_php` (a `.yard` at `/code/old` with `php = "7.3"` beats the site's 7.1 and a `/code/.yard-php`); `project_file_without_php_is_the_default`; `nearest_project_file_wins` (`/code/a/.yard` 7.3 and `/code/a/sub/.yard` 7.4, cwd `/code/a/sub/x` → 7.4); `bad_project_file_names_it`; in `yard-shim/tests`, `missing_project_php_hints_init`.
- [ ] **Step 2: Run** `cargo test -p yard-core shim && cargo test -p yard-shim` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `the php shims follow the nearest .yard`

### Task 10: `yard init`, `yard link --yes` and `yard sites` (yard-cli)

**Files:**
- Create: `crates/yard-cli/src/commands/init.rs`
- Modify: `crates/yard-cli/src/cli.rs`, `commands/mod.rs`, `commands/sites.rs`
- Test: `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `SITES_INIT`, `InitResult`, `Need`, `run_job`.
- Produces: `Command::Init { name: Option<String>, #[arg(long)] force: bool, #[arg(long)] yes: bool }` with the `--help` text "Write .yard for the current project, link it, and install what it needs after asking". `Command::Link` gains `--yes`.
- `init::settle(ctx: &Ctx, site: SiteInfo, yes: bool) -> Outcome`, shared by `init` and `link`:
  - No needs: print `Serving <url>`.
  - Otherwise print each need as `  <describe>  (<command>)`. Then:
    - `--yes`: go ahead.
    - A terminal on stdin and stdout without `--json`: ask `Install these? [y/N] `; anything but `y`/`yes` exits 1 with `NEEDS_INSTALL`.
    - Neither: `NEEDS_INSTALL`, message `<site> needs <describe…>`, hint `yard init --yes`, `data.needs`.
  - To install: run each `Php` need with `PHP_INSTALL`, fetch the site again from `sites.list`, then run each `Extension` need still listed with `EXT_ADD { name, php }`, and print `Serving <url>`.
- `init` prints `Wrote <file>:` (or `<file> is already there:`), the text indented two spaces, then calls `settle`. With `--json` it prints the `InitResult` after the jobs.
- `yard sites` gains `STATUS` (`serving` / `needs setup`) as its last column.

- [ ] **Step 1: Write the failing tests** against `MockDaemon`:
  - `init_without_needs_prints_file_and_url`.
  - `init_without_terminal_or_yes_is_needs_install` (exit 1, stderr has the need and `hint: yard init --yes`).
  - `init_yes_installs_php_then_remaining_extensions`: sequenced `sites.list` answers: after PHP the site needs `redis`, not `mbstring`. Assert the `php.install` params, then one `ext.add` with `name: redis`.
  - `link_yes_settles_needs`.
  - `init_json_needs_install_error`.
  - `sites_shows_needs_setup`.
  - `--help` lists `init`.
- [ ] **Step 2: Run** `cargo test -p yard-cli` — Expected: FAIL.
- [ ] **Step 3: Implement.** Use `std::io::IsTerminal` for the terminal check.
- [ ] **Step 4: Run** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` — Expected: all PASS.
- [ ] **Step 5: Commit** `yard init, and yard link asks before installing what a .yard needs`

### Task 11: Docs and the real-machine check

**Files:**
- Create: `scripts/e2e-m4a.sh`
- Modify:
  - Main spec: §3.3 (`nginx/setup/`, `state/php-modules.json`); §4.5 (the `.yard` order for sites and shims); §5.1 ("nothing is written into the project" gains the `yard init` exception); §10 (e2e-m4a); §12 (M4 split per decision 1).
  - `.yard` spec: §6 (no `[services]` comment until M4b), §8 (M4a landed), and the status line.
  - `README.md`: the command table (`init`, `link --yes`; `isolate`, `secure`, `docroot` refused with a `.yard`; the `install-shims` order).
  - `AGENTS.md`: status and key decisions (sites are explicit, and `yard init` is the one write into a project).
  - `.hydra/rules/cli-output.md`: the confirm and `--yes` convention, and `NEEDS_INSTALL` without a terminal.
  - `.hydra/rules/config-ownership.md`: `.yard` is the project's file; Yard writes it only in `yard init`, through `project::render` / `rewrite`.

**Interfaces:**
- `scripts/e2e-m4a.sh`, after `e2e-m1.sh`, against the real data root, in the style of `e2e-m3.sh`:
  1. Create a scratch project with `public/index.php` printing `PHP_VERSION`, and put `php = "7.1"` in its `.yard`.
  2. `yard link` → curl shows 7.1.
  3. Edit `php = "8.4"` → within 3 s curl shows 8.4.
  4. Set `php = "7.3"` (not installed) → curl answers 503 and the body contains `yard init`.
  5. `yard isolate 8.4` → exit 1, `SITE_HAS_PROJECT_FILE`.
  6. `yard unlink`, then remove the scratch dir and its override.
  - It installs nothing, and backs up and restores `sites.toml` with a `trap`.

- [ ] **Step 1: Docs** as listed; `hydra sync && hydra doctor` — Expected: PASS.
- [ ] **Step 2: Grep the docs** for `.yard-php`, `isolate`, `sites.list`, `link` and `PROTOCOL` and fix what no longer matches.
- [ ] **Step 3: Write** `scripts/e2e-m4a.sh`.
- [ ] **Step 4: Ask the maintainer**, then `cargo build --release`, `yard stop && yard start`, and `scripts/e2e-m1.sh && scripts/e2e-m4a.sh` — Expected: both pass, and `yard --version` shows the new commit.
- [ ] **Step 5: Commit** `M4a docs and the e2e check for .yard`
