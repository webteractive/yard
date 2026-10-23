# Yard M4b (services, `yard env`, `yard info`, brew sources) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Yard runs MySQL, PostgreSQL, Redis and MinIO instances the way DBngin does, a project's `.yard` asks for them, `yard env` and `yard info` show how to reach them, and every brew install from a third-party tap waits for the user's trust in brew's own store.

**Architecture:** `yard-core` gains a pure `services` module: the engine table (lines, formulae, defaults, ports, commands, rendered configs, `.env` lines, accounts) and `services.toml`. yardd adds each instance as a supervised unit in reconcile, allocates its ports once at `add` and records them `pinned`, and runs `add` as a job (resolve, trust gate, `brew install`, initialize data, reconcile, wait for health). A new `sources` module reads and changes trust only through `brew trust`, and `php.install`, `ext.add` and `services.add` check it before their job starts. The CLI gains `services`, `service`, `env`, `info`, `sources` and `source`, and asks before trusting.

**Tech Stack:** Rust 2024; `serde_json` for brew's JSON; `toml`/`toml_edit`; `insta`; no new crates.

**Spec:** `docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md` (all of it), with `docs/superpowers/specs/2026-10-19-yard-design.md` §3.3, §6.1–6.3, §7, §8.2 and `docs/superpowers/specs/2026-10-22-yard-project-file-design.md` §4.

## Global Constraints

- Engines: `mysql` (lines `"8.0"`, `"8.4"`, … from brew; default `"8.4"`), `postgres` (`"15"`–`"18"`, `"14"` while brew has it; default `"17"`), `redis` (`true` only), `minio` (`true` only, formula `minio/stable/minio`). No MariaDB, Valkey or emailulator.
- Instance names: `mysql-<line>`, `postgres-<line>`, `redis`, `minio`. A line is a formula, never a patch release.
- Every service listens on 127.0.0.1 only. MySQL sets `mysqlx = OFF`.
- Ports: allocated once at `add` (`--port`, else the usual port, else the next free one above that is not registered and not reserved), recorded `pinned`, never moved by Yard. Reserved while installed: DBngin.app → 3306, 5432, 6379; Postgres.app → 5432 (`/Applications` or `~/Applications`).
- Accounts shown everywhere: MySQL `root` / no password; PostgreSQL the macOS user / no password; Redis no auth; MinIO `minioadmin` / `minioadmin`. Yard stores no other password.
- Yard creates no databases, roles or buckets, imports no data, and never runs `brew services` or `brew uninstall` for a service.
- Trust: read only with `brew trust --json=v1`, changed only by `brew trust`/`brew untrust` through `yard source trust|untrust` or the CLI's y/N prompt. `--yes`, `init`, `link` and installs never grant it. Yard sets no `HOMEBREW_NO_*` variable. Homebrew older than 6.0 skips the gate.
- `.yard` `[services]`: keys `mysql`, `postgres`, `redis`, `minio`; values a line or `true`. A missing instance is a need (setup page); a stopped or failed one is not.
- New error codes (never renamed): `SERVICE_NOT_FOUND`, `SERVICE_EXISTS`, `SERVICE_IN_USE`, `SERVICE_VERSION_UNSUPPORTED`, `FORMULA_UNTRUSTED`, `NEEDS_CONFIRMATION`. Protocol `2.4` → `2.5`.
- Config ownership: `services.toml` through `TomlFile` only; `services/<i>/overrides.conf` created once and never written; generated files carry the header and their hash.
- `yard-core` stays pure: reads, spawns and the clock are injected (`.hydra/rules/rust-workspace.md`).
- Tests never run real brew, a real database or real `brew trust`, never touch `~/.homebrew`, never bind 53, 80, 443, 3306, 5432, 6379, 8080, 8443, 9000, 9001 or 53535: every usual port in a test comes from `free_port()` through the engine table's test override (Task 2).
- Docs move with each task. Commits: lowercase prose subject, no type prefix, no trailers.

## Decisions (rulings made while planning; the maintainer reviews them)

1. **The trust gate runs in the RPC handler, before the job starts.** `FORMULA_UNTRUSTED` is then an ordinary error the CLI can catch, prompt on and retry, instead of a failed job. Cost if wrong: a second check inside the job.
2. **Lines come from brew.** `add` runs `brew search --formula "/^<base>(@|$)/"` for the names and `brew info --json=v2 <name>` for versions, taps and deprecation. Nothing in Yard hard-codes `9.7` or `26.10`.
3. **Service ports are allocated by `services.add`, not reconcile.** Reconcile only re-reads the pinned entries; if a user deleted one from `ports.toml`, reconcile allocates it again the same way.
4. **Orphans are found by pid file.** The supervisor writes `run/<unit>.pid` for units whose spec asks for one (services do); at startup yardd stops a recorded process whose executable is that unit's program and whose parent is launchd.
5. **`minio.<tld>` sends `Host` as the upstream's `127.0.0.1:<console>`** and `X-Forwarded-Host $http_host`. M4d's `mail.<tld>` uses the same option.
6. **`Need::Service { engine, version: Option<String> }`**, `None` for `true`; adding it adds the default line.
7. **The fake brew reads `<prefix>/fake-formulae.json`** (brew's `info --json=v2` formula objects) for `search`, `info` and `install`, and `<prefix>/fake-trust.json` for `trust`.
8. **`HolderKind` gains `Dbngin` and `PostgresApp`**, so a conflict reads `mysqld (pid 77034, DBngin)`. DBngin's servers run from `/Users/Shared/DBngin/`, Postgres.app's from `/Applications/Postgres.app/`.
9. **Doctor learns the shell's brew settings from the CLI**: `DoctorParams { client_env }` carries `XDG_CONFIG_HOME` and every `HOMEBREW_FORBIDDEN_*` the CLI sees.
10. **`yard sites` keeps its columns.** Spec §9 says `yard sites` shows a stopped service's status; `SiteInfo` carries no service state, so `yard info` and `yard doctor` show it instead. Cost if wrong: one column and a `SiteInfo` field.
11. **Plan approval.** the maintainer approving this plan approves executing it with a commit per task. Task 14's run on the real data root, which brew-installs engines, waits for his go-ahead.

## Review Focus

1. **A typo in `services/<i>/overrides.conf` makes the engine exit at once.** The instance goes `failed` with its last lines, doctor names the override, and every site and other instance keeps serving. Pinned by Task 8's `broken_override_fails_only_that_instance`.
2. **`yard service add` cancelled or failing after it initialized data** leaves no data directory, no `services.toml` entry and no port entry, and the same add works again. Pinned by Task 9's `cancelled_add_leaves_nothing_behind`.
3. **Two instances of one engine added while the usual port is free** get the usual port and the next one, never the same. Pinned by Task 5's `second_instance_skips_registered_port` and Task 9's `two_mysql_lines_run_side_by_side`.
4. **A hand-edited `services.toml` with an unknown key** is `CONFIG_INVALID` with file and line, and the last good state keeps serving. Pinned by Task 3's `unknown_service_key_names_line` and Task 8's `invalid_services_toml_keeps_serving`.
5. **yardd killed while an instance runs** stops the leftover process on its next start, and the instance comes back on its own port. Pinned by Task 6's `orphan_service_is_stopped_on_start`.

---

### Task 1: Protocol types, methods and error codes

**Files:**
- Modify: `crates/yard-proto/src/types.rs`, `methods.rs`, `error.rs`, `lib.rs`
- Modify: CLI test expecting `"2.4"`; every exhaustive `match` on `Need` and `HolderKind`
- Docs: main spec §6.2 (method list), §6.3 (codes)

**Interfaces:**
- Produces: `Need::Service { engine: String, version: Option<String> }`. `describe()` → `"MySQL 8.4"`, `"Redis"`; `command()` → `"yard service add mysql --version 8.4"`, `"yard service add redis"`. Engine display names: `MySQL`, `PostgreSQL`, `Redis`, `MinIO`.
- `ServiceState` (`#[serde(rename_all = "snake_case")]`): `Running`, `Starting`, `Stopped`, `Failed { last_lines: Vec<String> }`.
- `Account { user: Option<String>, password: Option<String> }`: `None` user means no auth; `Some("")` password means none.
- `ServiceInfo { name, engine, version: Option<String>, formula, ports: Vec<PortRef>, state: ServiceState, stopped_by_user: bool, account: Account, console: Option<String>, used_by: Vec<String> }`, `PortRef { name: String, port: u16 }`.
- `EnvBlock { instance: String, lines: Vec<String>, note: Option<String> }`.
- `SourceInfo { tap, tapped: bool, trusted: bool, formulae: Vec<SourceFormula> }`, `SourceFormula { name, trusted: bool, installed: bool }`.
- `SettingSource` (`project_file`, `sites_toml`, `default`). `SiteDetail { site: SiteInfo, php_full: Option<String>, sources: BTreeMap<String, SettingSource>, services: Vec<SiteService> }`, `SiteService { engine, want: Option<String>, service: Option<ServiceInfo> }`.
- `HolderKind::{Dbngin, PostgresApp}`, labelled `DBngin` and `Postgres.app`.
- Methods: `SERVICES_LIST|ADD|REMOVE|START|STOP|RESTART|LOGS|PORT` (`"services.list"`, …), `ENV_PRINT = "env.print"`, `SITES_INFO = "sites.info"`, `SOURCES_LIST|TRUST|UNTRUST`.
- Params: `ServiceAddParams { engine, version: Option<String>, port: Option<u16> }`, `ServiceParams { name }`, `ServiceRemoveParams { name, keep_data: bool }`, `ServicePortParams { name, port: u16 }`, `ServiceLogsParams { name, lines: Option<usize> }`, `EnvParams { path, site, service: Option<String> }`, `SiteDetailParams { path, site: Option<String> }`, `SourceParams { target: String, tap: bool }`, and `DoctorParams` gains `client_env: BTreeMap<String, String>` (`#[serde(default)]`). Optional fields default.
- `ErrorCode::{ServiceNotFound, ServiceExists, ServiceInUse, ServiceVersionUnsupported, FormulaUntrusted, NeedsConfirmation}`. `PROTOCOL_VERSION = "2.5"`.

- [ ] **Step 1: Write the failing tests** in `yard-proto`: `need_service_is_tagged` (`{"kind":"service","engine":"mysql","version":"8.4"}`), `need_service_describe_and_command` (both forms above), `service_state_failed_carries_lines`, `doctor_params_without_client_env_parse`, `holder_labels` gains the two kinds; extend the method-name and error-code tests.
- [ ] **Step 2: Run** `cargo test -p yard-proto` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; fix the exhaustive matches (the CLI's `settle` ignores `Service` until Task 13).
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs** §6.2, §6.3. **Commit** `service, env, info and sources types, protocol 2.5`

### Task 2: The engine table (yard-core)

**Files:**
- Create: `crates/yard-core/src/services/mod.rs`, `services/engine.rs`, `services/brew_info.rs` (registered in `lib.rs`)

**Interfaces:**
- Produces: `Engine` enum `Mysql | Postgres | Redis | Minio`, with `key() -> &str` (`.yard` key and `engine` value), `from_key(&str) -> Option<Engine>`, `display()`, `base_formula()` (`mysql`, `postgresql`, `redis`, `minio`), `versioned() -> bool` (MySQL, PostgreSQL), `default_line() -> Option<&str>`, `usual_ports() -> &[(&str, u16)]` (`[("", 3306)]`, `[("", 5432)]`, `[("", 6379)]`, `[("", 9000), ("console", 9001)]`; the suffix names the registry entry `<instance>-<suffix>`), `account(macos_user: &str) -> Account`, `valid_line(&str) -> bool` (MySQL `^\d+\.\d+$`, PostgreSQL `^\d+$`, others never).
- `instance_name(engine, line: Option<&str>) -> String`; `parse_instance(name) -> Option<(Engine, Option<String>)>`.
- `brew_info::Formula { name, full_name, tap, version: String, deprecated: bool, deprecation_date: Option<String>, disable_date: Option<String>, aliases: Vec<String> }` and `brew_info::parse(json: &str) -> Result<Vec<Formula>, String>` over `brew info --json=v2`'s `formulae`.
- `resolve_line(engine, line: &str, names: &[String], info: &dyn Fn(&str) -> Option<Formula>) -> Result<Formula, YardError>`: `<base>@<line>` when `names` has it, else the plain formula when its `version`'s major (PostgreSQL) or major.minor (MySQL) equals `line`; else `SERVICE_VERSION_UNSUPPORTED` whose message lists the lines found and whose hint is `yard service add <key> --version <first line>`.
- `lines(engine, names, info) -> Vec<String>`: every line `resolve_line` accepts, sorted by version.
- `env_lines(engine, ports: &[PortRef], account: &Account) -> Vec<String>`: exactly spec §7.3's table.
- Unit tests pass ports explicitly. The daemon takes its usual ports from `DaemonOptions.usual_ports: Option<BTreeMap<String, u16>>` (Task 8), keyed `mysql`, `postgres`, `redis`, `minio`, `minio-console`; `None` means the engine table's, and daemon tests always set it from `free_port()`.

- [ ] **Step 1: Write the failing tests**:
  - `resolve_prefers_versioned_formula`: names `[mysql, mysql@8.0, mysql@8.4, mysql@9.7]`, `"8.4"` → `mysql@8.4`.
  - `resolve_falls_back_to_plain_formula`: plain `mysql` version `26.10.0`, `"26.10"` → `mysql`; `"26.11"` → `SERVICE_VERSION_UNSUPPORTED` naming `8.0, 8.4, 9.7, 26.10`.
  - `postgres_lines_are_majors`: `postgresql@17` for `"17"`; `"17.2"` is not a valid line.
  - `instance_names`: `mysql-8.4`, `postgres-17`, `redis`, `minio`, and `parse_instance` round-trips them; `mysql` alone and `redis-8` do not parse.
  - `env_lines_match_spec` for all four engines (MySQL port 3307 → `DB_PORT=3307`, `DB_USERNAME=root`, `DB_PASSWORD=`; PostgreSQL `DB_CONNECTION=pgsql`, `DB_USERNAME=me`; Redis `REDIS_PASSWORD=null`; MinIO the five `AWS_*` lines with `AWS_ENDPOINT=http://127.0.0.1:9000`).
  - `brew_info_parses_real_shape`: a trimmed real `brew info --json=v2 mysql@8.0` (deprecated, dates) parses.
- [ ] **Step 2: Run** `cargo test -p yard-core services` — Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p yard-core` — Expected: PASS.
- [ ] **Step 5: Commit** `the service engine table`

### Task 3: `services.toml` and `.yard` `[services]` (yard-core)

**Files:**
- Create: `crates/yard-core/src/config/services.rs`
- Modify: `crates/yard-core/src/config/mod.rs`, `paths.rs`, `project.rs`
- Docs: project-file spec §4 (`[services]` rules from M4b spec §9), §6 (the commented block)

**Interfaces:**
- Consumes: Task 2's `Engine`, `instance_name`, `parse_instance`.
- Produces: `Paths::services_toml()`, `services_dir()`, `service_dir(i)`, `service_data(i)` (`services/<i>/data`), `service_conf(i, file)`, `service_overrides(i)` (`services/<i>/overrides.conf`), `service_pid(i)` (`run/<i>.pid`), `nginx_services_dir()`, `nginx_service(name)`, `services_certs_dir()`.
- `ServicesConfig { services: BTreeMap<String, ServiceEntry> }`, `ServiceEntry { engine: Engine, version: Option<String>, formula: String, stopped: bool }` (`deny_unknown_fields`; `stopped` defaults false). `load_services(paths)`, `parse_services(text, file)`: a name that `parse_instance` rejects, or whose engine/line disagree with `engine`/`version`, is `CONFIG_INVALID` at its span.
- `ProjectFile.services: BTreeMap<Engine, ServiceWant>`, `ServiceWant::{Line(String), Default}`. `true` → `Default`; `false`, a number, a table, `"latest"` or a line the engine rejects is `PROJECT_FILE_INVALID` at its line; an unknown key names it.
- `project::service_needs(file: &ProjectFile, services: &ServicesConfig) -> Vec<Need>`: a `Line` is met by `instance_name(engine, line)`; `Default` by any entry of that engine.
- `project::render` appends `# [services]` and the four commented lines of spec §9 after the other keys.

- [ ] **Step 1: Write the failing tests**:
  - `parses_services_toml`: the spec §4 example gives two entries, `postgres-17` stopped.
  - `unknown_service_key_names_line`: `port = 3306` under `[services."mysql-8.4"]` → `CONFIG_INVALID`, `data.line` is its line.
  - `instance_name_must_match_engine`: `[services."mysql-8.4"]` with `engine = "postgres"` is invalid.
  - `project_services_parse`: `mysql = "8.4"`, `redis = true` parse; `redis = "8"`, `mysql = "latest"`, `mysql = 8.4`, `minio = false`, `mongo = true` are each `PROJECT_FILE_INVALID` naming their line.
  - `service_needs_table`: `mysql = "8.4"` with only `mysql-8.0` → `[Service mysql Some("8.4")]`; `mysql = true` with `mysql-8.0` → `[]`; `redis = true` with nothing → `[Service redis None]`.
  - Update the `render` snapshot/test for the commented block.
- [ ] **Step 2: Run** `cargo test -p yard-core` — Expected: FAIL.
- [ ] **Step 3: Implement** with `Spanned` keys, as `project.rs` does for extensions.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS (M4a's project tests unchanged apart from `render`).
- [ ] **Step 5: Docs; commit** `services.toml and the .yard [services] table`

### Task 4: Rendering engine configs, commands and `minio.<tld>` (yard-core)

**Files:**
- Create: `crates/yard-core/src/services/render.rs`
- Modify: `crates/yard-core/src/render/nginx.rs` (service block, `X-Forwarded-Host $http_host`, `nginx_main` includes `nginx/services/*.conf`), `render/mod.rs` (`overrides_for`)

**Interfaces:**
- Consumes: Tasks 2–3.
- Produces: `UnitCommand { program: PathBuf, args: Vec<OsString>, env: Vec<(String, String)>, stop_signal: i32 }` and `services::render::command(engine, instance, keg: &Path, paths: &Paths, ports: &[PortRef]) -> UnitCommand`, per spec §5.1–5.4: `mysqld --defaults-file=<conf>`; `postgres -D <data> -c listen_addresses=127.0.0.1 -c port=<p> -c unix_socket_directories=<run>` with SIGINT; `redis-server <conf>`; `minio server <data> --address 127.0.0.1:<p> --console-address 127.0.0.1:<c>` with `MINIO_ROOT_USER`/`MINIO_ROOT_PASSWORD` = `minioadmin`.
- `init_command(engine, instance, keg, paths, macos_user) -> Option<UnitCommand>`: MySQL `mysqld --initialize-insecure --datadir=<data> --basedir=<keg>`; PostgreSQL `initdb -D <data> -U <user> --auth=trust -E UTF8`; else `None`.
- `config_file(engine, instance, paths, ports) -> Option<(PathBuf, String)>`: `services/<i>/my.cnf` (`[mysqld]`, `port`, `bind-address = 127.0.0.1`, `socket = "<run>/<i>.sock"`, `datadir`, `mysqlx = OFF`, last line `!include "<overrides>"`) or `services/<i>/redis.conf` (`bind 127.0.0.1`, `port`, `dir "<data>"`, `daemonize no`, last `include "<overrides>"`). Headed `# Generated by Yard — edit <overrides> instead`; paths quoted.
- `nginx::service_site(ctx, name: &str, upstream_port: u16, secure: bool) -> String`: `server_name <name>.<tld>`, `proxy_set_header Host 127.0.0.1:<port>`, `X-Forwarded-Host $http_host`, `X-Forwarded-Proto $scheme`, websocket headers.
- The site proxy block sends `X-Forwarded-Host $http_host` (spec §11).

- [ ] **Step 1: Write the failing tests**: snapshots `mysql_cnf`, `redis_conf`, `nginx_service_minio`, `nginx_service_minio_secure`; `commands_per_engine` (program is `<keg>/bin/mysqld` etc., MinIO env has both variables, PostgreSQL stop is SIGINT); `paths_with_spaces_are_quoted` (root `/tmp/a b`); update the existing proxy snapshots to `$http_host`; `overrides_for` maps `services/<i>/my.cnf` to `services/<i>/overrides.conf`.
- [ ] **Step 2: Run** `cargo test -p yard-core render` — Expected: FAIL.
- [ ] **Step 3: Implement**; review every new snapshot against spec §5 before accepting it.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `render service configs and minio.<tld>, and keep the port in X-Forwarded-Host`

### Task 5: Allocation and holders (yard-core)

**Files:**
- Modify: `crates/yard-core/src/ports/mod.rs`, `ports/holder.rs`
- Create: `crates/yard-core/src/services/reserved.rs`
- Docs: `.hydra/rules/port-registry.md` (pinned service ports, reserved ports, DBngin/Postgres.app holders); main spec §7 (services allocate pinned; replace the `mysql-8-0`/`redis` example lines)

**Interfaces:**
- Produces: `Registry::allocate_from(&mut self, unit: &str, usual: u16, reserved: &BTreeSet<u16>, is_free: &dyn Fn(u16) -> bool) -> Result<u16, YardError>`: the first port `p >= usual` (up to 65535) that is not registered to another unit, not in `reserved`, and `is_free`; recorded as `pinned`. `Registry::allocate_exact(unit, port, is_free)` for `--port`: `PORT_IN_USE` if registered elsewhere or busy.
- `services::reserved::reserved_ports(apps: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> BTreeSet<u16>`; the daemon passes `/Applications` and `~/Applications`.
- `classify` returns `Dbngin` for paths under `/Users/Shared/DBngin/` or containing `/DBngin.app/`, `PostgresApp` for `/Postgres.app/`, checked after Herd.

- [ ] **Step 1: Write the failing tests**: `allocate_from_takes_usual_when_free`; `allocate_from_skips_busy_and_reserved` (usual 3306 busy, 3307 reserved → 3308, entry is pinned); `second_instance_skips_registered_port` (two calls with the same usual and everything free → 3306 then 3307); `allocate_exact_refuses_registered`; `reserved_ports_for_installed_apps` (DBngin only → `{3306, 5432, 6379}`; Postgres.app only → `{5432}`; none → empty); `classifies_dbngin_and_postgres_app` with the two real paths.
- [ ] **Step 2: Run** `cargo test -p yard-core ports` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs; commit** `allocate service ports once, around ports other apps use`

### Task 6: Supervisor: several ports, a stop timeout and pid files (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/supervisor/mod.rs`, `supervisor/unit.rs`, `reconcile.rs` (existing specs), `portscan.rs` (own units' ports)
- Create: `crates/yard-daemon/src/orphans.rs`
- Test: `crates/yard-daemon/tests/supervisor.rs`

**Interfaces:**
- Produces: `UnitSpec.ports: Vec<(u16, Proto)>` (replaces `port`; nginx and dnsmasq pass one or none), `UnitSpec.stop_timeout: Option<Duration>` (`None` = `SupervisorConfig.stop_timeout`), `UnitSpec.pid_file: Option<PathBuf>`, written on spawn and removed on exit.
- `async orphans::stop(pid_file: &Path, program: &Path, grace: Duration, signal: i32) -> Option<u32>` (async, waits on tokio's clock; `signal` is the unit's own `UnitSpec.stop`, so an orphaned postgres gets SIGINT): when the pid file names a live process whose `proc_pidpath` canonicalizes to `program` and whose parent is 1, send `signal`, SIGKILL after `grace`, remove the file, and return the pid. Anything else: remove a stale file, return `None`.
- Service units use `stop_timeout: Some(30 s)` (tests: `fast()` sets a short one through `SupervisorConfig`).

- [ ] **Step 1: Write the failing tests**: `every_port_is_probed_before_start` (a unit with two ports, the second held → `Failed` with the holder, no backoff); `unit_stop_timeout_overrides_default` (a `signals` fake that ignores SIGTERM is killed after the unit's timeout, not the config's); `pid_file_follows_the_process`; `orphan_service_is_stopped_on_start` (spawn the fake detached with `setsid` so its parent becomes launchd, write its pid file, call `orphans::stop` → it exits and the port frees); `orphan_ignores_other_programs` (a pid file naming an unrelated live pid leaves it running).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test supervisor` — Expected: FAIL.
- [ ] **Step 3: Implement**; update the existing specs (`ports: vec![(dns, Proto::TcpUdp)]`, …).
- [ ] **Step 4: Run** `cargo test --workspace` three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `units with several ports, their own stop timeout and a pid file`

### Task 7: Brew sources and the trust gate (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/sources.rs`, `crates/yard-daemon/src/rpc/sources.rs`; in `yard-core`: `crates/yard-core/src/sources.rs` (pure parsing)
- Modify: `rpc/handlers.rs`, `rpc/php.rs` (`install`), `rpc/ext.rs` (`add`, brew source), `src/bin/yard-fake-unit.rs` (`trust`, `untrust`, `--version`, `search`, `info --json=v2`, install of service formulae)
- Test: `crates/yard-daemon/tests/sources.rs`
- Docs: `.hydra/rules/homebrew.md` (the gate; never `HOMEBREW_NO_*`; trust only via `brew trust`)

**Interfaces:**
- Produces (core): `sources::TrustStore { taps: BTreeSet<String>, formulae: BTreeSet<String> }`, `sources::parse_trust(json) -> Result<TrustStore, String>` (`brew trust --json=v1`'s `taps`/`formulae`), `sources::tap_of(formula) -> Option<&str>` (`None` for a bare name or `homebrew/core/…`), `TrustStore::trusts(formula) -> bool` (core, the formula, or its tap), `sources::YARD_TAPS = ["shivammathur/php", "shivammathur/extensions", "minio/stable"]`, `sources::brew_major(version_output) -> Option<u32>` (`Homebrew 7.0.10` → 7).
- Produces (daemon): `sources::require_trusted(brew: &Brew, formula: &str) -> Result<(), YardError>`: runs `brew --version` (skip the gate below 6) and `brew trust --json=v1`; an untrusted formula is `FORMULA_UNTRUSTED`, message `<formula> comes from the <tap> tap, which brew does not trust yet`, hint `yard source trust <formula>`, `data { formula, tap }`. `sources::list(brew, yard) -> Vec<SourceInfo>`; `sources::trust(job, brew, target, tap)` / `untrust` run `brew trust|untrust --formula|--tap <target>` as jobs.
- `php.install` and brew-source `ext.add` call `require_trusted` before `jobs.start`.
- Fake brew: `--version` prints `Homebrew` and `<prefix>/fake-brew-version` (default `7.0.10`); `trust --json=v1` prints `fake-trust.json` (default `{"taps":[],"formulae":[],"casks":[],"commands":[]}`); `trust`/`untrust --formula|--tap X` edits it; `search --formula "/^base(@|$)/"` prints matching names from `fake-formulae.json`; `info --json=v2 X` prints `{"formulae":[<entry>]}` or fails like brew; `install X` for an entry creates `opt/<name>/bin/<engine binaries>` linked to the fake (`mysqld`; `postgres`, `initdb`; `redis-server`; `minio`).

- [ ] **Step 1: Write the failing tests**: core `trust_store_rules` (core formula trusted with an empty store; `shivammathur/php/php@8.3` trusted by its formula or its tap, not otherwise); `brew_major_parses`. Daemon: `php_install_refuses_untrusted_formula` (empty fake trust → `FORMULA_UNTRUSTED` with the hint, no job, nothing in `fake-brew.log` but `--version` and `trust`); `php_install_proceeds_when_tap_trusted`; `old_brew_skips_the_gate` (`fake-brew-version` `5.1.0`); `ext_add_refuses_untrusted_formula`; `source_trust_runs_brew_trust` (job; `fake-trust.json` then has the formula); `source_trust_tap` (`--tap`); `sources_list_reports_trust` (three taps, `shivammathur/php` trusted as a tap, `php@7.1` installed); `no_homebrew_no_vars_are_set` (the fake records its environment; no `HOMEBREW_NO_` key other than the existing `HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK`).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test sources` — Expected: FAIL.
- [ ] **Step 3: Implement.** Existing php/ext tests seed `fake-trust.json` with `shivammathur/php` and `shivammathur/extensions` in `Fixture::new`, so they keep passing.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs; commit** `ask brew whether a tap is trusted before installing from it`

### Task 8: Reconcile runs service instances (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/reconcile.rs`, `state.rs` (`Snapshot.services: ServicesConfig`), `watch.rs` (`services.toml`), `daemon.rs` (`DaemonOptions.usual_ports`, startup `orphans::stop` for each instance before the first reconcile), `project.rs` (`fill_needs` adds `service_needs`), `src/bin/yard-fake-unit.rs` (engine fakes)
- Test: `crates/yard-daemon/tests/services.rs`

**Interfaces:**
- Consumes: Tasks 2–6.
- Produces: reconcile loads `services.toml` with the other TOMLs (a parse error keeps the snapshot); for each entry, ensures its pinned ports (`allocate_from` if missing), renders `config_file` and `ensure_user_file(service_overrides)`, renders `nginx/services/minio.conf` (and its certificate under `certs/services/` when `secure_by_default`) when a `minio` entry exists and no site is named `minio`; adds a unit per entry not `stopped` whose keg binary exists (else a warning `mysql-8.4: <keg> is missing (brew install <formula>)`), with `pid_file`, `stop_timeout` 30 s, health `Tcp(first port)`, and restarts it when its config file changed. `managed` gains `nginx/services`. `fill_needs` adds service needs, so a missing instance gives the setup page.
- `services::service_info(snapshot, supervisor, name) -> Option<ServiceInfo>` (in `crates/yard-daemon/src/services.rs`, used by Tasks 9–12; `None` for a name not in `services.toml`, which callers answer `SERVICE_NOT_FOUND`): state from the unit (`Stopped` with `stopped_by_user` when `stopped`), `used_by` from sites whose `.yard` asks for it, `console` `http(s)://minio.<tld>` for MinIO, with the nginx profile's port when that is not 80/443 (the console is proxied through nginx, not the `minio-console` port).
- Engine fakes in `yard-fake-unit` (by `argv[0]`): `mysqld --initialize-insecure --datadir=D` creates `D/ibdata1`; `mysqld --defaults-file=F` exits 1 with `unknown variable` when the overrides file it includes contains `bad`, else listens on `port`; `initdb -D D` creates `D/PG_VERSION`; `postgres … -c port=N` listens; `redis-server F` listens on `port`; `minio server … --address A --console-address C` listens on both. `mysqld --initialize-insecure` and `initdb` wait `<prefix>/fake-init-delay` milliseconds and fail when `<prefix>/fake-init-fail` exists, the prefix being three directories above the binary (`<prefix>/opt/<formula>/bin/<name>`).
- `Fixture::add_formula(name: &str)` creates `opt/<name>/bin/<engine binaries>` linked to the fake, as the fake brew's `install` does.

- [ ] **Step 1: Write the failing tests** (each seeds `services.toml`, `ports.toml` entries from `free_port()`, and kegs through a new `Fixture::add_formula(name)`):
  - `instance_runs_with_its_config`: `mysql-8.4` → unit `running`, `services/mysql-8.4/my.cnf` has `bind-address = 127.0.0.1` and the port, `overrides.conf` exists and is empty.
  - `stopped_instance_stays_stopped`: `stopped = true` → no unit after two reconciles.
  - `broken_override_fails_only_that_instance`: `bad` in `overrides.conf` → `mysql-8.4` `failed` with its last lines; a `redis` instance and a linked site keep running.
  - `missing_instance_is_a_site_need`: `.yard` `mysql = "8.4"` with no entry → the site's needs are `[Service mysql 8.4]` and it has a setup page; adding the entry clears it.
  - `stopped_instance_is_not_a_site_need`.
  - `minio_gets_its_host`: a `minio` entry renders `nginx/services/minio.conf`; a linked site named `minio` suppresses it.
  - `invalid_services_toml_keeps_serving`: an unknown key → `status` shows `config_error`, units keep running.
  - `services_toml_edit_applies`: editing the file through the watcher starts a new instance within 3 s.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test services` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `reconcile runs the service instances services.toml lists`

### Task 9: `services.add`, `remove` and `list` (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/rpc/services.rs`
- Modify: `rpc/handlers.rs`, `crates/yard-daemon/src/services.rs`
- Test: `crates/yard-daemon/tests/services.rs`

**Interfaces:**
- Consumes: Tasks 2, 5, 7, 8.
- Produces: `services.list -> Vec<ServiceInfo>`.
- `services.add(ServiceAddParams) -> JobStarted`. Before the job: unknown engine → `INVALID_ARGUMENT`; `version` given for Redis/MinIO, or not a valid line → `INVALID_ARGUMENT`; instance exists → `SERVICE_EXISTS`; resolve the formula (`search`, `info`; Task 2) → `SERVICE_VERSION_UNSUPPORTED`; `require_trusted` for a non-core tap. For MinIO with `opt/minio` already present, the installed keg's tap (`brew info --json=v2 minio`) decides: a core `minio` is used as it is, without a trust check (spec §15). The job: `brew install <full_name>` when the keg is missing; allocate ports (`allocate_exact` for `--port`, else `allocate_from` with `reserved_ports`) in `ports.toml`; run `init_command` when `services/<i>/data` is empty (job line `reusing the data in <dir>` otherwise); write the entry; reconcile; wait for `running` (60 s, `failed` ends it with the last lines); a deprecated formula adds a job line naming brew's disable date. On failure or cancel after it created them: remove the data directory it created, the entry and the port entries.
- `services.remove(ServiceRemoveParams) -> ()`: `SERVICE_NOT_FOUND`; `SERVICE_IN_USE` naming the linked sites whose `.yard` asks for it (hint `edit <file>`); else remove the entry and port entries, reconcile (stopping the unit), delete `services/<i>/` (keeping `data/` with `keep_data`). Confirmation is the CLI's (Task 13).

- [ ] **Step 1: Write the failing tests**:
  - `add_installs_initializes_and_runs`: `fake-formulae.json` with `mysql@8.4`; `services.add mysql 8.4` → job lines include `brew install mysql@8.4`; `data/ibdata1` exists; the instance runs on the free usual port; `services.list` shows `account.user == "root"`.
  - `two_mysql_lines_run_side_by_side`: `8.0` then `8.4` with the usual port free → distinct pinned ports, both running.
  - `add_respects_reserved_ports`: a fake `/Applications/DBngin.app` in the test's app dirs → MySQL skips the usual port.
  - `add_unknown_line_lists_lines`: `9.9` → `SERVICE_VERSION_UNSUPPORTED` naming `8.0, 8.4`.
  - `add_twice_is_service_exists`.
  - `add_minio_needs_trust`: untrusted `minio/stable` → `FORMULA_UNTRUSTED`, no job.
  - `cancelled_add_leaves_nothing_behind`: `fake-init-delay` 5000; cancel after the job line `initializing` → no `services/mysql-8.4`, no entry, no port entry; with the delay removed a second add succeeds.
  - `failed_init_rolls_back`: `fake-init-fail` with `postgres 17` → the same three are gone.
  - `add_uses_an_installed_core_minio`: `opt/minio` present and `brew info --json=v2 minio` reporting tap `homebrew/core` → no `brew install`, no trust check, and a job line naming brew's disable date.
  - `remove_refuses_in_use` and `remove_deletes_data` / `remove_keep_data_then_add_reuses`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test services` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs** main spec §8.2 (commands as built). **Commit** `yard service add, remove and list`

### Task 10: `start`, `stop`, `restart`, `logs`, `port` (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/rpc/services.rs`, `rpc/handlers.rs`
- Test: `crates/yard-daemon/tests/services.rs`

**Interfaces:**
- Produces: `services.stop`/`start(ServiceParams) -> ServiceInfo`: set or remove `stopped` through `TomlFile`, reconcile. `services.restart` → `units.restart` for the instance. `services.logs(ServiceLogsParams) -> Vec<String>`: the last `lines` (default 100) of `logs/<i>.log`. `services.port(ServicePortParams) -> ServiceInfo`: `allocate_exact` on the instance's first port (refuse busy or registered with `PORT_IN_USE` and the holder), reconcile, which restarts it.

- [ ] **Step 1: Write the failing tests**: `stop_survives_restart_of_yardd` (stop, shut the daemon down, start it on the same root → still stopped); `start_clears_stopped`; `port_moves_and_restarts` (new free port → listening there, old one free); `port_refuses_busy` (a `held_listener` → `PORT_IN_USE`); `logs_tail` (fake writes lines; `lines: 2` returns the last two); each on an unknown name → `SERVICE_NOT_FOUND`.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `start, stop, restart, logs and port for service instances`

### Task 11: `env.print` and `sites.info` (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/rpc/info.rs`
- Modify: `rpc/handlers.rs`; `crates/yard-core/src/php.rs` (`full_version`)
- Test: `crates/yard-daemon/tests/services.rs`, `tests/project.rs`

**Interfaces:**
- Produces: `env.print(EnvParams) -> Vec<EnvBlock>`. `service` → that instance; else `site`, else the linked site whose path contains `path`, → the services its `.yard` lists (a missing one is a block with no lines and `note` `mysql-8.4 is not added: yard service add mysql --version 8.4`; `true` picks the default line's instance if present, else the first by name); else every instance. Two DB engines in one project add `note` `both set DB_*; the second block wins in a .env` to the second.
- `sites.info(SiteDetailParams) -> SiteDetail`: the site by name or by `path`, else `SITE_NOT_FOUND` with hint `yard info <site>` / `yard link`. `sources` has `php`, `docroot`, `secure`: `project_file` when the parsed `.yard` sets it, `sites_toml` when the entry does, else `default`. `php_full` from `php::full_version(read_link(opt))`: the Cellar directory name without a `_N` revision (`7.3.33_8` → `7.3.33`). `services` lists each `[services]` entry with its `ServiceInfo` when met.

- [ ] **Step 1: Write the failing tests**: `env_for_project_lists_its_services` (two instances, `.yard` asks for one → one block whose lines carry its real port); `env_names_a_missing_instance`; `env_without_project_lists_all`; `env_two_databases_note`; `info_by_path_and_name` (sources: `php` from `project_file`, `secure` `default`; `php_full` `7.3.33`); `info_outside_a_project_is_site_not_found`; core `full_version_strips_revision`.
- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `env.print and sites.info`

### Task 12: Doctor (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs`, `rpc/handlers.rs` (`DoctorParams`)
- Test: `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Produces checks (spec §10), ids `service:<i>`, `service-data:<i>`, `service-need:<site>`, `service-line:<i>`, `service-deprecated:<i>`, `sources:trust`, `sources:brew`, `sources:env`, `php7.0-mysql:<site>`:
  - failed instance → `fail`, last lines, hint `yard service restart <i>`; port held → existing port check with the new holder kinds;
  - empty or missing data → `fail`, hint `yard service remove <i> && yard service add <engine> …`;
  - `.yard` asking for a stopped or missing instance → `warn` naming the site;
  - plain-formula instance whose keg version's line differs from its name → `warn`;
  - deprecated formula → `warn` with the disable date (one `brew info --json=v2` call for all instance formulae);
  - installed formula from a Yard tap that is not trusted → `warn`, hint `yard source trust <formula>`; brew below 6 → `warn`;
  - `client_env` sets `XDG_CONFIG_HOME` or a `HOMEBREW_FORBIDDEN_*` that yardd's environment lacks or differs from → `warn`, hint `put it in ~/.homebrew/brew.env`;
  - a PHP 7.0 site whose `.yard` asks for `mysql` → `warn` naming "authentication method unknown".
- Doctor without a daemon (in-process) runs the file-based ones and skips unit state, as today.

- [ ] **Step 1: Write the failing tests**: one per check above with its id and status, plus `healthy_services_are_ok`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test doctor` (skip the known flake `security_does_not_inherit_a_listener`) — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `doctor checks for services and brew sources`

### Task 13: The CLI (yard-cli)

**Files:**
- Create: `crates/yard-cli/src/commands/services.rs`, `commands/env.rs`, `commands/info.rs`, `commands/sources.rs`
- Modify: `cli.rs`, `commands/mod.rs`, `commands/init.rs` (`settle` handles `Need::Service` after extensions; trust prompt), `commands/php.rs`, `commands/ext.rs`, `commands/doctor.rs` (`client_env`), `tests/support/mod.rs` (`MockDaemon` answers for the new methods)
- Test: `crates/yard-cli/tests/cli.rs` and snapshots
- Docs: README (services, `yard env`, `yard info`, sources), `--help` text, `.hydra/rules/cli-output.md` if a pattern is new

**Interfaces:**
- Consumes: Task 1's types and methods.
- Produces commands exactly as spec §7.1, each with `--json`. `yard services` table `INSTANCE ENGINE VERSION PORT STATE ACCOUNT USED BY` (ACCOUNT `root (no password)`, `me (no password)`, `— (no auth)`, `minioadmin / minioadmin`). `yard service add` runs the job, then prints the env lines. `yard service remove` asks `Delete <i> and its data? [y/N]` (or `Delete <i>? Its data stays in <dir>. [y/N]` with `--keep-data`); `--yes` skips; no terminal and no `--yes` → `NEEDS_CONFIRMATION`, hint `yard service remove <i> --yes`, exit 1. `yard service logs -f` prints the tail, then follows `log_line` events for the unit until Ctrl-C. `yard env` prints `# <instance>` then the lines, notes as `# …`. `yard info` prints spec §7.4's layout.
- `sources::with_trust<T>(ctx, call: impl Fn() -> Future<Result<T, CliError>>) -> Result<T, CliError>`: on `FORMULA_UNTRUSTED` and `interactive(ctx)`, asks `Trust <formula> (from the <tap> tap)? [y/N]`, on yes runs the `sources.trust` job and calls again; otherwise returns the error. `--yes` does not change this. Used by `php install`, `ext add`, `service add` and `settle`.
- `yard doctor` sends `client_env`.

- [ ] **Step 1: Write the failing tests** against `MockDaemon`: snapshots for `services`, `env`, `info`, `sources` (human); `--json` round-trips for each; `service_remove_needs_confirmation_without_tty`; `service_remove_yes_calls_remove`; `untrusted_install_without_tty_fails_with_hint` (exit 1, hint `yard source trust …`, no `sources.trust` call); `yes_does_not_grant_trust` (`yard init --yes` whose need is PHP 8.3 and whose `php.install` answers `FORMULA_UNTRUSTED` → exit 1 with that error, no `sources.trust` call); `init_yes_adds_missing_services` (needs `[Service mysql 8.4]` → a `services.add` call after the PHP and extension jobs); `doctor_sends_client_env` (`HOMEBREW_FORBIDDEN_FORMULAE` set in the test's env appears in the params).
- [ ] **Step 2: Run** `cargo test -p yard-cli` — Expected: FAIL.
- [ ] **Step 3: Implement**; accept snapshots by moving `.snap.new` over `.snap` after reading each.
- [ ] **Step 4: Run** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` — Expected: all pass.
- [ ] **Step 5: Docs; commit** `yard services, service, env, info, sources and source`

### Task 14: Docs and the real-machine check

**Files:**
- Create: `scripts/e2e-m4b.sh`
- Modify: main spec (§3.3, §6.2, §6.3, §7, §8.2, §10, §12), project-file spec §4/§6, README, AGENTS.md (status, Build & test, environment findings), `.hydra/rules/config-ownership.md` (`services.toml`, `services/<i>/overrides.conf`), the M4d plan header (re-check note: M4b names are now `Engine`, `allocate_from`, `UnitSpec.ports`, `services.port`)

- [ ] **Step 1: Write `scripts/e2e-m4b.sh`** in `e2e-m4a.sh`'s style: needs `e2e-m1.sh` first; backs up `services.toml`, `ports.toml`, `sites.toml`; records the listeners on 3306, 5432, 6379 and Herd's resolver hash; `yard service add mysql --version 8.4`, `postgres --version 17`, `redis`, `minio` (`--yes`; minio fails fast with `FORMULA_UNTRUSTED` unless `minio/stable` is trusted, which it is here); checks `"$(brew --prefix)/opt/mysql@8.4/bin/mysql" -h 127.0.0.1 -P <port> -u root -e 'select 1'`, `opt/postgresql@17/bin/psql -h 127.0.0.1 -p <port> -c 'select 1' postgres`, `opt/redis/bin/redis-cli -p <port> ping` → `PONG`, `curl -fsS http://127.0.0.1:<api>/minio/health/live`, and `curl -s -o /dev/null -w '%{http_code}' http://minio.yard:8080/` → `200`; `yard env --json` ports equal `yard services --json` ports; links a scratch project with `.yard` `[services] mysql = "8.4"` and checks `yard info` shows it met; then removes every instance it added (`--yes`), restores the backups, and fails if the 3306/5432/6379 listeners or Herd's resolver changed.
- [ ] **Step 2: Docs** as listed; `grep -rn "mysql-8-0\|services.list\|yard service" docs README.md AGENTS.md .hydra` and fix what no longer matches.
- [ ] **Step 3: Run** `cargo test --workspace`, clippy, fmt — Expected: PASS.
- [ ] **Step 4: Commit** `M4b docs and the e2e check for services`
- [ ] **Step 5: Ask the maintainer**, then `cargo build --release`, `yard stop && yard start`, and run `scripts/e2e-m1.sh` then `scripts/e2e-m4b.sh` (it brew-installs `mysql@8.4`, `postgresql@17` and `redis` if missing). Record the result in AGENTS.md and commit `record that M4b passed its e2e on this machine`.
