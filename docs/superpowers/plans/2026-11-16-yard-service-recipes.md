# Yard service recipes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Approved by the maintainer on 2026-11-16**, to run natively (superpowers:executing-plans) on main, one commit per task.

**Goal:** Service engines become a `ServiceEngine` contract with MySQL, PostgreSQL and MinIO in Rust
and a recipe engine read from TOML at runtime (`~/.yard/engines/*.toml`, sources `brew`, `path`,
`download`, `npm`), Redis moves to a carried recipe, and Valkey, Memcached, Meilisearch and
Typesense ship as carried recipes.

**Architecture:** `yard-core::services::engines` holds the trait, the `Engines` set and every
implementation; recipe parsing, load checks and placeholder expansion are pure and live there too.
yardd reads `~/.yard/engines/` at the start of every reconcile (keeping each recipe's last good
version), and the snapshot carries `Arc<Engines>`, so every reader sees one consistent set. Installs
(`download`, `npm`) are jobs in a new `yard-daemon::programs` module writing
`~/.yard/programs/<key>/<version>/`. The closed `Engine` enum is deleted; nothing outside
`services/engines/` matches on an engine key.

**Tech Stack:** Rust 1.92 (root) and 1.98.1 (`app/`), serde/toml/toml_edit, sha2 (present),
`/usr/bin/curl`, `/usr/bin/tar`, Yard's fnm Node (M4c); GPUI for Yard.app. No new crates.

**Spec:** `docs/superpowers/specs/2026-11-16-yard-service-recipes-design.md` (approved 2026-11-16).
Read it first; §n below are the spec's.

## Global Constraints

- Protocol `2.13` (additive). New methods `engines.list` and `engines.install` (a job) join `methods::ALL`; new event `engines_changed` on topic `state`; `ServiceInfo.engine_name: String` (`#[serde(default)]`).
- New error codes: `ENGINE_UNKNOWN`, `ENGINE_RECIPE_INVALID`, `ENGINE_NOT_INSTALLED`, `DOWNLOAD_HASH_MISMATCH`, `PATH_NOT_ALLOWED`. Every hint is the command or file that fixes it.
- Recipe keys: `^[a-z][a-z0-9-]{0,31}$`, not `mysql`, `postgres`, `minio`, `emailulator`, not a Yard command name (`yard_core::services::engines::YARD_COMMANDS`, which includes `db` and `pgsql`), and not starting `mysql-` or `postgres-`. The file stem is the key.
- Using an engine is `yard <engine> <verb>` (spec §9.1); running one stays `yard service …`. `yard db <verb>` keeps working for this release with the stderr line `yard db is going away: use yard <engine> <verb>`.
- Recipe `schema = 1` is the only schema. `deny_unknown_fields` on every recipe table.
- Programs run by absolute path only, with no shell and no PATH lookup; a `path` program under `~/Herd`, `~/Library/Application Support/Herd`, `/Applications/Herd.app`, `/Applications/DBngin.app`, `~/Library/Application Support/com.tinyapp.DBngin` or `/Applications/Postgres.app` is `PATH_NOT_ALLOWED`.
- `download`: `https://` only (a `file://` URL only when `DaemonOptions.allow_file_urls`, tests), a 64-hex `sha256`, staged beside the install root and renamed into place; `npm`: exact `version`, Yard's fnm Node, PATH with that Node's `bin` first, `npm_config_python=/usr/bin/python3`.
- `services.toml` and `.yard` stay readable as they are today; the only new data is `version` (and no `formula`) on `download`/`npm` instances.
- Redis's rendered `redis.conf`, `overrides.conf` seed, data dir, unit and `yard env` lines stay byte-identical to today's.
- `yard-core` stays pure (no spawning, files or clock); recipes are parsed from text the daemon or CLI read. `yard-shim` never loads recipes.
- Tests never bind 3306, 5432, 6379, 7700, 8108, 9000, 9001, 11211, 80, 443, 8080, 8443, 53 or 53535 and never run real engines or the network; daemon tests use `yard-fake-unit`.
- Work on main. Commit messages: lowercase prose subject, no type prefix, prose body, no trailers. Ask the maintainer before each commit unless he has approved the run.

### Decided while planning (the spec is amended in Task 15)

- **The set type is `Engines`, not `Registry`:** `Snapshot.registry` is already the port registry.
- **Recipes load in reconcile, not in a separate watcher path.** The watcher only adds `~/.yard/engines` to its watched directories and reconciles on a change there; reconcile reads the directory, builds `Engines` and puts `Arc<Engines>` in the snapshot. One mechanism, no second shared state.
- **An unknown `.yard` engine is `Need::Manual`** (`what`: "the <key> service, which this Mac has no recipe for", `command`: "add ~/.yard/engines/<key>.toml"), not a new `Need` variant: it is a need Yard cannot install, which is exactly what `Manual` is (protocol 2.9), so older clients already show it.
- **Which keys take a line is fixed without the set:** only `mysql` and `postgres` are versioned (recipe engines are unversioned, spec §3), so `project::parse` and `config::services` validate lines with no `Engines` and the shim keeps parsing `.yard` with no recipes.
- **The keg-owner check is generic:** any `brew` source naming a tap formula (`a/b/c`) is checked the way MinIO's is (sources.rs), so MinIO needs no special case.

## Review Focus

1. **A recipe with a key that is also a built-in or a carried recipe's** (`mysql.toml`, `redis.toml` in `~/.yard/engines`) — the built-in is refused with `ENGINE_RECIPE_INVALID`; the carried one is replaced (Task 6, `user_recipe_replaces_carried`, `user_recipe_cannot_replace_builtin`).
2. **A path with spaces or quotes in a placeholder value** (`{data}` under a home with a space, a `path` program in `~/My Tools/`) stays one argument (Task 3, `placeholder_values_are_never_split`).
3. **A recipe edited to a broken state while its instance runs** keeps the instance on the last good recipe, and `yard status` names the file and line (Task 6, `broken_edit_keeps_last_good`).
4. **A download interrupted or cancelled mid-way** leaves no install root and no stage, and a retry starts clean (Task 7, `cancelled_download_leaves_nothing`).
5. **Two engines with the same usual port** (`redis` and `valkey` on 6379) — the second gets the next free port and its `yard env` names it (Task 12, `valkey_beside_redis_takes_next_port`).

---

### Task 1: Protocol 2.13 (yard-proto)

**Files:**
- Modify: `crates/yard-proto/src/lib.rs` (`PROTOCOL_VERSION = "2.13"`), `methods.rs`, `types.rs`, `error.rs`, `events.rs`
- Modify: `.hydra/rules/rpc-protocol.md` (a 2.13 bullet), `docs/superpowers/specs/2026-10-19-yard-design.md` §6.2

**Interfaces:**
- Produces: `methods::ENGINES_LIST = "engines.list"`, `methods::ENGINES_INSTALL = "engines.install"`; `EngineInfo { key, name, description, source: EngineSourceKind, origin: EngineOrigin, installed: bool, error: Option<YardError>, ports: Vec<EnginePort { name, usual }>, console: bool, databases: bool, connect: bool, version: Option<String> }`; `EngineSourceKind { Brew, Path, Download, Npm, #[serde(other)] Other }`; `EngineOrigin { BuiltIn, Carried, User { file: String }, Other }` (tagged, with the `Other` fallback the rule describes); `EnginesListResult { engines: Vec<EngineInfo>, errors: Vec<YardError> }` (errors: recipes that failed to load with no last good version); `EngineInstallParams { key: String }`; `ServiceInfo.engine_name: String`; `clients: Vec<String>` (`#[serde(default)]`) on `db.connect`'s result; `engine: Option<String>` (`#[serde(default)]`) on the `db.*` params, limiting the instance choice to that engine; `Event::EnginesChanged` (topic `State`); the five `ErrorCode`s.

- [ ] **Step 1: Write the failing tests** in `types.rs`/`methods.rs`/`error.rs` tests: `protocol_is_2_13`; `engine_info_round_trips` (all fields, `origin` as `{"kind":"user","file":"/x/a.toml"}`); `unknown_origin_and_source_read_as_other`; `service_info_without_engine_name_reads_empty`; `new_error_codes_are_screaming_snake` (the five, exact strings); `methods_all_lists_engines_methods`; `engines_changed_is_topic_state`.
- [ ] **Step 2: Run** `cargo test -p yard-proto` — FAIL (missing items).
- [ ] **Step 3: Add the types, codes, methods and event** with the names above; `engine_display` keeps its table (older yardd).
- [ ] **Step 4: Run** `cargo test -p yard-proto` — PASS; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** (`protocol 2.13 for service recipes`).

### Task 2: The contract and the built-ins (yard-core, additive)

**Files:**
- Create: `crates/yard-core/src/services/engines/{mod.rs,key.rs,mysql.rs,postgres.rs,minio.rs,redis_builtin.rs}`
- Modify: `crates/yard-core/src/services/mod.rs` (`pub mod engines;`)

**Interfaces:**
- Produces (in `services::engines`):
  - `EngineKey(String)`: `EngineKey::new(&str) -> Result<EngineKey, String>` (syntax only), `as_str()`, `Display`, `Ord`, serde as a string. `EngineKey::is_reserved(&self) -> bool` (built-in keys, `emailulator`, `mysql-`/`postgres-` prefixes).
  - `LineSpec { base_formula: &'static str, default_line: &'static str, shape: LineShape }`, `LineShape { MajorMinor, Major }`, `LineSpec::valid(&self, line) -> bool`, `LineSpec::plain_line(&self, version) -> Option<String>`; consts `MYSQL_LINES`, `POSTGRES_LINES`; `fn versioned_lines(key: &str) -> Option<&'static LineSpec>` (the registry-free lookup).
  - `PortSpec { suffix: String, usual: u16 }` (first is main; `""` names the entry `<instance>`).
  - `SourceSpec { Brew { formula: String }, BrewLines(&'static LineSpec), Path { program: PathBuf }, Download { version, url, sha256, strip: u32 }, Npm { package, version, node: String } }`.
  - `Origin { BuiltIn, Carried, User(PathBuf) }`, `DbFlavor { Mysql, Postgres }`, `HostHeader { Upstream, Client }`, `Console { port: String /* suffix */, host_header: HostHeader, nginx_overrides: bool }`, `Connect { url: String, shell: Option<Vec<OsString>>, clients: Vec<String>, socket: Option<PathBuf> }`.
  - `InstanceCtx<'a> { instance: &'a str, home: &'a Path, paths: &'a Paths, ports: &'a [PortRef], macos_user: &'a str, node: Option<&'a Path> }` (`home` is the keg for brew, the install root otherwise, the program's directory for `path`).
  - `trait ServiceEngine: Send + Sync + Debug` with `key, display, description, origin, source, lines (default None), ports, account(macos_user), dirs (default empty), command(ctx) -> UnitCommand, init(ctx) (default None), config_file(ctx) (default None), user_overrides() -> Option<String> (default None), start_timeout() (default None), may_sleep() (default true), env_lines(ctx), console() (default None), databases() (default None), connect(ctx) (default None)`.
  - `fn main_port(engine: &dyn ServiceEngine, instance: &str, ports: &[PortRef]) -> u16` (generalizes `render::main_port`: the entry named `<instance>`, else the first not named `<instance>-<suffix>` of a non-main port).
  - `struct Engines` with `Engines::builtin() -> Engines` (MySQL, PostgreSQL, MinIO, and for now a Rust Redis), `get(&self, key: &str) -> Option<&Arc<dyn ServiceEngine>>`, `all() -> impl Iterator`, `errors() -> &[RecipeError]` (empty until Task 5), `keys() -> Vec<String>`.

- [ ] **Step 1: Write the parity tests** in `engines/mod.rs`: for each of `mysql-8.0`, `mysql-8.4`, `mysql-9.7`, `postgres-17`, `redis`, `minio` with fixed ports, `parity_with_the_enum` asserts the trait's `command`, `init`, `config_file`, `user_overrides`, `env_lines`, `account("me")`, `ports`, `start_timeout` equal the existing `render::*`/`engine::*`/daemon `start_timeout` outputs (MySQL and PostgreSQL `Some(DATABASE_START_TIMEOUT)`; MinIO `console() == Some(Console { port: "console", host_header: Upstream, nginx_overrides: true })`; `databases()` `Some(Mysql)`/`Some(Postgres)`/`None`; `connect` for Redis has URL `redis://127.0.0.1:<port>` and shell `redis-cli -p <port>` from the keg). `engine_key_rules`: `a`, `meili-search` valid; `A`, `1a`, `a_b`, 33 chars invalid; `mysql-x` and `emailulator` reserved.
- [ ] **Step 2: Run** `cargo test -p yard-core services::engines` — FAIL.
- [ ] **Step 3: Implement** each built-in by moving the corresponding match arm's body (the old functions stay and call nothing new yet).
- [ ] **Step 4: Run** `cargo test -p yard-core` — PASS.
- [ ] **Step 5: Commit** (`a service engine contract beside the enum`).

### Task 3: Recipes: parse, check, expand (yard-core)

**Files:**
- Create: `crates/yard-core/src/services/engines/recipe.rs`
- Test fixtures: `crates/yard-core/src/services/engines/testdata/*.toml` (one good recipe per source, broken ones inline in tests)

**Interfaces:**
- Consumes: Task 2's types.
- Produces: `RecipeError { file: PathBuf, line: Option<usize>, message: String }` with `to_yard_error() -> YardError` (`ENGINE_RECIPE_INVALID`, message `<file>:<line>: <why>`, hint `yard engines check <file>`); `LoadOptions { allow_file_urls: bool }`; `pub fn parse_recipe(text: &str, file: &Path, origin: Origin, opts: &LoadOptions) -> Result<RecipeEngine, RecipeError>`; `RecipeEngine` implements `ServiceEngine` and exposes `version() -> Option<&str>` (download/npm pin) and `run_config_include() -> Option<&str>`.

The format is spec §6 exactly, with `run.config.include` (the include line template, e.g. `"include {overrides}"`) added; `[env] lines`, `[account]`, `[connect]` (`url`, `shell`, `clients`), `[console]` (`port`, `host_header` `"upstream"|"client"`, default client), `[health] tcp` (a port name; required), `[run]` (`command`, `env`, `stop_signal` `TERM|INT|QUIT`, `start_timeout` as a duration string like `sleep_after`'s, `dirs`, `init`, `sleep`).

Expansion: a template string is split into literal text and `{name}` placeholders once; each placeholder is replaced by its value as text, and an argument is never re-split (Review Focus 2). Values: spec §6.1. `{node}` only for `npm`; `{program}` only for `path`; `{home}` for all but `path`; `{include_overrides}` only inside `run.config.text`.

- [ ] **Step 1: Write the failing tests:** `each_source_parses` (the four testdata files); a table test `broken_recipes_report_file_and_line` with one case each for: TOML syntax, unknown field, `schema = 2`, stem ≠ key, reserved key, a Yard command name (`link`, `db`), unknown placeholder, `{program}` in a brew recipe, `{node}` outside npm, `command[0]` not under `{home}`/`{program}`/`{node}`, zero ports, nine ports, duplicate port names, usual port 80, `health.tcp` naming no port, `console.port` naming no port, `http://` download, `file://` download without `allow_file_urls`, short `sha256`, npm `version = "^1.2"`, bad `stop_signal`, bad `start_timeout` — each asserting the message and the 1-based line; `placeholder_values_are_never_split` (`{data}` under `/tmp/a b/"q"` is one argument); `config_text_includes_overrides` (rendered `run.config.text` with `include {overrides}` resolved); `ports_name_registry_entries` (`main` → `<instance>`, `console` → `<instance>-console`).
- [ ] **Step 2: Run** `cargo test -p yard-core recipe` — FAIL.
- [ ] **Step 3: Implement** `parse_recipe` with serde (`toml::Spanned` for line numbers, as `config::services` does) then the checks above; `RecipeEngine::command/config_file/env_lines/connect` expand at call time from pre-validated templates (infallible).
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** (`recipes parsed, checked and expanded in yard-core`).

### Task 4: Redis becomes a carried recipe

**Files:**
- Create: `crates/yard-core/engines/redis.toml`, `crates/yard-core/src/services/engines/carried.rs` (`include_str!` list)
- Delete: `crates/yard-core/src/services/engines/redis_builtin.rs`

**Interfaces:**
- Produces: `pub fn carried() -> Vec<RecipeEngine>` (parsed with `Origin::Carried`; a parse failure is a test failure, never a runtime panic: `carried()` returns only those that parse and a test asserts all do); `Engines::builtin()` now = MySQL, PostgreSQL, MinIO + `carried()`.

- [ ] **Step 1: Write the failing test** `redis_recipe_matches_the_builtin` comparing, for instance `redis` with port 26379, the recipe's `command`, `config_file` (path and every byte), `user_overrides` (`Some("")`), `dirs` (`["data"]`), `env_lines`, `connect`, `start_timeout` (`None`), stop signal SIGTERM and `ports` against `redis_builtin` (kept until this step passes); `every_carried_recipe_loads`.
- [ ] **Step 2: Run** — FAIL (no recipe).
- [ ] **Step 3: Write `redis.toml`** reproducing `render::config_file`'s Redis text verbatim (header line included) and today's `REDIS_*` lines; then delete `redis_builtin.rs` and keep the snapshot of its output as the test's expected value (insta, `crates/yard-core/src/services/snapshots/`).
- [ ] **Step 4: Run** `cargo test -p yard-core` — PASS.
- [ ] **Step 5: Commit** (`redis as a carried recipe`).

### Task 5: Switch Yard onto `Engines` and delete the enum (root workspace + app model)

**Files:**
- Modify: every non-test site in the map: `yard-core` `services/{engine.rs (deleted),mod.rs,render.rs (engine bodies gone; UnitCommand and signals stay)}`, `config/services.rs`, `project.rs`, `db/{mod,connect,resolve,command,sql}.rs`, `render/mod.rs:62`; `yard-daemon` `services.rs`, `reconcile.rs` (units :723-760, config loop :553, MinIO console block :562, `port_owner` :944, `serves_console` :1006, `secure_service_hosts` :1018, `prepare_dirs` :1160), `doctor.rs`, `sleep.rs`, `watch.rs:162`, `sources.rs:165-185`, `rpc/{services,info,db}.rs`, `daemon.rs`, `state.rs`; `yard-cli` `commands/{db,info}.rs`, `cli.rs:278`; `app/yard-app-model/src/db.rs`; tests' `support/mod.rs` and `support/daemon.rs`
- Modify: `.hydra/rules/config-ownership.md` (Redis's data dir is now "the dirs a recipe lists")

**Interfaces:**
- Consumes: Tasks 2–4.
- Produces:
  - `ServiceEntry { engine: EngineKey, version: Option<String>, formula: Option<String>, stopped, sleep_after }`. Parse rules: a name that is `mysql-<line>`/`postgres-<line>` with a valid line needs that `engine`, `version` equal to the line and a `formula`; any other name must be a valid `EngineKey` equal to `engine`; `formula` and `version` are otherwise optional. The "names are" error text lists the rule, not four engines.
  - `parse_instance(name: &str) -> Option<(EngineKey, Option<String>)>` and `instance_name(key: &EngineKey, line: Option<&str>) -> String`, registry-free via `versioned_lines`.
  - `ProjectFile.services: BTreeMap<EngineKey, ServiceWant>`; parse refuses a key that is not a valid `EngineKey` (or `emailulator`), and a line for any key without `versioned_lines` (`"<key> takes only true"`), never an unknown engine.
  - `project::service_needs(project: &ProjectFile, services: &ServicesConfig, engines: &Engines) -> Vec<Need>`: an unknown key is `Need::Manual` (wording in "Decided while planning").
  - `Snapshot.engines: Arc<Engines>` (`Engines::builtin()` until Task 6); daemon functions that took `Engine` take `&dyn ServiceEngine` (or the key and `&Engines`): `port_entries(engine: &dyn ServiceEngine, instance) -> Vec<(String, String, u16)>`, `unit_command(engine, ctx)`, `start_timeout` via the trait, `resolve(brew, engine: &dyn ServiceEngine, line) -> Result<Resolved, YardError>`, `remove/move_port/forget(paths, engine: &dyn ServiceEngine, …)`.
  - Consoles: every engine with `console()` gets `nginx/services/<key>.conf` at `<key>.<tld>` (a linked site of that name wins), its certificate `certs/services/<key>.pem` and its URL in `ServiceInfo.console`; only `nginx_overrides: true` consoles include and watch `services/<i>/overrides.conf`. `port_owner` maps `<instance>-<suffix>` to `<instance>` for every engine.
  - `yard-core::db` works on `DbFlavor` (from `engine.databases()`) instead of `Engine`; `supports_connect` becomes `engine.connect(..).is_some()`; the CLI's and the app's client tables come from `Connect.clients`, which `db.connect`'s answer carries (`clients`, Task 1); `engines.list` says whether an engine has `connect`.
  - `ServiceInfo.engine_name` filled from `display()`.
  - An instance whose key `Engines` does not know: no unit; `left_out` holds `ENGINE_UNKNOWN` (`"no engine named <key>: its recipe is missing"`, hint `restore ~/.yard/engines/<key>.toml, or yard service remove <instance>`); `service.remove` still works (it needs only names and ports already recorded).
  - `.yard` errors that used to say "unknown service" for a typo now become a manual need on the setup page; `yard-shim` keeps compiling against the new `ProjectFile` and ignores `[services]`.

- [ ] **Step 1: Write the failing tests:** core `services_toml_reads_recipe_entries` (`[services.memcached] engine = "memcached"` with no formula; `[services.typesense] engine = "typesense" version = "30.1"`) and `services_toml_keeps_line_checks` (old cases unchanged); `project_unknown_engine_parses` and `project_line_on_unversioned_is_invalid` (`redis = "7"` still `PROJECT_FILE_INVALID`); `unknown_engine_is_a_manual_need`; shim `shim_runs_with_unknown_service` (a `.yard` with `soketi = true` and `php = "8.4"` resolves PHP 8.4); daemon `instance_of_unknown_engine_is_failed_and_removable` (write a `[services.ghost] engine = "ghost"` entry: `services.list` shows `failed` with `ENGINE_UNKNOWN`, reconcile starts nothing for it, `services.remove ghost --keep-data` succeeds and leaves `services/ghost/data`); `minio_console_unchanged` (the existing MinIO console tests pass untouched).
- [ ] **Step 2: Run** `cargo test --workspace` — the new tests FAIL to compile or fail.
- [ ] **Step 3: Migrate** file by file to the interfaces above; delete `services/engine.rs` and the engine bodies in `render.rs`. `grep -rnE 'Engine::|"(mysql|postgres|redis|minio)"' crates app --include='*.rs'` outside `services/engines/`, tests and `yard_proto::engine_display` must return nothing that names an engine.
- [ ] **Step 4: Run** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`; in `app/`: `cargo test --workspace --locked` and clippy — all PASS, with every pre-existing test unchanged except imports.
- [ ] **Step 5: Commit** (`every engine through the contract; the enum is gone`).

### Task 6: User recipes, last good versions, and the watch (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/engines.rs`
- Modify: `crates/yard-core/src/paths.rs` (`engines_dir() -> <root>/engines`, `programs_dir()`, `program_root(key, version)`), `crates/yard-daemon/src/{reconcile.rs,watch.rs,daemon.rs,rpc/handlers.rs}`, `crates/yard-daemon/src/bin/yard-fake-unit.rs` (generic `fake-service`), tests `support/mod.rs` (`Fixture::add_recipe(name, text)`, `Fixture::add_fake_formula(formula, bins)` for `fake-service`)

**Interfaces:**
- Consumes: `parse_recipe`, `Engines`.
- Produces: `pub struct RecipeLoader { last_good: BTreeMap<EngineKey, Arc<RecipeEngine>> }` with `pub fn load(&mut self, paths: &Paths, opts: &LoadOptions) -> Engines` (reads `engines/*.toml`, sorted; a valid user recipe replaces a carried one with its key; a built-in key is an error; a broken file whose key had a last good version keeps it and records the error; a broken user file shadowing a carried key falls back to the carried one; a file that is not `*.toml` is ignored). `Engines::with_user(...)` and `Engines::errors()` filled. `Reconciler` owns a `RecipeLoader`; each reconcile calls `load` first and, when the key set or any recipe's text changed, emits `Event::EnginesChanged` after `reconciled`. `status` reports each `errors()` entry as an `ENGINE_RECIPE_INVALID` warning. The watcher adds `engines_dir()` (created by reconcile if missing) to its directory batch and reconciles on any change in it. `DaemonOptions.allow_file_urls: bool` (default false) feeds `LoadOptions`.
- `yard-fake-unit` named `fake-service`: listens on every argument of the form `127.0.0.1:<port>` and every `--port <n>`, logs its argv to `<argv0 dir>/fake-service.log`, exits 0 on SIGTERM.

- [ ] **Step 1: Write the failing daemon tests** (`crates/yard-daemon/tests/engines.rs`): `user_recipe_is_added_without_restart` (write `engines/echo.toml` with a brew `fake-echo` formula running `fake-service`; within the debounce `services.add echo` works and the instance runs on its allocated port); `user_recipe_replaces_carried` (`engines/redis.toml` with a different `[env] lines`; `env.print` shows the new line); `user_recipe_cannot_replace_builtin` (`engines/mysql.toml` → status warning `ENGINE_RECIPE_INVALID`, MySQL unaffected); `broken_edit_keeps_last_good` (break a running recipe: instance keeps running, same pid, status names `echo.toml:<line>`; fix it: warning clears); `broken_at_start_is_unknown` (yardd started with only a broken `echo.toml` and an `echo` instance → `ENGINE_UNKNOWN`); `deleted_recipe_leaves_data` (delete `echo.toml`: instance stops, `failed` `ENGINE_UNKNOWN`, `services/echo/data` intact); `edited_command_restarts_unit` (change an argument: new pid, `init` not re-run); `engines_changed_event_fires`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test engines` — FAIL.
- [ ] **Step 3: Implement** `engines.rs`, the reconcile hook, the watch and the fake.
- [ ] **Step 4: Run** `cargo test -p yard-daemon` — PASS (if test daemons time out at startup, `cargo clean --profile dev`, never plain `cargo clean`).
- [ ] **Step 5: Commit** (`user recipes in ~/.yard/engines, loaded each reconcile`).

### Task 7: `path` and `download` sources, recorded versions, `engines.install` (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/programs.rs`, `crates/yard-daemon/src/rpc/engines.rs`
- Modify: `crates/yard-core/src/services/engines/recipe.rs` (`pub fn path_allowed(program: &Path, home: &Path) -> Result<(), YardError>`, pure), `crates/yard-daemon/src/services.rs` (`Add::run`/`provision` dispatch on `source()`; `version` written for download/npm, `formula` only for brew), `reconcile.rs` (install root per instance: keg for brew, `program_root(key, entry.version)` for download/npm, missing → `ENGINE_NOT_INSTALLED` in `left_out`), `rpc/handlers.rs`

**Interfaces:**
- Produces: `pub async fn install(job: &JobCtx, paths: &Paths, engine: &RecipeEngine, opts: &LoadOptions) -> Result<PathBuf /* install root */, YardError>`; `pub fn home(entry: &ServiceEntry, engine: &dyn ServiceEngine, brew: &Brew, paths: &Paths) -> Option<PathBuf>`; `pub fn sweep(paths: &Paths, services: &ServicesConfig, engines: &Engines) -> Vec<PathBuf>` (deletes `programs/<key>/<version>` that no instance records and no recipe pins; returns what it removed); RPC `engines.list` (built from `Snapshot.engines`, `installed` per §5, `version` = pin) and `engines.install { key }` (a job: install the pin, then for the key's instance rewrite `version` in `services.toml` and restart it, then `sweep`). `services.remove` calls `sweep` after forgetting.
- Download steps: `/usr/bin/curl -fsSL --proto =https` (`=https,file` with `allow_file_urls`) `-o <programs>/<key>/.stage-<pid>/download <url>`; SHA-256 with `sha2`; mismatch → `DOWNLOAD_HASH_MISMATCH` (`expected <a>, got <b>`, hint: correct `sha256` in the recipe); magic bytes `1f 8b`, `fd 37 7a 58 5a`, `50 4b 03 04` → `/usr/bin/tar -xf download -C stage --strip-components <strip>`, else a bare program saved as `stage/<last URL segment>` mode 0755; rename stage → `program_root`; any failure or cancel removes the stage.

- [ ] **Step 1: Write the failing tests:** core `path_allowed_refuses_herd_dbngin_postgresapp` (each listed directory, with `home` = a temp dir) and `path_allowed_needs_absolute`; daemon `path_source_runs_the_program` (recipe `program = <fixture>/bin/fake-service`; add, runs); `path_source_missing_is_refused`; `download_installs_and_records_version` (a `file://` tar.gz of `fake-service` built by the test, its sha; `services.toml` gets `version = "1.0"` and no `formula`; unit runs from `programs/<key>/1.0/`); `download_bad_hash_leaves_nothing`; `cancelled_download_leaves_nothing` (a `file://` URL to a FIFO the test never writes; cancel the job; no `programs/<key>` children); `pin_change_keeps_running_version` (edit the recipe to `2.0` with a new archive: instance still on 1.0, `engines.list` shows pin 2.0); `engines_install_moves_instance` (then `engines.install`: `services.toml` says 2.0, new pid, `programs/<key>/1.0` swept); `missing_install_root_is_not_installed` (delete `programs/<key>/2.0`: `failed` `ENGINE_NOT_INSTALLED`, hint `yard engines install <key>`).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — PASS.
- [ ] **Step 5: Commit** (`path and download sources, with each instance keeping its version`).

### Task 8: The `npm` source (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/programs.rs`, `crates/yard-daemon/src/bin/yard-fake-unit.rs` (fake fnm installs `npm` as a link to itself; `npm install --prefix P --no-save --no-audit --no-fund <pkg>@<v>` writes `P/node_modules/<pkg>/package.json` and links `P/node_modules/.bin/fake-service` to the fake), tests

**Interfaces:**
- Consumes: `node::installed`, `node::fnm_env`, `yard_core::node::{parse_spec, pick}`, the M4c install path (`node.install`'s job body, factored into `pub async fn install_node(job, paths, spec) -> Result<NodeVersion, YardError>` in `node.rs` if not already callable).
- Produces: the `npm` branch of `programs::install`: pick or install the recipe's `node`, run `<fnm dir>/node-versions/v<x>/installation/bin/npm` by absolute path with PATH = that `bin` + `/usr/bin:/bin`, `npm_config_python=/usr/bin/python3`, `FNM_DIR` unset; `{node}` in `InstanceCtx` set for the unit, whose PATH is the same.

- [ ] **Step 1: Write the failing tests:** `npm_source_installs_with_yards_node` (fake fnm has 22; recipe `node = "22"`; the fake npm's log shows the absolute npm, `PATH` starting with the node `bin`, and `npm_config_python`); `npm_source_installs_missing_node` (no 18 installed; the add job installs it first); `npm_unit_path_has_node_first`.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement.** **Step 4: Run** `cargo test -p yard-daemon` — PASS.
- [ ] **Step 5: Commit** (`the npm source, on Yard's own Node`).

### Task 9: CLI: `yard engines`, `engines check`, `engines install` (yard-cli)

**Files:**
- Create: `crates/yard-cli/src/commands/engines.rs`
- Modify: `crates/yard-cli/src/{cli.rs,main.rs}`, `commands/services.rs` (help text: any engine key; `yard services` shows `ENGINE_UNKNOWN`/`ENGINE_NOT_INSTALLED` and a pin differing from the instance's version), `commands/db.rs` (clients from the answer; `connect`/`shell` for any engine with `connect`), `README.md`, `DEVELOPMENT.md`

**Interfaces:**
- Consumes: `engines.list`, `engines.install`, `parse_recipe`, `MockDaemon` in tests.
- Produces: `yard engines [--json]` (columns `ENGINE NAME SOURCE ORIGIN INSTALLED`, then each load error as `<file>:<line>: <why>`); `yard engines check <file>` (no daemon: parse with `Origin::User`, print errors and exit 1, or print the `command` line, ports with usual numbers, `env` lines and `[console]` for an instance named for the key with its usual ports); `yard engines install <key>` (job with progress like `service add`).

- [ ] **Step 1: Write failing CLI tests** (insta, `MockDaemon`): `engines_lists_all_origins`, `engines_json_is_the_wire_type`, `engines_check_good_file` (no daemon running), `engines_check_reports_line` (exit 1), `engines_install_runs_job`, `services_shows_unknown_engine`.
- [ ] **Step 2: Run** `cargo test -p yard-cli` — FAIL. **Step 3: Implement** (cli-output rule: human table on a tty, `--json` the proto type). **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** (`yard engines`).

### Task 10: Per-engine commands; `yard db` on its way out (yard-cli, yard-daemon)

**Files:**
- Create: `crates/yard-cli/src/commands/engine_cmd.rs`
- Modify: `crates/yard-cli/src/{cli.rs,main.rs}` (`allow_external_subcommands`; `Db` hidden; an "Engines" help section), `crates/yard-cli/src/commands/db.rs` (its verbs take an engine and become the bodies the engine commands call), `crates/yard-core/src/services/engines/mod.rs` (`YARD_COMMANDS`), `crates/yard-core/src/db/resolve.rs` (`pick` filters by key instead of `allow: fn(Engine)`), `crates/yard-daemon/src/rpc/db.rs` (honour `engine`; `db.command` kind `shell` for any engine with `connect.shell`, waking it first), `scripts/e2e-db.sh` (new commands, plus one `yard db list` that must still work and print the notice)

**Interfaces:**
- Consumes: `Engines` (built by the CLI from `Engines::builtin()` plus `~/.yard/engines` through a pure `load_user(paths, texts)` that Task 6's loader shares), `db.*` with `engine`.
- Produces: `pub const YARD_COMMANDS: &[&str]` (every top-level command and alias, `db` and `pgsql` included); `pub fn engine_verbs(engine: &dyn ServiceEngine) -> Vec<Verb>` with `Verb { Shell, List, Create, Dump, Import, Connect, Open }` (rules in spec §9.1); `yard <key> [verb] [flags] [-- args]`, bare = `shell`; `pgsql` → `postgres`; `yard db <verb>` → the same body with the notice on stderr.

- [ ] **Step 1: Write the failing tests:** core `yard_commands_cover_the_cli` (in yard-cli: every clap subcommand name and alias is in `YARD_COMMANDS`); `verbs_follow_the_contract` (MySQL all seven; Redis `shell`, `connect`, `open`; Meilisearch `connect`, `open`; Memcached none but `connect` if its recipe has one); CLI (`MockDaemon`): `yard_mysql_bare_is_shell`, `yard_pgsql_is_postgres`, `yard_mysql_service_of_other_engine_refused` (`--service postgres-17` → usage error), `yard_redis_list_names_its_verbs` (exit 2), `recipe_engine_command_works_offline_for_check` (a user recipe's `yard echo connect` asks the daemon; with none running it says yardd is not running, not "unknown command"), `yard_db_still_works_with_notice`, `help_lists_engine_commands`.
- [ ] **Step 2: Run** `cargo test -p yard-cli -p yard-core` — FAIL.
- [ ] **Step 3: Implement.** The verbs' flags are exactly today's `DbSub` flags.
- [ ] **Step 4: Run** `cargo test --workspace`, clippy, fmt — PASS; `shellcheck scripts/e2e-db.sh` clean.
- [ ] **Step 5: Commit** (`yard mysql, yard postgres and a command for every engine`).

### Task 11: Doctor (yard-daemon)

**Files:** Modify `crates/yard-daemon/src/doctor.rs`; tests `crates/yard-daemon/tests/doctor.rs`.

**Interfaces:** Produces checks `engine:<key>` (fail, from `Engines::errors()`), `engine-program:<i>` (fail: install root or `path` program missing; hint `yard engines install <key>` or `brew reinstall <formula>`), `engine-version:<i>` (info: pin ≠ recorded version, naming both), `service-exposed:<i>` (warn: a running recipe instance with a listener on `*` or a non-loopback address, read through the existing `lsof` holder code), `programs-unused` (info: install roots `sweep` would remove, with their size). `service-data:<i>` is checked only for engines with `databases()`.

- [ ] **Step 1: Write failing tests** for each check, `service-exposed` using `fake-service` started with `0.0.0.0:<port>`.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement.** **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** (`doctor checks for recipes and their programs`).

### Task 12: Carried recipes: Valkey, Memcached, Meilisearch, Typesense

**Files:** Create `crates/yard-core/engines/{valkey,memcached,meilisearch,typesense}.toml`; modify `carried.rs`.

Values (spec §8): ports 6379 / 11211 / 7700 / 8108; `yard env` lines `REDIS_HOST=127.0.0.1`, `REDIS_PORT=<p>`, `REDIS_PASSWORD=null` (Valkey); `MEMCACHED_HOST=127.0.0.1`, `MEMCACHED_PORT=<p>`; `MEILISEARCH_HOST=http://127.0.0.1:<p>`, `MEILISEARCH_KEY=`; `TYPESENSE_HOST=127.0.0.1`, `TYPESENSE_PORT=<p>`, `TYPESENSE_PROTOCOL=http`, `TYPESENSE_API_KEY=yard`. Valkey: brew `valkey`, `valkey-server {conf}` with a `valkey.conf` shaped like Redis's (`bind 127.0.0.1`, `port`, `dir {data}`, `daemonize no`, include overrides), `dirs = ["data"]`, connect `redis://` URL and `valkey-cli -p`. Memcached: brew `memcached`, `memcached -l 127.0.0.1 -p {port.main} -U 0 -m 64`, no dirs. Meilisearch: brew `meilisearch`, spec §6's example command, `dirs = ["data"]`, console `port = "main"`, `host_header = "client"`. Typesense: download of the current Apple Silicon `typesense-server-<v>-darwin-arm64.tar.gz` from `dl.typesense.org`, its SHA-256 computed from the downloaded file (`curl -fsSL <url> | shasum -a 256`), command `{home}/typesense-server --data-dir {data} --api-key yard --listen-address 127.0.0.1 --api-port {port.main} --enable-cors`, `dirs = ["data"]`.

- [ ] **Step 1: Write failing tests:** `every_carried_recipe_loads` covers the five; `valkey_beside_redis_takes_next_port` (daemon: add `redis` then `valkey` on the fixture's fake brew; valkey's port is not redis's and `env.print` names it); per-recipe snapshot of `command` and `env_lines` with fixed ports.
- [ ] **Step 2: Run** — FAIL. **Step 3: Write the recipes**; check each program's flags against `--help` of the real binary only after asking the maintainer to install it (homebrew rule), else from upstream docs, and note which in the commit. **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** (`valkey, memcached, meilisearch and typesense recipes`).

### Task 13: Yard.app (app/)

**Files:** Modify `app/yard-app/src/services/{forms.rs,table.rs,db.rs}`, `app/yard-app/src/{runner.rs,env.rs,sites/detail.rs,state.rs}`, `app/yard-app-model/src/db.rs`; `app/scripts/checklist.sh` (a new item); `.hydra/rules/app.md` if it names the engine list.

**Interfaces:** Consumes `engines.list`, `Event::EnginesChanged`, `ServiceInfo.engine_name`, `db.connect`'s `clients`. Produces: the database screen's dump and import run `yard <engine> dump|import` instead of `yard db …`; the Add picker lists `engines.list` (plus emailulator as today), refreshed on `engines_changed`; names shown from `engine_name` (falling back to `engine_display` for an older yardd); Connect… shown when the engine's `connect` is true, clients from the answer; the Services screen shows each recipe load error under its header.

- [ ] **Step 1: Write failing model tests** in `yard-app-model`: `picker_lists_engines_from_daemon`, `picker_falls_back_without_engines_list` (older yardd: the old fixed list), `connect_clients_from_answer`.
- [ ] **Step 2: Run** `cargo test --workspace --locked` in `app/` — FAIL. **Step 3: Implement.** **Step 4: Run** tests and clippy — PASS; `scripts/bundle.sh`, then `app/scripts/checklist.sh` with its new item (drop `engines/echo.toml` into `/tmp/yard-check/engines`, find "Echo" in the Add picker within 5 s without a restart) — all items pass.
- [ ] **Step 5: Commit** (`Yard.app lists engines from yardd`); remove `app/target` afterwards.

### Task 14: Real-machine check (`scripts/e2e-engines.sh`)

**Files:** Create `scripts/e2e-engines.sh` (shellcheck clean, bash 3.2).

Behaviour (spec §13): refuses to start if `valkey`, `memcached`, `meilisearch`, `typesense`, `e2e-path` or `e2e-npm` is in `services.toml`; backs up `services.toml`, `ports.toml`, `sites.toml` and restores them in a `trap`; checksums Herd's `/etc/resolver/test` and the listeners on 3306, 5432 and 6379; adds the four carried engines, then asks each its own question by absolute path (`$(brew --prefix)/opt/valkey/bin/valkey-cli -p <p> PING` → `PONG`; `printf 'version\r\n' | nc 127.0.0.1 <p>` → `VERSION`; `curl -fsS 127.0.0.1:<p>/health` for Meilisearch → `available`, for Typesense → `"ok":true`); checks `yard env` in a scratch linked project whose `.yard` lists them; curls `http://meilisearch.yard:8080/`; writes `~/.yard/engines/e2e-path.toml` (a `path` recipe running `/usr/bin/nc -lk 127.0.0.1 {port.main}`) and `e2e-npm.toml` (`npm` of `http-server@14.1.1`, `node = "22"`, `{node} {home}/node_modules/http-server/bin/http-server -a 127.0.0.1 -p {port.main} {data}`), adds and curls both; sets `sleep-after 60s` on memcached, waits for it to sleep, wakes it with a connection; removes everything it added and the two recipe files, then runs `yard engines` to confirm only carried and built-in engines remain. `scripts/e2e-m4b.sh` and `scripts/e2e-m4e.sh` run unchanged against the Redis recipe.

- [ ] **Step 1: Write the script**; `shellcheck scripts/e2e-engines.sh` clean.
- [ ] **Step 2: Ask the maintainer** to approve the run and the installs (`brew install valkey memcached meilisearch`, the Typesense download, Node 22 if missing); `cargo build --release`, `yard stop && yard start`.
- [ ] **Step 3: Run** `scripts/e2e-m1.sh`, `scripts/e2e-engines.sh`, `scripts/e2e-m4b.sh`, `scripts/e2e-m4e.sh` — all PASS; record what each found.
- [ ] **Step 4: Commit** (`e2e-engines passes on <machine>`).

### Task 15: Docs and the spec's amendments

**Files:** `AGENTS.md` (Status, Key decisions → Services, Build & test: `e2e-engines.sh`, Layout: `services/engines/`, `crates/yard-core/engines/`), `README.md` (writing a recipe, `yard engines`), `DEVELOPMENT.md`, `.hydra/rules/{homebrew.md,config-ownership.md,rust-workspace.md (fake-service, add_recipe)}`, the M4b spec (§2, §3 non-goals, §5 point to the recipes spec), the recipes spec (Status: built; §7/§11 amended with "Decided while planning": `Engines`, loading in reconcile, `Need::Manual`, `db.connect` clients).

- [ ] **Step 1: Update each file**; then `grep -rn "Engine::\|four engines\|mysql, postgres, redis, minio\|yard db " --include='*.md' .` and fix what no longer matches (README's and AGENTS.md's `yard db` usage, e2e-db.sh's description; Status history stays as written). Add to AGENTS.md Status that `yard db` is removed in the release after the one that ships this.
- [ ] **Step 2: Run** `cargo test --workspace`, clippy, fmt, `cargo deny check` (no lockfile change expected; run it if one happened) — PASS.
- [ ] **Step 3: Commit** (`service recipes documented`).
