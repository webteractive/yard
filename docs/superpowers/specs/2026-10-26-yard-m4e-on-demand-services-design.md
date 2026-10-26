# Yard M4e: on-demand services (sleep when idle, wake on use) — design

Status: **draft, not approved.** Written 2026-10-26 in a separate session (the maintainer's homedir
session), from the yard session's account of the code at main `2472f47` (M4c in progress),
then checked against the code at `c4e6eef` (M4c built and verified) the same day and
corrected where it disagreed; corrections the code forced are marked **(code check)**.
Needs the maintainer's review before a plan is written; nothing here is built. Part of M4 (main spec
§12). It builds on the M4b service model
(`docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md`) and changes no M4b
decision except where §10 says so.

Decisions marked **(maintainer)** are the maintainer's, from the 2026-10-26 conversation. Decisions marked
**(builder's call)** were made while writing this and can be reversed on their own. §13
lists what is still open for the maintainer.

## 1. Intent

the maintainer's Mac has 8 GB of RAM and swaps hard (7.7 GB of 9.2 GB swap in use on 2026-10-26).
Herd keeps a PHP-FPM master per version resident all day, and DBngin's mysqld alone sits at
about 800 MB. Yard replaces both, but as built after M4b it would cost the same: every
service instance starts when yardd starts, at login, and runs until the user stops it.

The maintainer wants resources to run only while something uses them **(maintainer)**:

- A service starts when something needs it, stops gracefully once nothing has used it for
  a while, and starts again on the next use.
- The first web request after a quiet spell pays for the boot (a cold start). That cost is
  accepted in exchange for running only what is used **(maintainer)**.

PHP mostly does this already (§3). The databases are where the memory is, so they are what
M4e changes.

### 1.1 Goals, non-goals, anti-goals

Goals:

1. A service instance that nothing has used for `sleep_after` (default 15 minutes, §4) is
   stopped gracefully, the way `yard service stop` stops it, and shows as `sleeping`.
2. A request to a linked site whose `.yard` lists a sleeping instance wakes it, holds the
   request until the instance passes its health check, and then serves it. MySQL wakes in
   about 1–3 s, PostgreSQL in under 1 s, and Redis and MinIO in well under 1 s. That needs a
   faster health poll while a unit starts (§6.2): as built, the first check passes no sooner
   than 5 s after spawn unless the process is ready at once.
3. `php` (and so `php artisan`) and `composer`, run through Yard's shims in a linked site,
   wake that site's sleeping instances before they run. There is no `artisan` shim.
4. `yard service start <i>` wakes a sleeping instance. That covers a GUI client (TablePlus)
   or a `mysql` by keg path, which Yard cannot see coming.
5. An instance with open client connections never sleeps, whatever the time since the last
   request (a queue worker, Horizon, `tinker`, a GUI client left connected).

Non-goals:

- Sleeping PHP-FPM masters, nginx or dnsmasq (§3).
- Waking on a raw TCP connection to the instance's port (that is §12's proxy design).
- Stopping or touching Herd, DBngin or Postgres.app.
- Predictive pre-warming (waking at login, at `cd` into a project, on a schedule).

Anti-goals (a failure even if it works):

- Waking an instance the user stopped, or sleeping one and leaving `stopped` behind.
- Killing an engine mid-write: sleep is the unit's graceful stop with its 30 s stop timeout
  (M4b §15), never SIGKILL first.
- A site that answers 5xx because yardd is down or slow. The wake hook fails open (§5.1).
- Any database byte passing through yardd.
- An instance's port moving because it slept.

## 2. Decisions

- **Sleep is a third state, not `stopped` (builder's call).** `stopped = true` is the user's
  word (M4b §4) and Yard never overrides it. A *sleeping* instance is one Yard stopped
  because it was idle; Yard may wake it at any time. The two never mix: waking never starts
  a stopped instance, and sleeping never writes `stopped`.
- **Sleep is runtime state, never written to `services.toml` (builder's call).** It does not
  survive a yardd restart as such; §6.4 says what a fresh yardd does instead.
- **No byte of database traffic goes through yardd (builder's call).** The yard session ruled
  out a yardd-owned port that splices bytes to the engine (§12): it fights the port
  registry's "the engine owns its pinned port" model, touches probes, holder checks, doctor
  and `yard env`, and makes yardd part of the data path, so a yardd crash would drop every DB
  connection. M4e wakes instances *beside* the traffic instead: from nginx before a PHP
  request (§5.1), and from the shims before a CLI command (§5.2).
- **Engines keep their pinned ports.** A sleeping instance's port is free on the machine but
  stays registered to it. Nothing in `ports.toml` changes (§8).
- **PHP-FPM is out of scope (builder's call).** §3.
- **Herd's FPM and DBngin's MySQL are out of scope.** Yard never stops or reconfigures them
  (AGENTS.md goal 1, herd-coexistence rule). Their memory goes when the maintainer retires them.

## 3. What already scales to zero

From the code at main `c4e6eef`:

- **PHP-FPM:** one pool per PHP version, shared by every site on it, started only for
  versions a linked site uses (`php_versions(&resolution.sites)`), and each pool is
  `pm = ondemand`, `pm.process_idle_timeout = 30s`, `pm.max_children = 10` (main spec §4.2).
  Workers already exit when idle; only the master (a few MB) stays resident. Stopping the
  master too would need nginx not to answer 502 while the socket is gone (fd handoff through
  `FPM_SOCKETS`, as `fdpass.rs` does for 80/443, or an `error_page 502` retry), for a saving
  of a few MB per version. Not worth it in M4e; listed in §12.
- **nginx, dnsmasq:** small and always needed; they stay up.
- **Services (MySQL, PostgreSQL, Redis, MinIO):** always running unless `stopped`. **This is
  what M4e changes.**

## 4. Configuration

A machine-wide default in `yard.toml`, and a per-instance override in `services.toml`, both
edited through `TomlFile` (config-ownership rule) **(builder's call)**:

```toml
# yard.toml
service_sleep_after = "15m"     # "off" disables sleeping for every instance
```

```toml
# services.toml
[services."mysql-8.4"]
engine = "mysql"
version = "8.4"
formula = "mysql@8.4"
sleep_after = "off"     # this instance never sleeps; or a duration, e.g. "1h"
```

- **A flat key in `yard.toml`, not a `[services]` table (code check).** In `.yard`, `[services]`
  maps an engine to a line; the same name meaning something else in `yard.toml` invites
  mistakes. `yard.toml` denies unknown fields, so a yardd from before M4e refuses the file
  with `CONFIG_INVALID` once the key is set, as it would for `default_node`.
- Durations take `s`, `m`, `h` (`"90s"`, `"15m"`, `"2h"`), minimum `60s`; `"off"` disables.
  Anything else is `CONFIG_INVALID` with file, line and column, like the other TOMLs.
- **Default `15m`, on for every engine (builder's call; open for the maintainer, §13).** Fifteen minutes
  outlasts a coffee break but frees memory within a meeting. Redis costs only a few MB, but it
  is on too so the behaviour is the same for every engine; `sleep_after = "off"` per instance
  covers a machine-wide Redis someone wants always up.
- `yard service sleep-after <instance> <duration|off|default>` edits the instance key
  (`default` removes it) **(builder's call on the name)**.

## 5. Waking

A wake is idempotent and per instance. Concurrent wakes for one instance share one start: the
first starts the unit, the rest wait on the same health check (§6).

### 5.1 From the web: nginx asks yardd before a PHP request

Every PHP site's `location ~ \.php$` block (`render/nginx.rs`) gains an `auth_request` to an
internal location that asks yardd whether the site's instances are awake. Brew's nginx is
built `--with-http_auth_request_module` (checked 2026-10-26):

```nginx
location ~ \.php$ {
    auth_request /__yard_wake;
    # ... unchanged fastcgi block ...
}

location = /__yard_wake {
    internal;
    proxy_pass http://unix:"<root>/run/wake.sock":/wake/<site>;
    proxy_pass_request_body off;
    proxy_set_header Content-Length "";
    proxy_read_timeout 35s;
    error_page 500 502 503 504 = @yard_wake_skipped;
}

location @yard_wake_skipped {
    return 204;
}
```

- **`<root>` is quoted with `nginx_str`, as `fastcgi_pass` is (code check;** config-ownership
  rule). The plan checks that nginx accepts a quoted `unix:` upstream path.
- **Only PHP requests ask.** Static files and the setup page never need a database, so they
  never wake one, and an asset-heavy page costs one ask, not fifty **(builder's call)**.
- **Sites without services never ask.** The block is rendered only for a site whose `.yard`
  lists at least one service. A site without a `.yard`, or with no `[services]`, renders
  exactly as today **(builder's call)**.
- **yardd answers `204` once every instance the site needs is awake and healthy**, or
  immediately when none is asleep. It answers `204` too when it gives up (§6.3), so the app
  shows its own connection error rather than nginx showing a 500.
- **Fails open.** If `wake.sock` is missing, yardd is down, or yardd answers anything but 2xx,
  `error_page` turns the subrequest into `204` and the request goes through as it does today.
  The plan must verify that nginx applies `error_page` to a subrequest's own upstream errors
  (nginx-generated 502/504); if it does not, the plan picks another fail-open mechanism
  before writing code **(builder's call; verify)**.
- **`wake.sock`** is a unix socket under `run/`, mode 0600, owned by the user, served by
  yardd as minimal HTTP/1.1 (one route, `GET /wake/<site>`). It is a second way into yardd
  beside the JSON-RPC `yardd.sock` that main spec §3.1 and the rpc-protocol rule describe as
  the only one, so §3.1 changes with it (§15, and open for the maintainer, §13). It is not a TCP port, so it
  needs no registry entry (port-registry rule) **(builder's call)**. The site name in the
  path is matched against `sites.toml`; an unknown one is `204` and a log line, never an
  error page.
- **Each ask counts as use.** It refreshes the last-use time of every instance the site needs
  (§7), so a site under steady traffic keeps its instances awake without any connection
  counting.
- **Cost.** One local unix-socket round trip per PHP request when everything is awake. The
  plan measures it against a plain request (the target is under 1 ms at p50) and reports the
  number **(builder's call)**.

### 5.2 From the CLI: the shims ask before running

`yard-shim` (one binary for `php`, `composer`, `pecl`, `phpize`, `php-config`, and `node`,
`npm`, `npx` from M4c) already resolves the current directory's PHP from `yard.toml`,
`sites.toml` and the `.yard` with no daemon. For `php` and `composer` inside a linked site
whose `.yard` lists services, it wakes them before `exec` **(builder's call on which shims)**:

- **Only inside a linked site (code check).** The shim follows the nearest committed `.yard`
  even in a project that is not linked, where there is no site to name, and it never reads
  `services.toml`, which turning `redis = true` into an instance name needs. So the shim
  finds the innermost linked site containing the cwd, as it already does for PHP, and asks
  for that site.
- It reads `run/sleeping`, a small file yardd rewrites atomically whenever an instance falls
  asleep or wakes, listing **the linked sites that need a sleeping instance**, one per line
  (code check: yardd knows which instances each site needs; the shim does not). When the
  site is not listed, the shim does nothing more: an awake machine costs one small file
  read, and no socket connect **(builder's call)**.
- Otherwise it sends `GET /wake/<site>` on `wake.sock` and waits up to 30 s, printing one line
  to stderr (`yard: waking mysql-8.4 …`) so a pause is not a mystery. Then it `exec`s as
  today.
- **It never fails the command.** A missing socket, a dead daemon or a timeout prints one line
  to stderr and runs the command anyway.
- `node`, `npm`, `npx`, `pecl`, `phpize` and `php-config` never wake anything.
- **The shims no longer work entirely without a daemon.** They still run with yardd down (one
  stderr line), but AGENTS.md's layout and main spec §4.5 say they need no daemon, and that
  sentence changes with M4e (§15).

### 5.3 From the user

- `yard service start <i>` on a sleeping instance wakes it and waits for health, as it does
  for a stopped one (M4b §7.2). On a stopped instance it does what it does today.
- **A direct client** (TablePlus, a `mysql` by keg path, a script outside a project) cannot be
  seen before it connects. Against a sleeping instance it gets "connection refused". The fix
  is `yard service start <i>`, or `sleep_after = "off"` for an instance used that way. This is
  the price of keeping yardd out of the data path (§2) and is documented in `--help` and
  README.

## 6. Instance lifecycle

### 6.1 States

`ServiceState` gains `sleeping` and `waking`:

```
            idle for sleep_after,             wake (§5)
            no connections (§7)          ┌──────────────┐
  running ───────────────────────▶ sleeping ───────────▶ waking ──healthy──▶ running
     ▲                                                     │
     └──────────────── user: yard service start ◀──────────┘ (failed: as today)

  stopped (user) — never enters or leaves through sleep or wake
```

- `sleeping`: no unit runs, the port is free but still registered to the instance, and the
  data directory is closed cleanly.
- `waking`: the unit is starting; asks and shim wakes wait on it.
- **A wake that fails** behaves as a `yard service start` does today (code check):
  - a held port fails at once, `failed` with `PORT_IN_USE` naming the holder and no backoff;
  - a start that is not healthy within its start timeout is stopped and retried with backoff,
    and the instance is `failed` only after 5 failures in 60 s (main spec §6.1);
  - a missing keg is left out of the desired set (M4b), and the ask is answered at once.

  The waiting asks are answered `204` at the latest after 30 s (§6.3), whatever the unit is
  still doing.

### 6.2 How it maps onto the supervisor

The supervisor is desired-state: `Supervisor::apply(desired)` starts what is new and stops what
is gone. Sleeping is therefore *leaving the instance out of the desired set* **(builder's
call)**, but in its own set, not `Snapshot.left_out` **(code check)**: `left_out` maps a name
to a `YardError`, and `services.start` and `restart` refuse with whatever it holds
(`refuse_left_out`), which would refuse §5.3's start.

- The reconcile's service section skips an instance that is asleep and records it in the
  snapshot's sleeping set (name → since); `services.list` and `yard info` report `sleeping`
  from it rather than `failed`.
- `apply` deletes a unit that is no longer wanted from the supervisor's table, so `yard status`
  (which lists that table) would lose a sleeping instance; status reads the sleeping set too
  (§9, §11).
- Falling asleep: the idle check (§7) marks the instance asleep and triggers a reconcile, which
  stops the unit through `apply` with the unit's own stop signal and 30 s timeout (M4b §15).
  The instance is listed in `run/sleeping` only after the process is gone.
- Waking: mark awake, remove it from `run/sleeping`, reconcile, and wait for the first health
  check, bounded by the unit's `start_timeout` (120 s for MySQL and PostgreSQL, 10 s for Redis
  and MinIO, M4b §15).
- **A faster first health check (code check).** The supervisor checks health once at spawn and
  then every `health_interval` (5 s) until the first pass (`supervisor/unit.rs`). For §1.1's
  wake times, a starting unit polls much faster until its first pass (every 100–250 ms,
  builder's call), then falls back to the 5 s interval.
- **A wake that arrives while the instance is still stopping** waits for the stop to finish,
  then starts it. The state machine never runs two processes on one data directory.

### 6.3 How long an ask waits

nginx's ask and the shim wait at most **30 s** for a wake **(builder's call)**. A MySQL doing
crash recovery may take longer than that; the ask then answers `204`, the request goes through
and the app reports its own connection error, while the unit keeps starting under its 120 s
start timeout. The next request finds it awake. 30 s sits under nginx's `proxy_read_timeout`
of 35 s for the ask, so nginx never times out first.

### 6.4 At yardd start (login)

An instance with sleeping enabled **starts asleep** when yardd starts: at login, no database
runs until a site, a shim or the user needs one **(builder's call; open for the maintainer, §13)**. This
is where most of the saving comes from on a machine that is rebooted or logged into daily.

- `orphans::stop` still runs first for every instance (M4b §15), so a process a killed yardd
  left behind is stopped cleanly before the instance is considered asleep.
- An instance with `sleep_after = "off"` starts at yardd start, as today.
- `yard service add` starts the new instance awake and waits for health (it prints `yard env`
  lines after a working connection, M4b §7.2). The idle clock starts then.

## 7. Detecting idle

An instance is idle when **both** hold for the whole of `sleep_after`:

1. **No ask.** No `/wake` request named a site that needs it (each ask stamps a last-use time,
   kept in memory per instance), and no shim wake or `yard service start` touched it.
2. **No client connections.** yardd samples the engine process's open sockets every 30 s
   **(builder's call)** and counts connected client sockets (TCP `ESTABLISHED`, and accepted
   unix-domain sockets for MySQL's and PostgreSQL's sockets under `run/`), not listeners. A
   sample with any client connection resets the idle clock.

- **Count across the unit's process tree, not its pid alone (code check).** PostgreSQL hands
  each client connection to a forked backend and the postmaster closes its copy, so the
  postmaster's own fds show no clients while clients are connected. The sampler counts the
  unit's pid and its descendants.
- **How to count (builder's call on the mechanism).** Prefer libproc
  (`proc_pidinfo(PROC_PIDLISTFDS)` then `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)`), which costs
  microseconds and needs no subprocess; `fds.rs` already lists yardd's own fds with
  `PROC_PIDLISTFDS`. `holder.rs` asks `/usr/sbin/lsof`, which is the fallback if libproc
  cannot see unix-domain peers. The plan confirms which works on this machine before choosing.
- **Yard's own health checks do not count.** The supervisor's health check is a TCP connect
  that closes at once (main spec §6.1). A 30 s sampler will rarely see one. Socket info gives
  addresses, not the peer's pid, so the sampler skips a sample taken while that unit's health
  check is in flight rather than matching peers to yardd **(code check; builder's call)**.
- **Long transactions, `tinker`, queue workers, GUI clients** hold connections, so the
  instance stays awake for as long as they do. That is the intended behaviour: something is
  using it **(builder's call)**.
- **Pooled connections** (a framework that keeps one idle connection open) also keep it awake.
  Laravel does not pool under PHP-FPM; Octane or Horizon do, and while they run the database
  is in use.

## 8. Ports

- A sleeping instance's port stays in `ports.toml`, `pinned`, and no other Yard unit may take
  it (port-registry rule).
- The runtime scan of idle registered ports (every 30 s, the `port_conflict` event) already
  covers a sleeping instance's port. If another program binds it while the instance sleeps
  (DBngin started on 3307, say), the scan emits `port_conflict`, `yard doctor` reports the holder,
  and the next wake fails with `PORT_IN_USE` naming it, without backoff, as any start does
  today. No new code path.

## 9. CLI and output

- `yard services` STATE gains `sleeping` and `waking`, and a new column SLEEP shows the
  effective setting (`15m`, `1h`, `off`) **(builder's call)**. A sleeping row shows how long it
  has slept (`sleeping 2h`).
- `yard info` shows `sleeping` for a sleeping instance, with the line
  `wakes on the next request, or: yard service start <i>` under the table.
- `yard env` is unchanged: a sleeping instance's port is still its port.
- `yard status` lists sleeping instances as units at rest, not as failures. Status lists the
  supervisor's units, and a sleeping instance is not one (§6.2), so its rows come from the
  sleeping set (code check).
- `yard service sleep-after <i> <duration|off|default>`: §4.
- `--help` for `yard services`, `yard service start` and `yard service sleep-after` says that
  instances sleep when idle and how to keep one awake.
- Output follows the cli-output rule.

## 10. Changes to M4b behaviour

- **M4b §4: "Every other instance starts when yardd starts."** With sleeping on, an instance
  starts asleep (§6.4); with `sleep_after = "off"` it starts as before.
- **M4b §9: "A stopped or failed instance does not send the site to the setup page."** The same
  holds for a sleeping one: the site is served, and the wake hook wakes the instance first.
- **M4b §10 doctor:** `service:<i>` is `ok` for a sleeping instance (`sleeping since 14:02`), and
  `service-need:<site>` does not warn about one. A wake that failed is reported as the failure
  it is.

## 11. RPC, errors, protocol

- `ServiceState` gains `sleeping { since }` and `waking`. `ServiceInfo` gains
  `sleep_after_secs: Option<u64>` (`None` = off; the protocol writes durations as integers, as
  `retry_in_ms` does, code check) and its source. `SettingSource` has no `yard.toml` or
  `services.toml` variant yet; M4e adds them.
- `yard status`'s result gains the sleeping instances (§9).
- New method `services.sleep.set { instance, after }` for §4's command (code check: every
  multi-word method is dotted, as `php.config.set` is; none has an underscore).
  `services.start` accepts a sleeping instance and wakes it.
- No new error codes; a failed wake reuses `PORT_IN_USE`, `FORMULA_MISSING` and `JOB_FAILED`
  as `services.start` does.
- **Not a plain minor bump (code check).** The rpc-protocol rule gives an enum a newer daemon may
  extend a `#[serde(other)]` variant, and `ServiceState` has none: it is externally tagged, so
  a 2.7 CLI fails to decode `services.list` from a daemon that sends `sleeping`. Adding an
  `Other` variant now does not help clients already built. The choices are a major bump, or
  keeping `ServiceState`'s variants and adding an optional `sleeping_since` beside it, so an
  older CLI shows a sleeping instance as `stopped` (open for the maintainer, §13). Either way M4e adds
  the `Other` fallback for later states.
- `wake.sock` is not JSON-RPC and not part of `yard-proto`: it is an nginx- and shim-facing
  side door with one route and one answer (`204`).

## 12. Alternatives considered

- **yardd owns each instance's port and splices bytes to the engine (socket activation).**
  Wakes on *any* client, the GUI included. Rejected for M4e: large effort, puts yardd in the
  data path, and fights the port registry, probes, holder checks, doctor and `yard env` (the
  yard session's assessment, 2026-10-26). Kept as a later option if §5.3's gap hurts in daily
  use.
- **launchd socket activation.** mysqld and postgres cannot take a listening socket from
  launchd, so it does not apply.
- **Stop-only, no wake** (`sleep` with manual `yard service start`). Cheap, but every first
  page load after a break would fail until the maintainer typed a command. It is a fallback if §5.1's
  fail-open check cannot be made to work in nginx.
- **Sleeping PHP-FPM masters.** Saves a few MB per version against a 502 risk (§3). Later, if
  ever.

## 13. Open for the maintainer

1. **Default on or opt-in?** This draft turns sleeping on for every instance at 15 minutes.
   The alternative is `off` by default, enabled per instance or machine-wide.
2. **Start asleep at login (§6.4)?** It saves the most memory, and every first request of the
   day pays the cold start.
3. **Is the GUI gap acceptable (§5.3)?** TablePlus against a sleeping MySQL gets "connection
   refused" until `yard service start`, or the instance is set to `sleep_after = "off"`.
4. **Milestone placement.** M4e after M4c, before or after M4d emailulator. If M4d ships first
   and Yard runs emailulator, it is one more engine that can sleep. Main spec §12 says M4 is
   "in four plans"; M4e makes five.
5. **A second socket into yardd (§5.1).** `wake.sock` is plain HTTP beside the JSON-RPC
   `yardd.sock`, which main spec §3.1 calls the only way in. Is that acceptable under
   architecture A?
6. **Shims that ask the daemon (§5.2).** They fail open, but until now they needed no daemon at
   all. Acceptable?
7. **A project that is not linked** but has a committed `.yard` with services: should its shims
   wake anything? This draft says no (§5.2).
8. **`yard service restart` on a sleeping instance:** wake it, or refuse? Not decided here.
9. **How long a web request may wait for a wake.** 30 s (§6.3) is long for a page; a shorter cap
   means the app's own connection error sooner.
10. **Protocol (§11):** a major bump, or `sleeping_since` beside the existing states so an older
    CLI still decodes.

## 14. Testing

- **yard-core:** duration parsing and validation, the effective-setting resolution (instance,
  then `yard.toml`, then default), the idle predicate (last ask, connection samples, clock), the
  rendered PHP block with and without services (`insta` snapshots), and `run/sleeping` encoding.
- **yard-daemon:** with `yard-fake-unit` standing in for engines and a fake clock:
  falls asleep after `sleep_after` with no asks and no connections; stays awake while a sample
  shows a connection; an ask wakes it and answers only after health; concurrent asks share one
  start; a wake during a stop waits for the stop; a stopped instance is never woken; a failed
  wake answers `204` and leaves `failed`; yardd start leaves sleep-enabled instances asleep;
  `wake.sock` answers `204` for an unknown site.
- **yard-shim:** no socket connect when `run/sleeping` does not list the project's instances; a
  wake when it does; a dead socket still runs the command.
- **yard-cli:** `services` and `info` render `sleeping`/`waking` and SLEEP, human and `--json`,
  and `service sleep-after`.
- **nginx:** a rendered config passes `nginx -t`, and an integration test proves fail-open: with
  `wake.sock` absent, a PHP request is still served.
- **`scripts/e2e-m4e.sh`** on the maintainer's machine, after `e2e-m1.sh` and with M4b's instances, asked
  first: sets `sleep_after = "60s"` on a scratch instance, links a scratch project whose `.yard`
  lists it, waits for it to sleep and checks the process is gone, curls the site and checks it
  answers with a working DB query, checks a `php` through the shim wakes it, checks a held
  connection keeps it awake, then restores every TOML it touched. DBngin's, Postgres.app's and
  Herd's ports and files come out unchanged.

## 15. When built

The same change updates the main spec (§3.1 for `wake.sock`, §4.2 note, §4.5 for shims that ask
the daemon, §6.1 supervisor states and the faster first health check, §6.2 methods, §12
milestones and its "four plans"), the M4b spec where §10 says, README, `--help`, AGENTS.md (status, the layout line saying the
shims need no daemon, and the `e2e-m4e.sh` entry under Build & test), and the hydra rules: config-ownership (`sleep_after`
in `yard.toml` and `services.toml`, `run/sleeping`), rpc-protocol (new states and method),
port-registry (a sleeping instance keeps its pinned port), and cli-output as its checklist
says.

## 16. Left to the builder

The exact Rust shape of the idle tracker and of `ServiceState`'s new variants; the HTTP
handling on `wake.sock` (hand-rolled or a small crate, within the rust-workspace rule's
dependency policy and `cargo deny`); how the fake clock is wired into `DaemonOptions`; the
wording of messages and hints within the cli-output rule.
