# Yard M4d (emailulator as a service) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Approved by the maintainer on 2026-10-26.** M4e may be built first (its plan says so).
>
> **Before execution, two gates.** (1) Spec §4, who runs emailulator, is decided: **B, the
> agent only** (2026-10-26, Claude's call; the maintainer asked Claude to decide). Task 8 is built and
> Task 7 is not; every other task is built either way. (2) This
> plan was written before the M4b plan exists. Every name marked **(M4b)** in an
> Interfaces block is an assumption (spec §14). Re-check each one against the approved
> M4b plan and rename here before Task 1 starts.

> **M4b re-check (2026-10-24, M4b built on branch `m4b-services`).** The names below were
> assumptions; M4b built these, so rename them in the Interfaces blocks before Task 1:
>
> - `Need::Service { service, version }` is `Need::Service { engine: String, version: Option<String> }`
>   (`None` is `true`). `ServiceInfo` is `{ name, engine, version, formula, ports, state,
>   stopped_by_user, account, console, used_by }`; M4d's `managed_by` and `web_url` are added to
>   that (`console` already holds MinIO's URL, so `web_url` may be that field renamed or reused).
> - The engine table is the `yard_core::services::Engine` enum (`ALL`, `key()`, `usual_ports()`,
>   `account()`, `default_line()`, plus the free functions `env_lines` and `instance_name` in
>   `services/engine.rs` and `command` in `services/render.rs`); `Engine` derives serde as its key, so a fifth engine
>   changes `ALL`, `yard_proto::engine_display`, the reserved-port tables and the tests that
>   pass free ports through the table's override. `ServiceEntry` is `{ engine, version, formula,
>   stopped }` with `deny_unknown_fields`, so M4d's `path` field is a new key there and in the parser.
> - `Registry::ensure_usual` does not exist. Ports are allocated once at `add` with
>   `Registry::allocate_from(unit, usual, reserved, is_free)` (the first free port at or above
>   the usual one) or `allocate_exact` (`--port`, never moves on), and recorded `pinned`;
>   `reserved_ports` names what DBngin and Postgres.app hold. Use these, or add an `external`
>   path beside them for the agent's ports.
> - `UnitSpec.ports: Vec<(u16, Proto)>`, `stop_timeout` and `pid_file` already exist (M4b Task 6);
>   nothing to change in the supervisor. `services.port` moves an instance's **first** port only
>   (`allocate_exact`); a second port moves with `yard ports set`.
> - `env.print { path?, site?, service? }` answers `Vec<EnvBlock { instance, lines, note }>`;
>   M4d's "warnings in the result" is `note` on the block (the CLI prints notes as `# ...`), and
>   `yard env` has no stderr warning channel beyond what `commands/env.rs` prints.
> - **Service nginx files map to the instance's own override.** `render::overrides_for` sends
>   `nginx/services/<name>.conf` to `services/<name>/overrides.conf` (created empty once, included
>   by `service_site`), not to `nginx_override("mail")`. Either keep that for emailulator, so its
>   file names `services/emailulator/overrides.conf`, or special-case `mail` and change this plan's
>   Task 3 interface and header test.
> - `X-Forwarded-Host $http_host` is already fixed in M4b: `proxy_location` in
>   `crates/yard-core/src/render/nginx.rs` sends it for every proxy block, `yard proxy` sites
>   included (spec §16's last item is stale and is corrected there).
> - `nginx::service_site(ctx, name, upstream_port, secure)` always sends `Host 127.0.0.1:<port>`
>   (the upstream) and proxies to `http://` only. `mail.<tld>` wants that same `Host`, so it can
>   reuse it; M4d needs a toggle on the function (or a sibling) if mail must send another `Host`,
>   and a `proxy_ssl_verify off` variant for `MailHost.upstream_tls`.

**Goal:** A project on Yard reaches emailulator without looking up a port: `yard env` prints the `MAIL_*` lines, `mail.<tld>` opens the web UI, doctor says who serves mail, and `emailulator = true` in `.yard` is met on `yard init`.

**Architecture:** `yard-core` gains a pure `emailulator` module (find the binary, parse `--version` and `service status --json`, decide who owns mail, the env lines) and an emailulator engine for M4b's engine table. yardd asks the binary about its LaunchAgent on each reconcile and port scan, records the ports as `external` registry entries while the agent owns mail, and renders `nginx/services/emailulator.conf` for `mail.<tld>` with `Host` rewritten to emailulator's loopback address. Under option A, yardd also runs `emailulator run` as an M4b service unit whenever no agent is loaded.

**Tech Stack:** Rust 2024; `serde_json` (already in the workspace); `insta`; no new crates.

**Spec:** `docs/superpowers/specs/2026-10-23-yard-emailulator-design.md` (all of it), with `docs/superpowers/specs/2026-10-19-yard-design.md` §7, §8.2, and the `.yard` spec §4–§6.

## Global Constraints

- Yard never downloads, installs, updates or deletes the emailulator binary, never writes its LaunchAgent plist, never runs `launchctl` on it, and never deletes its message store.
- Yard never runs a second emailulator server while the agent is `installed` and `loaded`.
- The binary is the recorded `path`, else `~/.local/bin/emailulator`; the CLI resolves `emailulator` on its own PATH. Minimum version 0.1.3.
- The agent's state comes only from `<bin> service status --json` (exit 0, 3 or 4), with a 5 s timeout and the `cloexec_inherited` hook. Never parse the plist; never hard-code the label.
- Unit command (A): `<bin> run --smtp-addr 127.0.0.1:<smtp> --http-addr 127.0.0.1:<http>`, no `--data-dir`, no inherited `EMAILULATOR_*` variables.
- Registry entries: `emailulator-smtp` (usual 1025) and `emailulator-http` (usual 8025); `external` with the agent's ports while the agent owns mail.
- `mail.<tld>` sends `Host 127.0.0.1:<http>`, `X-Forwarded-Host $http_host`, `X-Forwarded-Proto $scheme`, `proxy_buffering off`, `proxy_read_timeout 1h`, `client_max_body_size 25m`. It is not a site.
- `yard env` lines, in this order: `MAIL_MAILER=smtp`, `MAIL_HOST=127.0.0.1`, `MAIL_PORT=<smtp>`, `MAIL_USERNAME=null`, `MAIL_PASSWORD=null`, `MAIL_ENCRYPTION=null`, `MAIL_SCHEME=null`.
- Error codes (new, never renamed): `EMAILULATOR_NOT_FOUND`, `SERVICE_EXTERNAL`. Protocol: additive, one minor over M4b's.
- `yard-core` stays pure; spawning and clocks are injected (`.hydra/rules/rust-workspace.md`).
- Tests never run the real emailulator, never touch `~/Library/LaunchAgents`, and never bind 1025, 8025, 53, 80, 443, 8080, 8443 or 53535: they use the fake binary and OS-assigned ports.
- Docs move with each task. Commits: lowercase prose subject, no type prefix, no trailers, only after the maintainer approves this plan.

## Review Focus

1. **The web UI through `mail.<tld>` is refused by emailulator's Host guard** (403) if the proxy passes the browser's `Host`, and a POST from the UI is refused if `X-Forwarded-Host` drops the port. Pinned by Task 3's `mail_host_rewrites_host` snapshot and Task 12's e2e curl through nginx.
2. **An agent that launchd is restarting** (loaded, not running) must not let Yard start a server in the gap. Pinned by Task 2's `loaded_agent_owns_mail_even_when_not_running` and, for A, Task 7's `no_unit_while_agent_is_loaded`.
3. **`service status` that hangs, prints garbage, or does not exist** (emailulator older than 0.1.3) never stalls reconcile for other sites. Pinned by Task 4's `status_timeout_keeps_last_state` and `old_emailulator_has_no_agent`.
4. **A linked site named `mail`** keeps being served and `mail.<tld>` is not rendered over it. Pinned by Task 5's `site_named_mail_wins`.
5. **The agent reinstalled on other ports** moves the registry entries, the proxy and `yard env` within one scan. Pinned by Task 5's `agent_port_change_is_followed`.

---

### Task 1: Protocol types and error codes

**Files:**
- Modify: `crates/yard-proto/src/types.rs`, `error.rs`, `lib.rs`
- Modify: every `ServiceInfo { .. }` literal (M4b's tests, `yard-cli/tests/support/mod.rs`)
- Docs: main spec §6.3 (the two codes)

**Interfaces:**
- Consumes **(M4b)**: `types::ServiceInfo`, `types::Need::Service { service: String, version: Option<String> }`.
- Produces: `Need::Manual { what: String, command: String }`. `describe()` is `what`; `command()` is `command`. `Need::installable(&self) -> bool`: false only for `Manual`.
- `types::ServiceManager` (`#[serde(tag = "kind", rename_all = "snake_case")]`): `Yard`, `LaunchAgent { label: String, pid: Option<u32> }`.
- `ServiceInfo` gains `managed_by: ServiceManager` (`#[serde(default)]`, default `Yard`) and `web_url: Option<String>` (`skip_serializing_if`).
- `ErrorCode::{EmailulatorNotFound, ServiceExternal}`. `PROTOCOL_VERSION` = M4b's plus one minor.

- [ ] **Step 1: Write the failing tests** in `yard-proto`: `manual_need_is_tagged` (`{"kind":"manual","what":"emailulator","command":"curl …"}`), `manual_need_is_not_installable`, `service_info_without_manager_is_yard` (an M4b-era `ServiceInfo` parses with `managed_by == Yard`, `web_url == None`), `launch_agent_manager_round_trips`; extend the error-code test with `EMAILULATOR_NOT_FOUND` and `SERVICE_EXTERNAL`.
- [ ] **Step 2: Run** `cargo test -p yard-proto` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; add the fields to every literal; bump the protocol and the CLI test that expects the old version.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs** §6.3. **Commit** `emailulator protocol: manual needs, who manages a service, two error codes`

### Task 2: The emailulator module (yard-core)

**Files:**
- Create: `crates/yard-core/src/emailulator.rs` (registered in `lib.rs`)
- Modify **(M4b)**: the engine table in `crates/yard-core/src/services/` gains `emailulator`; `crates/yard-core/src/project.rs` (`[services]` parsing)

**Interfaces:**
- Produces:
  - `pub const INSTALLER: &str = "curl -fsSL https://raw.githubusercontent.com/webteractive/emailulator/main/install.sh | sh";` `pub const MIN_VERSION: (u32, u32, u32) = (0, 1, 3);` `SMTP_UNIT = "emailulator-smtp"`, `HTTP_UNIT = "emailulator-http"`, `SMTP_USUAL = 1025`, `HTTP_USUAL = 8025`, `MAIL_HOST_LABEL = "mail"`.
  - `locate(recorded: Option<&Path>, home: &Path, is_exec: &dyn Fn(&Path) -> bool) -> Result<PathBuf, YardError>`: the recorded path, else `<home>/.local/bin/emailulator`; else `EMAILULATOR_NOT_FOUND`, hint `INSTALLER`, `data.searched`.
  - `parse_version(stdout: &str) -> Option<(u32, u32, u32)>`: `"emailulator 0.1.3 (823e513, 2026-10-20)"` → `(0, 1, 3)`.
  - `AgentStatus { label: String, installed: bool, loaded: bool, running: bool, answering: bool, pid: Option<u32>, smtp: SocketAddr, http: SocketAddr, tls: bool, log: Option<PathBuf> }`.
  - `parse_status(exit: i32, stdout: &str) -> Result<Option<AgentStatus>, String>`: exit 4 → `Ok(None)`; exit 0 or 3 with JSON → `Ok(Some)` (missing `loaded`/`running` read as false; `0.0.0.0` becomes `127.0.0.1`); anything else → `Err(reason)`.
  - `enum Owner { Agent(AgentStatus), Yard, Nobody }`; `owner(agent: Option<&AgentStatus>, yard_instance: bool) -> Owner`: an agent that is `installed && loaded` wins; else `Yard` when Yard has an instance (A only; B passes `false`); else `Nobody`.
  - `run_args(smtp: u16, http: u16) -> Vec<String>`: `["run", "--smtp-addr", "127.0.0.1:<smtp>", "--http-addr", "127.0.0.1:<http>"]`.
  - `env_lines(host: &str, smtp: u16) -> Vec<(String, String)>`: the seven Global Constraints lines.
- `.yard`: `[services] emailulator` accepts `true`/`false` only; a string is `PROJECT_FILE_INVALID` at its line, message `emailulator takes true: Yard does not choose its version`.

- [ ] **Step 1: Write the failing tests** in `emailulator.rs`: `locate_prefers_recorded_then_local_bin`; `locate_not_found_names_installer` (code, hint `INSTALLER`, `data.searched` has both paths); `parse_version_forms` (`0.1.3`, `1.10.0`, garbage → `None`); `parse_status_installed` (the JSON this machine printed on 2026-10-23, `pid 3261`, ports 1025/8025); `parse_status_not_installed` (exit 4 → `None`); `parse_status_wildcard_is_loopback`; `parse_status_garbage_is_err`; `loaded_agent_owns_mail_even_when_not_running`; `unloaded_agent_does_not_own_mail`; `env_lines_exact` (all seven, in order). In `project.rs`: `emailulator_takes_only_true`.
- [ ] **Step 2: Run** `cargo test -p yard-core emailulator` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; status JSON through a `#[serde(default)]` raw struct, unknown fields ignored.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `find emailulator and read its LaunchAgent`

### Task 3: Rendering `mail.<tld>` (yard-core)

**Files:**
- Modify: `crates/yard-core/src/render/nginx.rs`, `render/mod.rs` (`overrides_for`), `crates/yard-core/src/paths.rs`
- Test: snapshots under `crates/yard-core/src/render/snapshots/`

**Interfaces:**
- Produces: `Paths::nginx_services_dir()` (`nginx/services`), `Paths::nginx_service(name)` (`nginx/services/<name>.conf`), `Paths::service_cert(name)` / `service_cert_key(name)` (`certs/services/<name>.pem|.key`).
- `nginx::MailHost { host: String, upstream: SocketAddr, upstream_tls: bool, secure: bool }`.
- `nginx::mail_host(ctx: &RenderCtx, m: &MailHost) -> String`: header `# Generated by Yard — edit <nginx/overrides/mail.conf> instead`; `server_name <host> *.<host>`; the http/https/redirect shape of `nginx_site`; the location block with exactly the headers in Global Constraints, `proxy_pass http://127.0.0.1:<port>` (`https://` and `proxy_ssl_verify off` when `upstream_tls`), plus `Upgrade`/`Connection` for websockets; `include` of the override.
- `nginx_main` includes `nginx/services/*.conf` after the sites.
- `overrides_for(nginx/services/emailulator.conf)` is `nginx_override("mail")`.

- [ ] **Step 1: Write the failing tests**: snapshots `mail_host_http` and `mail_host_secure`; `mail_host_rewrites_host` (asserts the block contains `proxy_set_header Host 127.0.0.1:8025;` and `proxy_set_header X-Forwarded-Host $http_host;` and does not contain `Host $host`); `mail_host_tls_upstream`; extend the `overrides_for` header test and the `nginx_main` snapshot.
- [ ] **Step 2: Run** `cargo test -p yard-core render` — Expected: FAIL.
- [ ] **Step 3: Implement**; review each new snapshot against spec §10 before accepting it.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `render mail.<tld> with emailulator's host rewritten`

### Task 4: Asking the binary (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/emailulator.rs`
- Modify: `crates/yard-daemon/src/daemon.rs` (`DaemonOptions.home: PathBuf`, `production` uses `$HOME`), `src/bin/yard-fake-unit.rs` (an `emailulator` personality), `tests/support/daemon.rs` (install the fake as `<tmp>/home/.local/bin/emailulator`)
- Test: `crates/yard-daemon/tests/emailulator.rs`

**Interfaces:**
- Consumes: Task 2.
- Produces: `emailulator::Probe { bin: PathBuf, version: Option<(u32,u32,u32)>, agent: Result<Option<AgentStatus>, String> }`; `async fn probe(bin: &Path, timeout: Duration) -> Probe`. It runs `<bin> --version` and, when the version is at least 0.1.3, `<bin> service status --json`, each with the `cloexec_inherited` hook, an empty `EMAILULATOR_*` environment, and the timeout (child killed on expiry). Below 0.1.3, `agent` is `Ok(None)`.
- `State` keeps `last_probe: Option<Probe>`; a probe whose `agent` is `Err` keeps the previous `agent` and records the reason for doctor.
- The fake: `--version` prints `$YARD_FAKE_EMAILULATOR_VERSION` (default `emailulator 0.1.3 (fake, 2026-10-20)`); `service status --json` prints the file `<its dir>/agent.json` and exits with the code on its first line, or exits 4; when `<its dir>/hang` exists it sleeps 60 s; `run` binds both addresses and answers HTTP 403 unless `Host` is `127.0.0.1:*` or `localhost:*`, else 200.

- [ ] **Step 1: Write the failing tests**: `probe_reads_agent` (fake agent JSON on two free ports → `Some`, ports match); `probe_without_agent` (exit 4 → `None`); `old_emailulator_has_no_agent` (version `0.1.2` → `None`, and `service status` was not run: the fake logs its argv to `<dir>/calls`); `status_timeout_keeps_last_state` (with `hang`, `probe` returns within 6 s with `Err`, and `State` keeps the earlier agent).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test emailulator` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the suite three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `ask emailulator about its LaunchAgent, with a timeout`

### Task 5: Reconcile points Yard at the owner (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/reconcile.rs`, `portscan.rs` (the scan probes and reconciles when the owner or its ports changed), `tls.rs` (`issue_at(cert, key, host, now, days)`, which `issue` calls), `state.rs`
- Test: `crates/yard-daemon/tests/emailulator.rs`

**Interfaces:**
- Consumes: Tasks 2–4; **(M4b)** the services config and its `path` field.
- Produces, in reconcile, after sites resolve: `locate` (a failure means no emailulator, not a reconcile error), `probe`, `owner`. Then:
  - `Owner::Agent(a)`: `TomlFile::edit(ports.toml)` sets `emailulator-smtp`/`-http` to `external` on the agent's ports.
  - `Owner::Yard`: entries left to the unit (Task 7).
  - `Owner::Nobody`: both entries removed.
  - `mail.<tld>` rendered when the owner is not `Nobody` and no resolved site is named `mail`; `nginx/services` joins the managed dirs; its cert issued with `issue_at` when `secure_by_default`.
- The scan calls `probe` and triggers a reconcile when `(owner kind, smtp, http, tls)` differs from `State.last_probe`.

- [ ] **Step 1: Write the failing tests**: `agent_ports_become_external_entries`; `mail_host_follows_the_agent` (`nginx/services/emailulator.conf` names the agent's http port); `no_emailulator_no_mail_host` (no binary → no file, no entries, other sites served); `site_named_mail_wins` (a linked `mail` site → its own config, no services file); `agent_port_change_is_followed` (rewrite `agent.json` with new ports → within two scan intervals of `support::fast()`, the entries and the services file show them); `secure_by_default_issues_mail_cert`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test emailulator` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the suite three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `point the registry and mail.<tld> at whoever serves emailulator`

### Task 6: `.yard` asks for emailulator (yard-core, yard-daemon, yard-cli)

**Files:**
- Modify **(M4b)**: the service needs function (`project::needs` or its M4b successor), `crates/yard-daemon/src/project.rs` (`fill_needs`), `crates/yard-cli/src/commands/init.rs` (`settle`), `crates/yard-core/src/project.rs` (`render`'s commented `[services]` block)
- Test: `crates/yard-daemon/tests/project.rs`, `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1, 2, 5; **(M4b)** `ProjectFile.services`, `Need::Service`, `settle`.
- Produces: for `emailulator = true`, the need is: none when the owner is not `Nobody`; `Need::Manual { what: "emailulator", command: INSTALLER }` when `locate` fails; else `Need::Service { service: "emailulator", version: None }`.
- `settle`: lists every need; after the yes it runs only installable ones; when a `Manual` need remains it prints `Still needed: <what>  (<command>)` and exits 1 with `NEEDS_INSTALL`, hint `<command>`.
- `yard init`'s commented block gains `# emailulator = true`.

- [ ] **Step 1: Write the failing tests**: daemon `emailulator_need_when_missing` (no binary → `Manual`, setup page names `install.sh`); `agent_meets_emailulator` (agent JSON → no need, site served); `stopped_owner_still_meets` (agent loaded, not answering → no need); CLI `settle_yes_leaves_manual_need` (needs `[Php 7.3, Manual]`, `--yes` → one `php.install`, then exit 1 with `NEEDS_INSTALL` and the installer as hint); core `init_comments_emailulator`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test project && cargo test -p yard-cli` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `emailulator = true in .yard, met by any emailulator Yard can reach`

### Task 7 (option A only, not built: B was chosen): Yard's emailulator unit

**Files:**
- Modify **(M4b)**: `crates/yard-daemon/src/rpc/services.rs`, the service-unit builder in `reconcile.rs`, `crates/yard-daemon/src/supervisor/mod.rs` (`UnitSpec.port` → `ports: Vec<(u16, Proto)>`, if M4b did not)
- Test: `crates/yard-daemon/tests/emailulator.rs`

**Interfaces:**
- Consumes: Tasks 2, 4, 5; **(M4b)** `Registry::ensure_usual`, the services config, `services.add|remove|start|stop|restart|port|logs`.
- Produces:
  - `services.add { engine: "emailulator", version, port, path }`: `version` set → `INVALID_ARGUMENT`, hint `emailulator self-update`; `locate(path)` must succeed; records `[services.emailulator] engine = "emailulator", path = "<abs>"`; `ensure_usual(SMTP_UNIT, port.unwrap_or(1025))`, `ensure_usual(HTTP_UNIT, 8025)`.
  - Unit `emailulator` when the owner is `Yard` and the user has not stopped it: program the located binary, args `run_args`, both ports probed before start, health TCP on SMTP, log `logs/emailulator.log`. A binary gone at start fails with `EMAILULATOR_NOT_FOUND` straight to `failed`, no backoff.
  - With `Owner::Agent`: no unit (a running one is stopped); `start|stop|restart|port` → `SERVICE_EXTERNAL`, hint `emailulator service uninstall`, `data { label, pid }`; `logs` reads the agent's `log`.
  - `remove`: stops the unit, drops the entry and both registry entries, never touches the store; `keep_data` changes nothing and the result says so.

- [ ] **Step 1: Write the failing tests**: `add_runs_the_unit_on_usual_ports` (with 1025/8025 replaced by free ports through a test hook on the usual numbers); `no_unit_while_agent_is_loaded` (agent JSON with `loaded: true, running: false` → no unit after add); `agent_appearing_stops_the_unit` (within two scan intervals); `agent_going_away_starts_the_unit`; `user_stopped_stays_stopped_after_agent_goes`; `version_is_refused`; `start_with_agent_is_service_external`; `missing_binary_fails_without_backoff`; `remove_keeps_the_store` (the fake's `run` writes `<home>/Library/Application Support/emailulator/store-marker`; after `remove` with and without `keep_data` the file is still there).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test emailulator` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the suite three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `run emailulator as a Yard unit unless its LaunchAgent is loaded`

### Task 8 (option B only): The agent is the only server

**Files:**
- Modify **(M4b)**: `crates/yard-daemon/src/rpc/services.rs`, `crates/yard-daemon/src/jobs.rs`
- Test: `crates/yard-daemon/tests/emailulator.rs`

**Interfaces:**
- Consumes: Tasks 2, 4, 5; **(M4b)** `services.*` and the job runner.
- Produces:
  - `services.add { engine: "emailulator", version, port, path }`: `version` set → `INVALID_ARGUMENT`; records `path`; with an agent loaded, returns at once with the agent's `ServiceInfo`; otherwise a job that runs `<bin> service install` (with `-- --smtp-addr 127.0.0.1:<port>` when `port` is set), then probes and reconciles.
  - `start|stop|restart|remove|port` → `SERVICE_EXTERNAL`, hint `emailulator service status` (`remove`: `emailulator service uninstall`).
  - `logs` reads the last lines of the agent's `log`, read-only.

- [ ] **Step 1: Write the failing tests**: `add_installs_the_agent` (the fake's `calls` shows `service install`, then `agent.json` written by the fake → `managed_by == LaunchAgent`); `add_with_port_passes_addresses`; `add_with_agent_runs_nothing`; `every_lifecycle_command_is_service_external`; `logs_read_the_agent_log`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test emailulator` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the suite three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `emailulator stays its own LaunchAgent; Yard points at it`

### Task 9: `yard env` prints `MAIL_*` (yard-daemon)

**Files:**
- Modify **(M4b)**: the `env.print` handler
- Test: `crates/yard-daemon/tests/emailulator.rs`

**Interfaces:**
- Consumes: Task 2's `env_lines`; **(M4b)** `env.print { path? }` and its result type.
- Produces: an `emailulator` group of the seven lines, with `MAIL_HOST` the owner's SMTP host and `MAIL_PORT` its port, included whenever M4b includes a service's lines; `warning: emailulator is not answering; see yard doctor` in the result's warnings when the owner does not answer.

- [ ] **Step 1: Write the failing tests**: `env_prints_mail_lines_for_the_agent` (agent on a free port → exactly the seven lines with that port); `env_warns_when_not_answering`; `env_has_no_mail_lines_without_emailulator`.
- [ ] **Step 2: Run** — Expected: FAIL. **Step 3: Implement.** **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `yard env prints emailulator's MAIL lines`

### Task 10: Doctor (yard-daemon)

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs`
- Test: `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Consumes: Tasks 2, 4, 5.
- Produces the checks in spec §11, with these ids and texts:
  - `emailulator` `ok`: `mail is caught by <Yard's unit | LaunchAgent <label> (pid N)>: SMTP 127.0.0.1:<p>, web UI <url>`. `fail`: `emailulator is not answering`, hint `yard service start emailulator` or `emailulator service status`; or `EMAILULATOR_NOT_FOUND`'s message and hint. Present only when an instance exists, an agent is installed, or a `.yard` lists it.
  - `emailulator-version` `warn`: `emailulator <v> is older than 0.1.3`, hint `emailulator self-update`.
  - `emailulator-status` `warn`: `emailulator service status failed: <reason>`.
  - `mail-host` `warn`: `the site mail is served at mail.<tld>, so emailulator's web UI is not`, hint `yard unlink mail`.
- The offline `yard doctor` runs the same probe.

- [ ] **Step 1: Write the failing tests**: `doctor_names_the_agent`; `doctor_fails_unanswering_emailulator`; `doctor_fails_missing_binary_when_yard_lists_it`; `doctor_silent_without_emailulator`; `doctor_warns_old_version`; `doctor_warns_site_named_mail`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test doctor` — Expected: FAIL. **Step 3: Implement.** **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `doctor says who catches mail`

### Task 11: CLI (yard-cli)

**Files:**
- Modify **(M4b)**: `crates/yard-cli/src/commands/services.rs`, `commands/env.rs`, `cli.rs`
- Test: `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1, 7 or 8, 9; **(M4b)** the `service` and `env` commands, `MockDaemon`.
- Produces: `yard service add emailulator` resolves `emailulator` on the CLI's PATH (`which`-style search of `$PATH`, executable regular files only) and sends it as `path`; not found → sends none (yardd falls back). `yard services` shows `MANAGED BY` (`yard` / `LaunchAgent <label>`) and the web URL. `yard env` prints warnings on stderr. `--help` for `service add` names emailulator and says Yard never installs it.

- [ ] **Step 1: Write the failing tests**: `service_add_sends_resolved_path` (a fake `emailulator` on a temp `PATH`); `services_shows_launch_agent`; `service_stop_external_prints_hint` (exit 1, `hint: emailulator service …`); `env_warning_on_stderr`; `service_add_help_mentions_emailulator`.
- [ ] **Step 2: Run** `cargo test -p yard-cli` — Expected: FAIL. **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` — Expected: all PASS.
- [ ] **Step 5: Commit** `yard services and yard env show emailulator`

### Task 12: Docs and the real-machine check

**Files:**
- Create: `scripts/e2e-m4d.sh`
- Modify: main spec §3.3, §6.3, §7 (the emailulator example entries), §8.2, §10, §12 (§1 and §3.1 were reworded for B when it was chosen); the `.yard` spec §4; the emailulator spec's status line; `README.md`; `AGENTS.md` (status, e2e list); `.hydra/rules/port-registry.md` (external entries follow the agent), `.hydra/rules/cli-output.md` (`Manual` needs and `SERVICE_EXTERNAL`).

**Interfaces:**
- `scripts/e2e-m4d.sh`, after `e2e-m1.sh`, against the real data root, read-only towards emailulator: it never installs, uninstalls, starts or stops the agent.
  1. `yard doctor --json` has `emailulator` `ok` naming the agent's label.
  2. `yard ports --json` has `emailulator-smtp` `external` on the agent's SMTP port.
  3. `curl -s -o /dev/null -w '%{http_code}' http://mail.yard:8080/` is `200`, and `curl` to the agent's HTTP port with `Host: mail.yard:8080` is `403` (proving the rewrite is what makes it work).
  4. A scratch project with `[services] emailulator = true` in its `.yard`: `yard link` serves it (no setup page), and `yard env` in it prints `MAIL_PORT=<agent's port>`.
  5. Unlink and remove the scratch dir and its override; `sites.toml` restored with a `trap`.
  - Under A it also needs the agent uninstalled to test Yard's unit; that is the maintainer's step, so the script only checks the agent path.

- [ ] **Step 1: Docs** as listed; `hydra sync && hydra doctor` — Expected: PASS.
- [ ] **Step 2: Grep the docs** for `emailulator`, `mailpit`, `mail.<tld>`, `MAIL_`, `SERVICE_EXTERNAL`, `PROTOCOL` and fix what no longer matches.
- [ ] **Step 3: Write** `scripts/e2e-m4d.sh`.
- [ ] **Step 4: Ask the maintainer**, then `cargo build --release`, `yard stop && yard start`, `scripts/e2e-m1.sh && scripts/e2e-m4d.sh` — Expected: both pass; `yard --version` shows the new commit.
- [ ] **Step 5: Commit** `M4d docs and the e2e check for emailulator`
