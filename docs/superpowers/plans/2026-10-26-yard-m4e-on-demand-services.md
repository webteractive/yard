# Yard M4e (on-demand services) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Approved by the maintainer on 2026-10-26**, to run natively (superpowers:executing-plans) on main.
> **Order:** M4e follows M4d in the milestone list (spec §13.4), but the maintainer approved both plans on
> the same day and asked for M4e to be built now. Nothing here depends on M4d: emailulator keeps its own
> LaunchAgent (M4d option B), so it is never a Yard unit and never sleeps.

**Goal:** A service instance that opts in stops gracefully after `sleep_after` with no use, and
wakes on the next PHP request to a site that needs it, on `php`/`composer` through the shims in
that site, or on `yard service start|restart`.

**Architecture:** Sleep is runtime state in a `Sleeper` that yardd shares between the reconcile
(which leaves a sleeping instance out of the desired units), an idle loop (which samples client
connections with `lsof` and puts idle instances to sleep), and a second socket, `run/wake.sock`,
that answers `GET /wake/<site>?wait=<s>` with `204` once the site's instances are up. nginx asks
it through `auth_request` before each PHP request of a site whose `.yard` lists services, and
fails open; the shims ask it when `run/sleeping` lists their site. On the wire a sleeping
instance is `stopped` with `sleeping_since`, protocol 2.8.

**Tech Stack:** Rust (tokio, serde, toml_edit, insta), brew's nginx `auth_request`, `/usr/sbin/lsof`, libproc's `proc_listchildpids`.

**Spec:** `docs/superpowers/specs/2026-10-26-yard-m4e-on-demand-services-design.md` (approved
2026-10-26). Read it first; this plan argues from it.

## Global Constraints

- Sleeping is off unless set: `service_sleep_after` in `yard.toml`, `sleep_after` on an instance in `services.toml`; the instance's key wins (spec §4).
- Durations: `s`, `m`, `h` suffixes, minimum `60s`, or `"off"`; anything else is `CONFIG_INVALID` with file, line and column. `yard service sleep-after` also takes `default`, which removes the key.
- Never wake an instance the user stopped; never write `stopped` when sleeping (spec §2, §1.1 anti-goals).
- Falling asleep is the unit's own graceful stop with `STOP_TIMEOUT` (30 s), never SIGKILL first.
- A sleeping instance keeps its pinned port in `ports.toml`; nothing in `ports.toml` changes (spec §8).
- Every instance starts awake when yardd starts; `run/sleeping` is emptied at yardd start (spec §6.4).
- nginx's ask: `wait=10`, `proxy_read_timeout 15s`. A shim's ask: `wait=30`. yardd caps `wait` at 30 (spec §6.3). `wait` is real seconds.
- `wake.sock` is `<root>/run/wake.sock`, mode 0600, one route, and **always answers `204`** (malformed, unknown site, timeout, failed wake included). It only wakes instances in `services.toml` that are not `stopped`.
- No database byte passes through yardd.
- Protocol `2.8`. `ServiceState` keeps its variants; `ServiceInfo.sleeping_since` (RFC 3339) is set with `Stopped` for a sleeping instance and with `Starting` for a waking one.
- Config files are edited through `TomlFile` only (config-ownership rule). New error codes: none.
- Tests never bind 3306, 5432, 6379, 9000, 9001, 80, 443, 8080, 8443, 53 or 53535, and never run real engines; they use `yard-fake-unit` (rust-workspace rule).
- Commit messages: lowercase prose subject, no type prefix, prose body, no trailers (commit-messages rule). Ask the maintainer before every commit unless he has approved the run.

### Checked while planning (2026-10-26, brew nginx 1.31.7; the spec is amended in Task 10)

- nginx refuses `proxy_pass http://unix:"<path>":/wake/<site>` ("invalid number of arguments"); the whole argument is quoted instead, as `fastcgi_pass` is: `proxy_pass "http://unix:<root>/run/wake.sock:/wake/<site>?wait=10";`. A space in the path works.
- `error_page` alone does not fail open on a `500` the socket itself sends: nginx answers the page with `500`. With `proxy_intercept_errors on;` it does. A missing socket (502) and a timeout (504) fail open either way. A `return` in the PHP location runs before `auth_request` (rewrite phase), so tests must use a content-phase handler.
- libc 0.2.191 has `proc_listchildpids` and `PROX_FDTYPE_SOCKET` but not `socket_fdinfo`, so connections are counted with `/usr/sbin/lsof` (spec §7's fallback). `lsof -F n` names an established TCP socket `127.0.0.1:A->127.0.0.1:B`; an accepted unix socket shows its listener's path, exactly as the listener does.

## Review Focus

1. **`run/sleeping` left by a yardd that was killed** lists sites whose instances now start awake; yardd empties it at start, so shims do not ask needlessly (Task 6, `startup_empties_run_sleeping`).
2. **A wake for an instance the user stopped** in the meantime starts nothing and still answers `204` (Task 6, `a_stopped_instance_is_never_woken`).
3. **A port taken while the instance slept** (DBngin started on it): the wake answers `204` at once rather than after `wait`, and the instance is `failed` with `PORT_IN_USE` naming the holder (Task 6, `a_held_port_answers_at_once_and_fails`).
4. **A slow or garbage client on `wake.sock`** never stalls yardd or other asks: a head that does not arrive in 2 s, or is over 8 KiB, is answered `204` and closed (Task 6, `a_silent_client_is_answered_and_closed`).
5. **`yard service stop` or `remove` of a sleeping instance** clears its sleep state, so a later start does not report `waking`, and `run/sleeping` no longer lists its sites (Task 6, `stop_and_remove_clear_sleep`).

---

### Task 1: Sleep settings in `yard.toml` and `services.toml` (yard-core)

**Files:**
- Create: `crates/yard-core/src/services/sleep.rs`
- Modify: `crates/yard-core/src/services/mod.rs` (declare `pub mod sleep`), `crates/yard-core/src/config/yard.rs`, `crates/yard-core/src/config/services.rs`, `crates/yard-core/src/paths.rs`, `crates/yard-proto/src/types.rs` (`SettingSource`)
- Test: in-file `#[cfg(test)]` modules of those files

**Interfaces:**
- Produces:
  - `pub enum SleepAfter { Off, After(u64) }` (seconds), `Debug, Clone, Copy, PartialEq, Eq, Deserialize` with `#[serde(try_from = "String")]`; `impl FromStr for SleepAfter { type Err = String }`; `impl TryFrom<String>`; `impl Display` writing the largest whole unit (`900` → `15m`, `3600` → `1h`, `90` → `90s`, `Off` → `off`).
  - `pub const MIN_SLEEP_SECS: u64 = 60;`
  - `SettingSource::YardToml` and `SettingSource::ServicesToml` (snake_case `yard_toml`, `services_toml`), and `impl Default for SettingSource` → `Default`.
  - `pub fn effective_sleep(instance: Option<SleepAfter>, machine: Option<SleepAfter>) -> (Option<u64>, SettingSource)`: instance key → `ServicesToml`, else machine key → `YardToml`, else `(None, Default)`; `Off` gives `None` with its source.
  - `pub fn sleeping_text(sites: &BTreeSet<String>) -> String` (one name per line, newline-terminated) and `pub fn lists_site(text: &str, site: &str) -> bool` (exact line match, trimmed).
  - `YardConfig.service_sleep_after: Option<SleepAfter>` (raw key `service_sleep_after`).
  - `ServiceEntry.sleep_after: Option<SleepAfter>` (raw key `sleep_after`).
  - `Paths::wake_socket() -> PathBuf` = `run/wake.sock`; `Paths::sleeping_file() -> PathBuf` = `run/sleeping`.

- [ ] **Step 1: Write the failing tests**
  - `sleep_after_parses_units_and_off`: `"90s"` → `After(90)`, `"15m"` → `After(900)`, `"2h"` → `After(7200)`, `"off"` → `Off`; `Display` round-trips `"15m"`, `"1h"`, `"90s"`, `"off"`.
  - `sleep_after_refuses_short_and_bad`: `"59s"`, `"0m"`, `"15"`, `"15 m"`, `"1d"`, `"-5m"`, `""` are `Err`; the message for `"59s"` contains `"at least 60s"`.
  - `effective_sleep_instance_then_machine_then_off`: `(Some(After(3600)), Some(After(900)))` → `(Some(3600), ServicesToml)`; `(None, Some(After(900)))` → `(Some(900), YardToml)`; `(Some(Off), Some(After(900)))` → `(None, ServicesToml)`; `(None, None)` → `(None, Default)`.
  - `sleeping_text_lists_sites`: `{"blog","shop"}` → `"blog\nshop\n"`; `lists_site("blog\nshop\n", "shop")` true, `"sho"` false, `""` false.
  - In `yard-proto`: `setting_source_names`: `YardToml` ↔ `"yard_toml"`, `ServicesToml` ↔ `"services_toml"`.
  - In `config/yard.rs`: `service_sleep_after_parses_and_a_bad_one_is_config_invalid` (`service_sleep_after = "15m"` → `Some(After(900))`; `"5s"` → `ErrorCode::ConfigInvalid` with line 1).
  - In `config/services.rs`: `sleep_after_on_an_instance` (`sleep_after = "off"` → `Some(Off)`; absent → `None`; `"fast"` → `ConfigInvalid` with its line).
- [ ] **Step 2: Run** `cargo test -p yard-core sleep` — Expected: FAIL (types missing).
- [ ] **Step 3: Implement** as the Interfaces say. Every existing `ServiceEntry { .. }` and `YardConfig { .. }` literal gains the new field (`None`).
- [ ] **Step 4: Run** `cargo test -p yard-core` — Expected: PASS.
- [ ] **Step 5: Commit** `read sleep_after from yard.toml and services.toml`

### Task 2: Protocol 2.8 (yard-proto)

**Files:**
- Modify: `crates/yard-proto/src/types.rs`, `crates/yard-proto/src/methods.rs`, `crates/yard-proto/src/lib.rs` (`PROTOCOL_VERSION`)
- Test: in-file tests; fix the `"2.7"` assertions in `crates/yard-cli/tests/cli.rs` and anywhere `grep -rn '"2\.7"' crates` finds them

**Interfaces:**
- Consumes: Task 1's `SettingSource` variants and `Default`.
- Produces:
  - `ServiceInfo` gains, each `#[serde(default, skip_serializing_if = …)]` where it is an `Option`: `sleeping_since: Option<String>`, `sleep_after_secs: Option<u64>`, and `#[serde(default)] sleep_after_source: SettingSource`.
  - `ServiceState` gains `#[serde(other)] Other` ("a state a newer yardd reports; never sent"). If serde_derive refuses `other` on this externally tagged enum, leave it out and ledger the ruling: `sleeping_since` already carries M4e's states.
  - `pub struct SleepingInstance { pub name: String, pub since: String }` and `StatusInfo.sleeping: Vec<SleepingInstance>` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
  - `pub const SERVICES_SLEEP_SET: &str = "services.sleep.set";` and `pub struct ServiceSleepParams { pub name: String, pub after: String }` (`after`: a duration, `off` or `default`).
  - `PROTOCOL_VERSION = "2.8"`.

- [ ] **Step 1: Write the failing tests**
  - `a_2_7_service_info_still_decodes`: the JSON of a 2.7 `ServiceInfo` (no new fields) decodes with `sleeping_since == None`, `sleep_after_secs == None`, `sleep_after_source == SettingSource::Default`.
  - `a_sleeping_instance_is_stopped_with_since`: a `ServiceInfo` with `state: Stopped, sleeping_since: Some("2026-10-26T14:02:00Z")` serializes `"state":"stopped"` and `"sleeping_since":"2026-10-26T14:02:00Z"`.
  - `an_unknown_unit_state_is_other` (only if `Other` compiles): `"hibernating"` decodes to `ServiceState::Other`.
  - In `methods.rs`: `SERVICES_SLEEP_SET == "services.sleep.set"`.
- [ ] **Step 2: Run** `cargo test -p yard-proto` — Expected: FAIL.
- [ ] **Step 3: Implement.** Every `ServiceInfo { .. }` and `StatusInfo { .. }` literal in the workspace gains the new fields; `state_name` in `crates/yard-cli/src/commands/services.rs` maps `Other` to `"unknown"`.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `protocol 2.8: sleeping_since and the sleep setting on ServiceInfo`

### Task 3: A starting unit polls its health fast (yard-daemon supervisor)

**Files:**
- Modify: `crates/yard-daemon/src/supervisor/mod.rs` (`SupervisorConfig`), `crates/yard-daemon/src/supervisor/unit.rs` (`supervise`), `crates/yard-daemon/src/bin/yard-fake-unit.rs` (new mode), `crates/yard-daemon/tests/support/mod.rs` (`fast()`)
- Test: `crates/yard-daemon/tests/supervisor.rs`

**Interfaces:**
- Produces: `SupervisorConfig.start_poll: Duration`, default 200 ms (spec §6.2's 100–250 ms); `fast()` sets 10 ms. Until the first health check passes, `supervise` sleeps `start_poll` between checks instead of `health_interval`; after it, `health_interval` as now.
- Fake: `yard-fake-unit listen-tcp-after <port> <ms>` sleeps `ms`, then listens as `listen-tcp`.

- [ ] **Step 1: Write the failing test** `a_starting_unit_is_running_soon_after_it_listens`: config with `health_interval: 5 s`, `start_poll: 20 ms`, `start_grace: 10 s`; a unit `listen-tcp-after <port> 150`; assert its state is `Running` within 1 s of `apply`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test supervisor a_starting_unit` — Expected: FAIL (field missing, then Running only after ~5 s).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test supervisor` three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `poll a starting unit's health every 200 ms until it first passes`

### Task 4: Counting an instance's client connections (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/conns.rs` (declared in `lib.rs`)
- Modify: `crates/yard-daemon/src/bin/yard-fake-unit.rs` (`serve` holds connections; `postgres` hands each to a child)
- Test: in-file parser tests; `crates/yard-daemon/tests/conns.rs`

**Interfaces:**
- Produces:
  - `pub fn client_connections(pid: u32) -> Result<usize, String>`: the unit's pid and every descendant (`libc::proc_listchildpids`, recursively), then `/usr/sbin/lsof -nP -a -p <pid,…> -iTCP -sTCP:ESTABLISHED -F n` and `/usr/sbin/lsof -nP -a -p <pid,…> -U -F n`, each with the `cloexec_inherited` hook and a 5 s timeout, stdout read whatever the exit status (lsof exits 1 when nothing matched, as `holder.rs` notes). `Err` only when lsof cannot run.
  - `fn count_tcp(out: &str) -> usize`: `n` lines containing `->`.
  - `fn count_unix(out: &str) -> usize`: among `n` lines naming a path (starting with `/`), the count minus the number of distinct paths, since an accepted unix socket shows its listener's path; `->0x…` lines (the process's own client ends) are not counted.
- Fake engines: `serve(listener, false)` keeps each accepted stream in a thread that reads it to EOF, as a server does. `postgres` gives each accepted stream to a child, `yard-fake-unit` run with `arg0` `postgres-backend` and the stream as its stdin, then drops its own copy; `postgres-backend` reads stdin to EOF. This is PostgreSQL's fork-per-connection, which is why the count walks the tree (spec §7).

- [ ] **Step 1: Write the failing tests**
  - `count_tcp_counts_established_lines`: `"p1\nf7\nn127.0.0.1:3307->127.0.0.1:50000\nf8\nn127.0.0.1:3307->127.0.0.1:50001\n"` → 2; `"p1\n"` → 0.
  - `count_unix_skips_listeners_and_client_ends`: `"p1\nf3\nn/r/run/mysql.sock\nf5\nn->0xabc\nf6\nn/r/run/mysql.sock\nf7\nn/r/run/mysql.sock\n"` → 2.
  - In `tests/conns.rs`, `connections_are_counted_across_the_process_tree`: spawn the fake as `postgres` on a free port (`port=<p>`), wait for it to listen, assert `client_connections(pid) == Ok(0)`; open two `TcpStream`s to it, wait up to 2 s until it is `Ok(2)`; drop them, wait up to 2 s until `Ok(0)`.
  - Same file, `a_redis_connection_is_counted`: the fake `redis-server` with a conf naming a free port: one `TcpStream` → `Ok(1)`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon conns` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS (the fakes holding connections must not change any other suite).
- [ ] **Step 5: Commit** `count an instance's client connections across its process tree`

### Task 5: Falling asleep (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/sleep.rs` (declared in `lib.rs`)
- Modify: `crates/yard-daemon/src/daemon.rs` (`DaemonOptions.sleep`, `Context`, spawn the loop), `crates/yard-daemon/src/reconcile.rs` (skip sleeping instances; `with_sleeper`, `sleeper()`), `crates/yard-daemon/src/services.rs` (`service_info`, `needed_by`), `crates/yard-daemon/src/rpc/services.rs`, `crates/yard-daemon/src/rpc/info.rs`, `crates/yard-daemon/src/rpc/handlers.rs` (`status`), `crates/yard-daemon/src/doctor.rs`, `crates/yard-daemon/tests/support/daemon.rs` (test `SleepConfig`)
- Test: `crates/yard-daemon/tests/sleep.rs`

**Interfaces:**
- Consumes: Task 1's `effective_sleep`, `sleeping_text`, `Paths::sleeping_file`; Task 2's fields; Task 4's `client_connections`.
- Produces:
  - `pub struct SleepConfig { pub sample_interval: Duration, pub second: Duration }`: how often the idle loop samples, and how long one second of a `sleep_after` lasts. `SleepConfig::production()` is `{ 30 s, 1 s }`; tests use `{ 50 ms, 1 ms }`, so `"60s"` lasts 60 ms (the fake clock, spec §16). `DaemonOptions.sleep: SleepConfig`.
  - `pub struct Sleeper` (`Debug`, behind `Arc`, one `std::sync::Mutex` inside) with: `seen(&self, i)` (starts the idle clock if it has none), `touch(&self, i)`, `idle_for(&self, i, now: Instant) -> Duration`, `fall_asleep(&self, i, since: OffsetDateTime)`, `wake(&self, i) -> bool` (true when it was asleep; it is then *waking* with the same `since` until `clear_waking`), `is_asleep(&self, i) -> bool`, `asleep(&self) -> BTreeMap<String, OffsetDateTime>`, `waking_since(&self, i) -> Option<OffsetDateTime>`, `clear_waking(&self, i)`, `forget(&self, i)`.
  - `Context.sleeper: Arc<Sleeper>`, also held by `Reconciler` (`with_sleeper`, `sleeper()`).
  - Reconcile's service loop skips an instance with `is_asleep`: no unit, not `left_out`, no warning.
  - `pub fn needed_by(snapshot: &Snapshot, site: &str) -> Vec<String>`: the instances in `services.toml` the site's `.yard` asks for (inverse of `used_by`; `ServiceWant::Default` through `default_instance`).
  - `pub fn write_sleeping(paths: &Paths, snapshot: &Snapshot, sleeper: &Sleeper)`: `run/sleeping` = the linked sites with at least one asleep instance in `needed_by`, written with `yard_core::generated::write_atomic`; a write error is a `tracing::warn`.
  - `service_info(snapshot, supervisor, sleeper, name)`: asleep → `Stopped` + `sleeping_since`; waking and the unit `Starting`/`Backoff` → `Starting` + `sleeping_since`; `sleep_after_secs` and `sleep_after_source` from `effective_sleep(entry.sleep_after, snapshot.yard.service_sleep_after)`. `instance_info` must not turn a sleeping `Stopped` into `Starting`.
  - `status`: `sleeping` lists `sleeper.asleep()`, `since` RFC 3339.
  - Doctor's `ServiceView` gains `sleeping: BTreeSet<String>`: `service:<i>` is `ok` `"<i> is sleeping since <HH:MM>; it wakes on use"`; `service-need:<site>` says nothing about a sleeping instance.
  - `pub fn spawn(ctx: Arc<Context>, cfg: SleepConfig) -> JoinHandle<()>`: every `sample_interval`, for each instance in the snapshot that is not `stopped`, not asleep, has `Some(secs)` from `effective_sleep`, and whose unit is `Running` with a pid: `seen`; `clear_waking`; `client_connections(pid)` on `spawn_blocking` (an `Err` is a warning and counts as a connection, so an unreadable instance stays awake); `> 0` → `touch`; then `idle_for >= secs × cfg.second` → `fall_asleep(now)`. When any fell asleep: reconcile (which stops their units with their graceful signal and 30 s timeout), then `write_sleeping`. An instance whose `effective_sleep` is `None` is never sampled.
  - `daemon::run`: writes an empty `run/sleeping` before the first reconcile (Review Focus 1), spawns the loop after it, aborts it on shutdown.

- [ ] **Step 1: Write the failing tests** in `tests/sleep.rs`, each with a fake `redis` instance (`add_formula("redis")`, `add_service("redis", "redis", false)`) and the test `SleepConfig`:
  - `nothing_sleeps_without_a_setting`: no sleep keys; after 500 ms `services.list` shows `running`, no `sleeping_since`.
  - `an_idle_instance_falls_asleep`: `service_sleep_after = "60s"` in `yard.toml`; within 2 s `services.list` has state `stopped`, `stopped_by_user: false`, `sleeping_since` set, `sleep_after_secs: 60`, `sleep_after_source: "yard_toml"`; the process in `services/redis/redis.pid` is gone (pid file absent); `ports.toml` is byte-for-byte what it was; `services.toml` has no `stopped`.
  - `an_instance_key_off_keeps_it_awake`: `yard.toml` `"60s"`, the instance `sleep_after = "off"`; after 500 ms still `running`, `sleep_after_source: "services_toml"`, no `sleep_after_secs`.
  - `a_held_connection_keeps_it_awake`: `"60s"`; hold a `TcpStream` to the instance's port; after 500 ms still `running`; drop it; within 2 s `sleeping_since` set.
  - `status_and_doctor_report_sleep`: after it falls asleep, `status` has `sleeping: [{name: "redis", since: …}]` and no `redis` unit; `doctor` has `service:redis` `ok` containing `"sleeping"`.
  - `run_sleeping_lists_the_sites_that_need_it`: a linked project whose `.yard` has `[services] redis = true`; after sleep `run/sleeping` is `"<site>\n"`.
  - `startup_empties_run_sleeping` (Review Focus 1): write `run/sleeping` = `"stale\n"` before starting the daemon; after startup it is empty.
  - `instances_start_awake_after_a_restart`: let it fall asleep, shut the daemon down, start a new one on the same root; `services.list` shows `running` without `sleeping_since` (spec §6.4).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test sleep` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `put idle service instances to sleep`

### Task 6: Waking: `wake.sock`, `yard service start|restart`, and `services.sleep.set` (yard-daemon)

**Files:**
- Create: `crates/yard-daemon/src/wake.rs`
- Modify: `crates/yard-daemon/src/sleep.rs`, `crates/yard-daemon/src/daemon.rs`, `crates/yard-daemon/src/rpc/services.rs`, `crates/yard-daemon/src/rpc/handlers.rs`, `crates/yard-daemon/src/services.rs`, `crates/yard-daemon/src/bin/yard-fake-unit.rs` (the engines wait `<prefix>/fake-stop-delay` ms after SIGTERM before exiting, as a database finishing a checkpoint does)
- Test: `crates/yard-daemon/tests/sleep.rs`

**Interfaces:**
- Consumes: Task 5's `Sleeper`, `needed_by`, `write_sleeping`.
- Produces:
  - `pub async fn wake(ctx: &Context, instances: &[String], wait: Duration)`: `touch` each; `wake` each (instances that are `stopped` in the snapshot are skipped, Review Focus 2); when one was asleep: `write_sleeping`, reconcile (a failure is a warning), then poll the supervisor every 20 ms until each woken unit is `Running`, `Failed`, or absent, or `wait` passes. It returns either way; a failed wake is the unit's own `Failed` state (spec §6.1).
  - `wake.rs`: `pub async fn serve(listener: UnixListener, ctx: Arc<Context>)` — one task per connection; reads the head until `\r\n\r\n` within `HEAD_TIMEOUT = 2 s` and `MAX_HEAD = 8192` bytes; `fn parse(head: &str) -> Option<(String, u64)>` takes `GET /wake/<site>?wait=<n> HTTP/1.x` (`wait` absent → 10; capped at `MAX_WAIT_SECS = 30`; a site name `is_site_name` refuses → `None`). A known site wakes `needed_by(site)`; anything else is a `tracing::info` line. Every connection is answered `HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n`.
  - `daemon::run` binds `wake.sock` after `claim` (removing a stale file first), mode 0600, serves it beside `yardd.sock`, and removes it on shutdown. `run/` is created first.
  - `services.start` on an instance that is asleep calls `wake(ctx, &[name], Duration::ZERO)` before its reconcile, and answers as now; the CLI polls the state as it does for a stopped one. `services.restart` on a sleeping instance wakes it the same way instead of `restart_and_wait` (spec §5.3). `services.stop` and `services.remove` call `sleeper.forget` and `write_sleeping` (Review Focus 5).
  - `services.sleep.set { name, after }`: `after` parsed as `SleepAfter` or `default`; an unknown instance is `SERVICE_NOT_FOUND`; a bad `after` is `INVALID_ARGUMENT` with hint `yard service sleep-after <name> 15m`. Writes `sleep_after` through `TomlFile` (removes it for `default`), `touch`es the instance (a new setting restarts its idle clock), reconciles, answers the `ServiceInfo`. Lives in `services::set_sleep_after(paths, instance, after: Option<SleepAfter>)` beside `set_stopped`.

- [ ] **Step 1: Write the failing tests** (a helper `ask(root, site, wait) -> (status_line, elapsed)` speaks HTTP/1.1 over `wake.sock` with `std::os::unix::net::UnixStream`):
  - `an_ask_wakes_the_sites_instance`: asleep `redis`, site needing it; `ask(site, 5)` answers `HTTP/1.1 204` and afterwards `services.list` shows `running` without `sleeping_since`; `run/sleeping` is empty.
  - `concurrent_asks_share_one_start`: two `ask`s at once; both `204`; the instance's log shows one start (count the fake's start line, or the pid file was written once: `restarts == 0` in `status`).
  - `an_ask_counts_as_use`: `"60s"`; `ask` every 20 ms for 300 ms; still `running` at the end.
  - `an_unknown_site_and_garbage_get_204`: `GET /wake/nope`, `GET /`, `"hello\r\n\r\n"` each answer `204`.
  - `a_silent_client_is_answered_and_closed` (Review Focus 4): connect and send nothing; answered `204` within 3 s; a second `ask` meanwhile answers at once.
  - `a_stopped_instance_is_never_woken` (Review Focus 2): asleep, then `services.stop`; `ask` answers `204`; still `stopped_by_user`, no unit.
  - `a_held_port_answers_at_once_and_fails` (Review Focus 3): asleep; bind its port with `held_listener`-style code; `ask(site, 10)` answers within 2 s; state `failed` with reason code `PORT_IN_USE`.
  - `a_wake_during_the_stop_waits_for_it`: `fake-stop-delay` = 500; as soon as `services.list` shows `sleeping_since` (the sleeper is marked before the reconcile stops the unit, so the stop has begun; `run/sleeping` is written only after it ends), `ask(site, 5)`; it answers after the old process is gone (its pid no longer exists when the answer arrives) and the instance is `running` with a new pid. Two processes never run on one data directory (spec §6.2).
  - `service_start_and_restart_wake`: asleep → `services.start` → within 2 s `running`; asleep again → `services.restart` → within 2 s `running`, and `stopped_by_user` stays false.
  - `stop_and_remove_clear_sleep` (Review Focus 5): asleep → `services.stop` → `sleeping_since` absent, `run/sleeping` empty.
  - `sleep_set_writes_and_clears_the_key`: `services.sleep.set {after: "1h"}` → `services.toml` has `sleep_after = "1h"` and the answer has `sleep_after_secs: 3600`; `"default"` removes the key; `"5s"` is `INVALID_ARGUMENT`; `{name: "nope"}` is `SERVICE_NOT_FOUND`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test sleep` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` three times — Expected: PASS each time.
- [ ] **Step 5: Commit** `wake sleeping instances from wake.sock and yard service start`

### Task 7: nginx asks before a PHP request (yard-core render, yard-daemon reconcile)

**Files:**
- Modify: `crates/yard-core/src/render/nginx.rs` (`nginx_site`), `crates/yard-core/src/render/mod.rs` tests, `crates/yard-daemon/src/reconcile.rs` (passes `wake`)
- Test: `crates/yard-core/src/render/mod.rs` (insta), `crates/yard-daemon/tests/real_nginx.rs` (opt-in), `crates/yard-daemon/tests/reconcile.rs`

**Interfaces:**
- Produces: `nginx_site(ctx: &RenderCtx, site: &SiteInfo, wake: bool) -> String`. Reconcile passes `wake = true` for a PHP site that does not `needs_setup()` and whose loaded `.yard` lists at least one service. With `wake`, the PHP location gains `auth_request /__yard_wake;` and the server body gains exactly:

```nginx
    location = /__yard_wake {
        internal;
        proxy_pass "http://unix:<root>/run/wake.sock:/wake/<site>?wait=10";
        proxy_pass_request_body off;
        proxy_set_header Content-Length "";
        proxy_read_timeout 15s;
        proxy_intercept_errors on;
        error_page 400 401 403 404 500 502 503 504 = @yard_wake_skipped;
    }

    location @yard_wake_skipped {
        return 204;
    }
```

  The `proxy_pass` argument is one `nginx_str` of the whole URL (checked above).

- [ ] **Step 1: Write the failing tests**
  - `nginx_site_php_wake` (insta snapshot) of a PHP site with `wake = true`; the existing `nginx_site_php` snapshot is unchanged with `wake = false`.
  - In `tests/reconcile.rs`, `only_sites_with_services_ask`: two linked projects, one whose `.yard` lists `redis = true` (instance added), one with no `.yard`; the first's `nginx/sites/<name>.conf` contains `auth_request /__yard_wake;`, the second's does not.
  - In `tests/real_nginx.rs` (`#[ignore = "needs brew nginx"]`), `the_wake_hook_fails_open`: render a server with the wake block whose PHP location serves a static `index.php` (no `return`, which runs before `auth_request`); with no socket, with a socket answering `500`, and with one that never answers (and `proxy_read_timeout` lowered to 1 s by string replace), `GET /index.php` is `200`; with one answering `204` after 300 ms it is `200` after at least 300 ms. `nginx -t` passes on the rendered file.
- [ ] **Step 2: Run** `cargo test -p yard-core nginx_site` and `cargo test -p yard-daemon --test reconcile only_sites` — Expected: FAIL.
- [ ] **Step 3: Implement.** Review the new snapshot with `cargo insta review` and accept it only if it matches the block above.
- [ ] **Step 4: Run** `cargo test --workspace`, then `cargo test -p yard-daemon --test real_nginx -- --ignored` — Expected: PASS.
- [ ] **Step 5: Commit** `nginx asks yardd to wake a site's services before PHP runs`

### Task 8: The shims wake their site (yard-core shim, yard-shim)

**Files:**
- Modify: `crates/yard-core/src/shim.rs`, `crates/yard-shim/src/main.rs`
- Test: `crates/yard-core/src/shim.rs` tests, `crates/yard-shim/tests/shim.rs`

**Interfaces:**
- Consumes: Task 1's `lists_site`, `Paths::{sleeping_file, wake_socket}`.
- Produces:
  - `pub fn site_at(cwd: &Path, sites: &SitesConfig, canonical: &dyn Fn(&Path) -> PathBuf) -> Option<String>`: the innermost linked site containing `cwd` (the last of `innermost`).
  - `pub const WAKE_SHIMS: [&str; 2] = ["php", "composer"];`
  - In `main.rs`, `fn wake(paths: &Paths, site: &str)`, called by the PHP path for a `WAKE_SHIMS` name just before `exec`, when `site_at` finds a site and `run/sleeping` (read with `read_small`) lists it: prints `yard: waking the services <site> uses …` to stderr, connects to `wake.sock` with `std::os::unix::net::UnixStream`, sets read and write timeouts of 31 s, sends `GET /wake/<site>?wait=30 HTTP/1.1\r\nHost: yard\r\nConnection: close\r\n\r\n`, and reads to EOF. Any error prints `yard: could not wake <site>'s services (<error>); running anyway` and carries on. The spec's `yard: waking mysql-8.4 …` names an instance, but `run/sleeping` lists sites (spec §5.2, code check), so the line names the site.

- [ ] **Step 1: Write the failing tests**
  - `site_at_is_the_innermost_site`: sites `old` at `/code/old` and `api` at `/code/old/api`; `/code/old/api/src` → `Some("api")`; `/code/old/x` → `Some("old")`; `/elsewhere` → `None`.
  - In `tests/shim.rs`, with a thread serving a `UnixListener` at `<root>/run/wake.sock` that records the request line and answers `204`:
    - `php_wakes_a_listed_site`: `run/sleeping` = `"old\n"`; `php -v` in the project records `GET /wake/old?wait=30 HTTP/1.1` and still prints the keg's argv; stderr has `yard: waking the services old uses`.
    - `php_does_not_connect_when_the_site_is_not_listed`: `run/sleeping` = `"other\n"`; nothing recorded; stderr has no `waking`.
    - `a_dead_socket_still_runs_php`: listed, no listener; exit 0, the keg ran, stderr has `could not wake`.
    - `node_never_wakes`: listed; run the shim as `node -v` (no Node is installed, so it exits 1 with `NODE_MISSING`); nothing is recorded and stderr has no `waking`.
- [ ] **Step 2: Run** `cargo test -p yard-core site_at` and `cargo test -p yard-shim` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `php and composer wake their site's sleeping services`

### Task 9: CLI (yard-cli)

**Files:**
- Modify: `crates/yard-cli/src/commands/services.rs`, `crates/yard-cli/src/commands/info.rs`, `crates/yard-cli/src/commands/units.rs`, `crates/yard-cli/src/cli.rs`, `crates/yard-cli/src/commands/mod.rs`
- Test: `crates/yard-cli/tests/cli.rs` (`MockDaemon`)

**Interfaces:**
- Consumes: Task 2's types and method.
- Produces:
  - `pub fn state_text(info: &ServiceInfo, now: OffsetDateTime) -> String`: `sleeping <age>` for `Stopped` with `sleeping_since` (age `45s`, `12m`, `2h`, `3d`, the largest whole unit), `waking` for `Starting` with it, else `state_name`.
  - `yard services` gains a `SLEEP` column after `STATE`: the effective setting as `Display`s it (`15m`, `1h`) or `off`. `yard info`'s service rows use `state_text`, and when any instance sleeps the line `wakes on the next request, or: yard service start <i>` follows the table (first sleeping instance's name).
  - `yard status` adds a row per `sleeping` entry: `UNIT <name>`, `STATE sleeping`, `PID -`, `RESTARTS -`.
  - `yard service sleep-after <instance> <duration|off|default>` calls `services.sleep.set` and prints `<i> now sleeps after <d> without use` / `<i> never sleeps` / `<i> now follows yard.toml (<d or off>)`, with `--json` the `ServiceInfo`.
  - `--help` of `services`, `service start` and `service sleep-after` says: "Instances can sleep when idle (service_sleep_after in yard.toml, or yard service sleep-after); a sleeping one wakes on the next PHP request, php or composer in a site that uses it, or yard service start. GUI clients cannot wake one."
- [ ] **Step 1: Write the failing tests**
  - `services_shows_sleeping_and_sleep_column`: mock `services.list` with one sleeping (`sleeping_since` 2 h before now, `sleep_after_secs: 900`) and one running with `sleep_after_secs: null`; output has `sleeping 2h`, `15m` and `off`.
  - `status_lists_sleeping_instances`: mock `status` with `sleeping: [{name: "mysql-8.4", since: …}]`; a row `mysql-8.4  sleeping`.
  - `info_says_how_a_sleeping_instance_wakes`.
  - `sleep_after_sends_and_prints`: `yard service sleep-after redis 1h` sends `{name: "redis", after: "1h"}` and prints `redis now sleeps after 1h without use`; `off` and `default` print theirs.
  - `service_help_mentions_sleep`.
- [ ] **Step 2: Run** `cargo test -p yard-cli` — Expected: FAIL.
- [ ] **Step 3: Implement** within the cli-output rule (human text, `--json`, the `for_agents` line).
- [ ] **Step 4: Run** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` — Expected: all PASS.
- [ ] **Step 5: Commit** `show sleeping services and add yard service sleep-after`

### Task 10: Docs and the real-machine check

**Files:**
- Create: `scripts/e2e-m4e.sh`
- Modify: the M4e spec (§5.1's block as in Task 7 with "checked 2026-10-26"; §5.2's stderr line names the site; §7 counts with lsof, since libc has no `socket_fdinfo`, and a health check a sample happens to catch keeps the instance awake one more sample, which is accepted instead of skipping samples; §16 the fake clock is `SleepConfig.second`), the main spec (§3.1 `wake.sock` as a second socket, §4.5 shims that ask the daemon, §6.1 `start_poll`, §6.2 the method list with `services.sleep.set`, §12 M4e built), the M4b spec where M4e §10 says, `README.md`, `AGENTS.md` (status; the layout line saying the shim needs no daemon; the `e2e-m4e.sh` entry under Build & test), `.hydra/rules/config-ownership.md` (`service_sleep_after`, `sleep_after`, `run/sleeping`), `.hydra/rules/rpc-protocol.md` (2.8, `sleeping_since`, `services.sleep.set`, `wake.sock` is not JSON-RPC), `.hydra/rules/port-registry.md` (a sleeping instance keeps its pinned port), `.hydra/rules/cli-output.md` (the `SLEEP` column, `sleeping <age>`, `sleep-after`'s lines).

**Interfaces:**
- `scripts/e2e-m4e.sh`, after `e2e-m1.sh`, against the real data root, with yardd built from this branch. It adds nothing to brew. It stops before changing anything if `redis` already exists in `services.toml` (it then asks the maintainer), otherwise:
  1. Back up `yard.toml`, `services.toml`, `ports.toml`, `sites.toml`; restore with `cp -p` and `cmp` in a `trap`.
  2. `yard service add redis --yes` (redis is installed from M4b), `yard service sleep-after redis 60s`.
  3. Link a scratch project under `$TMPDIR` whose `.yard` has `[services] redis = true` and an `index.php` that opens a TCP connection to `127.0.0.1:<redis port>` and prints `PONG` from a `PING`.
  4. Wait up to 120 s for `yard services --json` to show `sleeping_since`, and check no process holds the port (`lsof -nP -iTCP:<port> -sTCP:LISTEN` is empty).
  5. `curl -s http://<site>.yard:8080/` prints `PONG` (the wake), and `yard services --json` shows `running`.
  6. Wait for sleep again, then `"$HOME/.yard/bin/php" -r 'echo 1;'` in the project prints `1` and stderr has `waking`; redis is `running` after.
  7. Hold `redis-cli -p <port>` open (by absolute keg path) for 90 s in the background; redis stays `running`; kill it.
  8. `yard unlink`, `yard service remove redis --yes`, restore the four files, and fail if the listeners on 3306, 5432, 6379 or Herd's resolver changed (as `e2e-m4b.sh` does). `shellcheck` clean.
- [ ] **Step 1: Docs** as listed; `hydra sync && hydra doctor` — Expected: PASS.
- [ ] **Step 2: Grep the docs** for `sleep`, `wake`, `2.7`, `services.sleep.set`, `need no daemon`, `four plans`, and fix what no longer matches.
- [ ] **Step 3: Write** `scripts/e2e-m4e.sh`; `shellcheck scripts/e2e-m4e.sh` — Expected: clean.
- [ ] **Step 4: Ask the maintainer**, then `cargo build --release`, `./target/release/yard stop && ./target/release/yard start`, `scripts/e2e-m1.sh && scripts/e2e-m4e.sh` — Expected: both pass; `yard --version` shows the new commit.
- [ ] **Step 5: Commit** `document on-demand services and add their real-machine check`
