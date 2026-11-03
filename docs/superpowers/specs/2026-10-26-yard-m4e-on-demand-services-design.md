# Yard M4e: on-demand services (sleep when idle, wake on use) — design

Status: **approved by the maintainer on 2026-10-26.** Written 2026-10-26 in a separate session (the maintainer's homedir
session), from the yard session's account of the code at main `2472f47` (M4c in progress),
then checked against the code at `c4e6eef` (M4c built and verified) the same day and
corrected where it disagreed; corrections the code forced are marked **(code check)**. The maintainer
answered §13's ten questions the same day, taking the yard session's recommendation on each
(marked **(maintainer, §13)**), and approved the spec. Built on main on 2026-10-26 (§15); what the
build settled is marked **(built)**. Part of M4 (main spec
§12). It builds on the M4b service model
(`docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md`) and changes no M4b
decision except where §10 says so.

Decisions marked **(maintainer)** are the maintainer's, from the 2026-10-26 conversation. Decisions marked
**(builder's call)** were made while writing this and can be reversed on their own. §13
records the maintainer's answers to the questions the draft left open.

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

1. With sleeping turned on (on by default since 2026-11-03, §4), a service instance that nothing has
   used for `sleep_after` is stopped gracefully, the way `yard service stop` stops it, and shows as `sleeping`.
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
- Predictive pre-warming (waking at `cd` into a project, on a schedule). Instances start awake
  at login as today (§6.4).

Anti-goals (a failure even if it works):

- Waking an instance the user stopped, or sleeping one and leaving `stopped` behind.
- Killing an engine mid-write: sleep is the unit's graceful stop with its 30 s stop timeout
  (M4b §15), never SIGKILL first.
- A site that answers 5xx because yardd is down or slow. The wake hook fails open (§5.1). The
  one deliberate 503 is a site whose instance cannot start because another program holds its
  port (§5.1, added 2026-11-02): serving the page would let PHP talk to that program.
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
service_sleep_after = "15m"     # absent or "off": no instance sleeps unless its own key says so
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
- **On by default, 15 minutes (maintainer, 2026-11-03; it was off by default, opt-in, from §13 until
  then).** With no `service_sleep_after` in `yard.toml` and no `sleep_after` on the instance, the
  instance sleeps after 15 minutes unused (`DEFAULT_SLEEP_AFTER`, applied by yardd's
  `SleepConfig.default_after`; test daemons have none). `service_sleep_after = "off"` keeps every
  instance up, `sleep_after = "off"` one. The GUI gap below is the price the maintainer accepted with it.
  What follows is the opt-in text of 2026-10-26, kept for its reasoning: with no setting,
  nothing slept, and Yard behaved as M4b. Turning it on is
  `service_sleep_after = "15m"` for every instance, or `sleep_after` on one. Sleeping on for
  everyone would break every GUI client after 15 idle minutes until its user learned why
  (§5.3). When turned on, it applies to every engine alike, Redis included; `sleep_after =
  "off"` on an instance keeps that one up. **Turning it off undoes it (built):** an asleep
  instance whose setting becomes `off` or goes away, or that is stopped or removed, is
  forgotten by the next reconcile, which starts it (unless stopped) and drops its sites from
  `run/sleeping`.
- `yard service sleep-after <instance> <duration|off|default>` edits the instance key
  (`default` removes it) **(builder's call on the name)**. The machine-wide key is edited by
  hand, like `default_node`'s neighbours, or by a `yard config` command if one exists by then.

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
    error_page 403 = @yard_wake_refused;
    # ... unchanged fastcgi block ...
}

location = /__yard_wake {
    internal;
    proxy_pass http://unix:"<root>/run/wake.sock":/wake/<site>?wait=10;
    proxy_pass_request_headers off;
    proxy_pass_request_body off;
    proxy_set_header Content-Length "";
    proxy_read_timeout 15s;
    proxy_intercept_errors on;
    error_page 400 401 404 500 502 503 504 = @yard_wake_skipped;
}

location @yard_wake_skipped {
    return 204;
}

location @yard_wake_refused {
    default_type text/plain;
    return 503 "A service this site needs cannot start: another program holds its port. Run yard doctor to see which.\n";
}
```

- **`<root>` is quoted with `nginx_str`, as `fastcgi_pass` is (code check;** config-ownership
  rule). nginx accepts the quoted `unix:` upstream path (checked 2026-10-26 with brew's nginx,
  `nginx -t` in `tests/real_nginx.rs`) **(built)**.
- **The ask carries only its path.** The client's headers, cookies included, and its body stay
  out of the subrequest (`proxy_pass_request_headers off`, `proxy_pass_request_body off`):
  yardd needs nothing else, and a session cookie has no business on `wake.sock`
  (checked with brew's nginx in `the_wake_hook_fails_open`) **(built, after the final review)**.
- **Only PHP requests ask.** Static files and the setup page never need a database, so they
  never wake one, and an asset-heavy page costs one ask, not fifty **(builder's call)**.
- **Sites without services never ask.** The block is rendered only for a site whose `.yard`
  lists at least one service. A site without a `.yard`, or with no `[services]`, renders
  exactly as today **(builder's call)**. Nor does a site whose instances cannot sleep: the
  block is rendered only when the site needs an instance that is not stopped and whose
  effective `sleep_after` is set, so with sleeping off no request reaches yardd and Yard
  serves as M4b did (§4) **(built, after the final review)**.
- **yardd answers `204` once every instance the site needs is awake and healthy**, or
  immediately when none is asleep. It answers `204` too when it gives up (§6.3), so the app
  shows its own connection error rather than nginx showing a 500.
- **Fails open.** If `wake.sock` is missing, yardd is down, or yardd answers anything but 2xx
  or `403`, `error_page` turns the subrequest into `204` and the request goes through as it does today.
  nginx applies `error_page` to the subrequest's own upstream errors, and with
  `proxy_intercept_errors on` to an error status yardd might answer, so the 4xx codes
  `auth_request` would turn into a refusal are caught too (checked 2026-10-26 with brew's
  nginx: no socket, a `500`, a hang past the timeout and a late `204` all serve the page,
  `the_wake_hook_fails_open`) **(built)**.
- **Refused when another program holds the port (2026-11-02, a security review's finding).** A
  sleeping instance leaves its 127.0.0.1 port unbound, and another local user (or any program)
  can bind it and pose as the database: on PHP 7.0 and 7.1, whose `mysqli.allow_local_infile`
  is on, a fake MySQL can read the developer's files with `LOAD DATA LOCAL INFILE`, and a fake
  Redis can feed Laravel's cache a crafted serialized value. So when an instance the site needs
  is `failed` with `PORT_IN_USE` (its start found the port held, §8), yardd answers `403` with
  a line per such instance (`redis: port 6380 is held by nc (pid 9)`); `auth_request` turns it
  into a refusal and the PHP location's `error_page 403` serves `@yard_wake_refused`'s 503,
  which names `yard doctor`. PHP never runs (checked with brew's nginx in
  `the_wake_hook_fails_open`) **(built)**. A refused instance keeps its sites listed in
  `run/sleeping` until it runs, so every `php` and `composer` asks and is refused too, not
  only the first; and once the other program lets go of the port, the next ask restarts the
  instance (`restart_and_wait`) instead of leaving the site refused until someone runs
  `yard service restart`. Only nginx-made errors are affected by the location's `error_page`:
  nginx inherits a server-level `error_page` (a user's override) only into a location that has
  none, so on a site with the wake hook such an override no longer applies to PHP requests (a
  custom 502 page for PHP-FPM being down, say). PHP's own error pages are untouched, since
  nginx does not intercept them **(Claude's call: the 503 that says why is worth it)**.
- **A cap on waiting asks (2026-11-02).** At most 64 asks wait on a wake at once
  (`wake::WAKE_SLOTS`); one past that starts the wake and is answered at once, so a flood of
  requests (a hostile page firing at `*.<site>.yard`) cannot pile up in yardd. A waiting ask
  reads the snapshot in place (`Reconciler::read`) rather than copying it every poll
  **(built)**.
- **`wake.sock`** is a unix socket under `run/`, mode 0600, owned by the user, served by
  yardd as minimal HTTP/1.1 (one route, `GET /wake/<site>?wait=<s>`). It is a second way into
  yardd beside the JSON-RPC `yardd.sock` that main spec §3.1 and the rpc-protocol rule
  describe as the only one, so §3.1 changes with it (§15). **Accepted (maintainer, §13):** nginx can
  only call an HTTP upstream, not JSON-RPC, so waking from the web needs it. It stays that
  narrow: one route, readable only by the user, and it can only start instances that already
  exist and are not `stopped`; it never adds, changes or removes anything. It is not a TCP port, so it
  needs no registry entry (port-registry rule) **(builder's call)**. The site name in the
  path is matched against `sites.toml`; an unknown one is `204` and a log line, never an
  error page.
- **Each ask counts as use.** It refreshes the last-use time of every instance the site needs
  (§7), so a site under steady traffic keeps its instances awake without any connection
  counting.
- **Cost.** One local unix-socket round trip per PHP request when everything is awake. The
  target is under 1 ms at p50 **(builder's call)**; `scripts/e2e-m4e.sh` measures the ask
  round trip on the real machine and prints its p50, which was 0.16 ms on the maintainer's machine on
  2026-10-26 **(built)**.

### 5.2 From the CLI: the shims ask before running

`yard-shim` (one binary for `php`, `composer`, `pecl`, `phpize`, `php-config`, and `node`,
`npm`, `npx` from M4c) already resolves the current directory's PHP from `yard.toml`,
`sites.toml` and the `.yard` with no daemon. For `php` and `composer` inside a linked site
whose `.yard` lists services, it wakes them before `exec` **(builder's call on which shims)**:

- **Only inside a linked site (code check; confirmed, the maintainer, §13).** The shim follows the
  nearest committed `.yard` even in a project that is not linked, where there is no site to
  name, and it never reads `services.toml`, which turning `redis = true` into an instance
  name needs. So the shim finds the innermost linked site containing the cwd, as it already
  does for PHP, and asks for that site.
- It reads `run/sleeping`, a small file yardd rewrites atomically in every reconcile when it
  changed, before that reconcile stops or starts a unit (so a site linked, or a `.yard`
  edited, while an instance sleeps is listed too, and a site is listed from the moment its
  instance starts to drain, before it stops, §6.2; built), listing **the linked sites that need a sleeping instance**, one per line
  (code check: yardd knows which instances each site needs; the shim does not). When the
  site is not listed, the shim does nothing more: an awake machine costs one small file
  read, and no socket connect **(builder's call)**.
- Otherwise it sends `GET /wake/<site>?wait=30` on `wake.sock` and waits up to 30 s (a CLI
  command such as `php artisan migrate` can wait longer than a page, §6.3), printing one line
  to stderr so a pause is not a mystery. Then it `exec`s as today. The line names the site,
  `yard: waking the services <site> uses …`, since `run/sleeping` lists sites, not instances
  **(built)**.
- **It never fails the command because yardd cannot answer.** A missing socket, a dead daemon or a timeout prints
  `yard: could not wake <site>'s services (<error>); running anyway` and runs the command
  **(accepted, the maintainer, §13)**. The one refusal is yardd's `403` (§5.1): the shim prints
  `yard: <site>'s services cannot start, so PHP did not run: <what holds which port>` with the
  hint `yard doctor`, and exits 1 without running PHP **(built, 2026-11-02)**. A unix-socket connect does not block (a missing or dead socket
  fails at once, and a hung yardd's backlog takes the connect), so there is no separate
  connect timeout: the read and write timeouts are 31 s, one more than the `wait` **(built)**.
- `node`, `npm`, `npx`, `pecl`, `phpize` and `php-config` never wake anything.
- **The shims no longer work entirely without a daemon.** They still run with yardd down (one
  stderr line), but AGENTS.md's layout and main spec §4.5 say they need no daemon, and that
  sentence changes with M4e (§15).

### 5.3 From the user

- `yard service start <i>` on a sleeping instance wakes it and waits for health, as it does
  for a stopped one (M4b §7.2). On a stopped instance it does what it does today.
- `yard service restart <i>` on a sleeping instance wakes it, as `start` does, and waits for
  health **(maintainer, §13)**: restart means "make it run fresh", and refusing would only make the
  user type `start` instead.
- **A direct client** (TablePlus, a `mysql` by keg path, a script outside a project) cannot be
  seen before it connects. Against a sleeping instance it gets "connection refused". The fix
  is `yard service start <i>`, or `sleep_after = "off"` for an instance used that way. This is
  the price of keeping yardd out of the data path (§2) and is documented in `--help` and
  README. **Accepted, since sleeping is opt-in (maintainer, §13); accepted again when sleeping became the
default (maintainer, 2026-11-03).** `yard services` shows the
  instance as `sleeping`, and the hint is `yard service start <i>`.

## 6. Instance lifecycle

### 6.1 States

An instance gains two states, `sleeping` and `waking`. On the wire they are carried without
new `ServiceState` variants (§11): `sleeping` is `stopped` with `sleeping_since` set, and
`waking` is `starting` with it set.

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

  The waiting asks are answered `204` at the latest after their `wait` (10 s from nginx, 30 s
  from a shim, §6.3), whatever the unit is still doing, except that an instance `failed` with
  `PORT_IN_USE` makes the answer `403` (§5.1).

### 6.2 How it maps onto the supervisor

The supervisor is desired-state: `Supervisor::apply(desired)` starts what is new and stops what
is gone. Sleeping is therefore *leaving the instance out of the desired set* **(builder's
call)**, but in its own set, not `Snapshot.left_out` **(code check)**: `left_out` maps a name
to a `YardError`, and `services.start` and `restart` refuse with whatever it holds
(`refuse_left_out`), which would refuse §5.3's start.

- The reconcile's service section skips an instance that is asleep. Sleep lives in yardd's
  `Sleeper` (`sleep.rs`: per instance, the last use, since when it sleeps, and whether it is
  waking), runtime state beside the snapshot rather than in it **(built)**; `services.list`
  and `yard info` report `sleeping` from it rather than `failed`.
- **Readers never wait on a stop (built, after the final review).** The reconcile holds its
  state lock through `apply`, which stops units one at a time, so it publishes each snapshot
  where `Reconciler::snapshot` reads it without that lock: a wake ask, `yard status` or
  `yard services` answers from the last good snapshot while a stop is in progress. A wake
  sets its deadline before anything else and waits on its (spawned) reconcile only until
  then, so `wait` bounds the answer even when that reconcile queues behind a stop.
- `apply` deletes a unit that is no longer wanted from the supervisor's table, so `yard status`
  (which lists that table) would lose a sleeping instance; status reads the sleeping set too
  (§9, §11).
- Falling asleep takes two passes **(built 2026-11-02, after a robustness review: a client that
  connected between the count and the stop was cut off by it, and a `migrate` killed mid-DDL
  is not undone)**. First the idle check (§7) marks the instance *draining* and triggers a
  reconcile, which lists its sites in `run/sleeping` while it still runs, so a shim run from
  then on asks for a wake, which counts as use. After `SleepConfig.drain` (1 s) its clients
  are counted again; any client, closed connection or use since the drain began keeps it
  awake (and the next reconcile takes its sites out of the file again). Only then is it marked
  asleep, and a reconcile stops the unit through `apply` with the unit's own stop signal and
  30 s timeout (M4b §15). Its sites stay listed through the stop, so a shim run during the stop
  asks for a wake, which waits for the stop and starts it again, rather than finding a
  stopping engine **(built, after the final review; this spec first listed them only once the
  process was gone)**. The idle check and the marking happen under one lock, so a use that
  lands between them keeps the instance awake. A reconcile that fails before it stops the unit
  (a TOML that no longer parses, an `nginx -t` failure) undoes the sleep: the instance runs
  and is reported running, and its idle clock starts again **(built, after the final review)**.
- Waking: mark awake, remove it from `run/sleeping`, reconcile, and wait for the first health
  check, bounded by the unit's `start_timeout` (120 s for MySQL and PostgreSQL, 10 s for Redis
  and MinIO, M4b §15).
- **The stopping process is not the instance (2026-11-02).** While a sleep's stop is under
  way the supervisor still lists the unit `running` with its old pid, so the reconcile that
  stops it records that pid as it hands the supervisor its units (`Sleeper::set_stopping`;
  `None` for an instance it keeps), and no wake counts the instance up until a different
  process runs (`Sleeper::running`). It is recorded there, not when the sleep is decided: a
  wake in between keeps the unit, and its process is then the instance. Before this, a second ask during a stop answered at once, and cleared the first ask's
  wait with it.
- **A wake while a TOML does not parse (2026-11-02).** A wake needs a reconcile, and one that
  fails at load (a half-saved `sites.toml`) used to leave the instance down, every request
  waiting out its `wait`. The reconcile keeps the units it last applied and the unit each
  asleep instance would run (`State.applied`, `State.slept`), and a wake whose reconcile fails
  starts the woken instances from those (`Reconciler::wake_from_last_good`), as a parse error
  keeps the last good config serving everything else.
- **A faster first health check (code check).** The supervisor checks health once at spawn and
  then every `health_interval` (5 s) until the first pass (`supervisor/unit.rs`). For §1.1's
  wake times, a starting unit polls much faster until its first pass, every 200 ms
  (`SupervisorConfig.start_poll`) **(built)**, then falls back to the 5 s interval.
- **A wake that arrives while the instance is still stopping** waits for the stop to finish,
  then starts it. The state machine never runs two processes on one data directory.

### 6.3 How long an ask waits

nginx's ask waits at most **10 s** for a wake **(maintainer, §13)**, and a shim's at most **30 s**
**(builder's call)**; each passes its limit as `wait`, which yardd caps at 30. MySQL and
PostgreSQL start in a few seconds here, and a page that hangs for 30 s is worse than the app's
own connection error. A MySQL doing crash recovery may take longer than the limit; the ask
then answers `204`, the request goes through and the app reports its own connection error,
while the unit keeps starting under its 120 s start timeout. The next request finds it awake.
nginx's `proxy_read_timeout` for the ask is 15 s, above its 10 s `wait`, so yardd answers
before nginx gives up.

### 6.4 At yardd start (login)

Every instance that is not `stopped` **starts awake** when yardd starts, as today, and its
idle clock starts then; one with sleeping on falls asleep after `sleep_after` if nothing uses
it **(maintainer, §13)**. Starting asleep would save more memory, but every first request of the day
would pay the cold start.

- `orphans::stop` still runs first for every instance (M4b §15), as today.
- `yard service add` starts the new instance awake and waits for health (it prints `yard env`
  lines after a working connection, M4b §7.2). The idle clock starts then.

## 7. Detecting idle

An instance is idle when **all three** hold for the whole of `sleep_after`:

1. **No ask.** No `/wake` request named a site that needs it (each ask stamps a last-use time,
   kept in memory per instance), and no shim wake or `yard service start` touched it.
2. **No client connections.** yardd samples the engine process's open sockets every 20 s
   **(builder's call; 30 s until 2026-11-02)** and counts connected client sockets (TCP
   `ESTABLISHED`, and accepted unix-domain sockets for MySQL's and PostgreSQL's sockets under
   `run/`), not listeners. A sample with any client connection resets the idle clock.
3. **No connection closed since the last sample (2026-11-02).** A client that connects for a
   moment (`schedule:run` every minute, which Laravel runs by `PHP_BINARY`, the keg's path, and
   so past the shim; the MinIO console; a proxied Node app) is rarely there when a sample
   looks. The connection it closed is: macOS keeps it in `TIME_WAIT` for 30 s
   (`net.inet.tcp.msl` is 15 s; measured 31 s on the maintainer's machine), longer than the sample
   interval. Each sample reads `/usr/sbin/netstat -an -p tcp`, and a `TIME_WAIT` on an
   instance's loopback port that it has not seen before counts as one use of that instance,
   whichever instance the pass is about (`conns::Closed`, `touch_closed`); the same port on
   another host is another server. A client that resets its connection instead of closing it
   leaves none and is missed.

- **Count across the unit's process tree, not its pid alone (code check).** PostgreSQL hands
  each client connection to a forked backend and the postmaster closes its copy, so the
  postmaster's own fds show no clients while clients are connected. The sampler counts the
  unit's pid and its descendants.
- **How to count (built).** `conns.rs` walks the tree with libproc's `proc_listchildpids`
  and asks `/usr/sbin/lsof -F dn` for the tree's established TCP and unix sockets, with a 5 s
  timeout. libc has `proc_listchildpids` but no binding for `socket_fdinfo`, so the sockets
  themselves cannot be read through libproc here. lsof's `d` field is the socket's kernel
  address, and each socket counts once: a backend can hold its connection on two
  descriptors, and a postmaster keeps a copy until it has handed it over. A connection whose
  two ends both belong to the tree (MinIO's console calling its own API) is the engine talking
  to itself and does not count **(built, after the final review)**. A count that fails keeps
  the instance awake.
- **Yard's own connections.** The supervisor's health check is a TCP connect that closes at
  once (main spec §6.1), every 5 s while a unit runs, and `yard ports` and `yard doctor`
  probe ports the same way. Each records its local port (`conns::own_connection`), and a
  `TIME_WAIT` whose other end is one of those is not a client **(built 2026-11-02; before it,
  an open health check seen by a sample kept the instance awake one sample longer, which still
  holds)**.
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

- **M4b §4: "Every other instance starts when yardd starts."** Unchanged (§6.4); with
  sleeping on, it may fall asleep later.
- **M4b §9: "A stopped or failed instance does not send the site to the setup page."** The same
  holds for a sleeping one: the site is served, and the wake hook wakes the instance first.
- **M4b §10 doctor:** `service:<i>` is `ok` for a sleeping instance (`sleeping since 14:02`), and
  `service-need:<site>` does not warn about one. A wake that failed is reported as the failure
  it is.

## 11. RPC, errors, protocol

- `ServiceState` keeps its variants **(maintainer, §13)**. `ServiceInfo` gains
  `sleeping_since: Option<String>` (RFC 3339): set with `stopped` for a sleeping instance and
  with `starting` for a waking one, absent otherwise. A 2.7 CLI ignores the field and shows a
  sleeping instance as `stopped` and a waking one as `starting`, which is true of the process.
  The protocol becomes **2.8**, a minor bump. `ServiceInfo` also gains
  `sleep_after_secs: Option<u64>` (`None` = off; the protocol writes durations as integers, as
  `retry_in_ms` does, code check) and its source. `SettingSource` has no `yard.toml` or
  `services.toml` variant yet; M4e adds them.
- `yard status`'s result gains the sleeping instances (§9), as an optional field older CLIs
  ignore.
- New method `services.sleep.set { name, after }` for §4's command (code check: every
  multi-word method is dotted, as `php.config.set` is; none has an underscore).
  `services.start` accepts a sleeping instance and wakes it.
- No new error codes; a failed wake reuses `PORT_IN_USE`, `FORMULA_MISSING` and `JOB_FAILED`
  as `services.start` does.
- **Why not new variants (code check).** The rpc-protocol rule gives an enum a newer daemon
  may extend a `#[serde(other)]` variant, and `ServiceState` has none: it is externally
  tagged, so a 2.7 CLI would fail to decode `services.list` from a daemon that sent
  `sleeping`. Adding an `Other` variant now does not help clients already built, so the new
  states ride on `sleeping_since` instead of a major bump. M4e still adds the `Other`
  fallback, so a later state can be a variant, with or without data: `#[serde(other)]` alone
  catches only a unit variant, so `ServiceState`'s `Deserialize` is written by hand to read an
  unknown `{"<state>": {…}}` as `Other` too, while a known state that does not parse is still
  an error **(built, after the final review)**.
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

## 13. the maintainer's answers (2026-10-26)

Each was the yard session's recommendation, and the maintainer took them all ("Proceed").

1. **Default on or opt-in?** Opt-in: `service_sleep_after` is off unless set, with a
   per-instance override (§4). **Changed 2026-11-03 (maintainer): on by default, 15 minutes.**
2. **Start asleep at login?** No. Instances start awake as today and sleep only after being
   idle (§6.4).
3. **The GUI gap?** Acceptable, since sleeping is opt-in; `yard services` shows `sleeping` and
   the hint is `yard service start <i>` (§5.3).
4. **Milestone placement.** After M4d, as a fifth M4 plan; main spec §12's "four plans"
   becomes five when M4e is approved.
5. **A second socket into yardd?** Accepted, kept to one route that only wakes existing
   instances, readable only by the user (§5.1).
6. **Shims that ask the daemon?** Accepted, with a short connect timeout; they run the command
   whatever yardd does (§5.2).
7. **A project that is not linked?** Wakes nothing (§5.2).
8. **`yard service restart` on a sleeping instance?** Wakes it (§5.3).
9. **How long a web request waits.** 10 s; a shim waits 30 s (§6.3).
10. **Protocol.** `sleeping_since` beside the existing states, protocol 2.8 (§11).

## 14. Testing

- **yard-core:** duration parsing and validation, the effective-setting resolution (instance,
  then `yard.toml`, then default), the idle predicate (last ask, connection samples, clock), the
  rendered PHP block with and without services (`insta` snapshots), and `run/sleeping` encoding.
- **yard-daemon:** with `yard-fake-unit` standing in for engines and a fake clock:
  falls asleep after `sleep_after` with no asks and no connections; stays awake while a sample
  shows a connection; an ask wakes it and answers only after health; concurrent asks share one
  start; a wake during a stop waits for the stop; a stopped instance is never woken; a failed
  wake answers `204` and leaves `failed`; yardd start starts sleep-enabled instances awake
  (§6.4);
  `wake.sock` answers `204` for an unknown site and at its `wait`, capped at 30 s;
  `services.restart` wakes a sleeping instance; with no sleep setting anywhere nothing ever
  sleeps.
- **yard-shim:** no socket connect when `run/sleeping` does not list the site; a wake when it
  does; a dead socket still runs the command; the Node shims never wake.
- **yard-cli:** `services` and `info` render `sleeping`/`waking` and SLEEP, human and `--json`,
  and `service sleep-after`; a 2.7-shaped `ServiceInfo` without `sleeping_since` still decodes.
- **nginx:** a rendered config passes `nginx -t`, and an integration test proves fail-open: with
  `wake.sock` absent, a PHP request is still served.
- **`scripts/e2e-m4e.sh`** on the maintainer's machine, after `e2e-m1.sh` and with M4b's instances, asked
  first: sets `sleep_after = "60s"` on a scratch instance, links a scratch project whose `.yard`
  lists it, waits for it to sleep and checks the process is gone, curls the site and checks it
  answers with a working DB query, checks a `php` through the shim wakes it, checks a held
  connection keeps it awake, then restores every TOML it touched. DBngin's, Postgres.app's and
  Herd's ports and files come out unchanged. Since 2026-11-02 it does this with scratch Redis,
  MySQL 8.0 and PostgreSQL 17 instances at once, times each engine's cold start through
  `yard service start` against §1.1's targets, fails a waking page slower than nginx's 10 s
  wait, and holds a connection to each engine.

## 15. When built

Hardened on 2026-11-02 for production, after a robustness review and a security review of the
feature: closed connections count as use (§7), an instance drains before it stops (§6.2), a
stopping process is not taken for the woken one and a wake works while a TOML does not parse
(§6.2), a port another program holds refuses the ask (§5.1), and waiting asks are capped
(§5.1). `scripts/e2e-m4e.sh` now runs the loop with real Redis, MySQL 8.0 and PostgreSQL 17
at once and times each engine's cold start (§14).

Built on main on 2026-10-26, from `8f5e06a` to the commit that adds `scripts/e2e-m4e.sh`, and
verified on the maintainer's machine the same day with `scripts/e2e-m1.sh` and `scripts/e2e-m4e.sh`
passing, at `a1083ed` and again at `12babe5` after the final review's fixes. The review's
deferred minors were fixed on 2026-10-27: the ask's headers (§5.1), `run/sleeping` written
before the stop, the idle check under one lock and a failed sleep undone (§6.2), an engine's
connections to itself (§7) and `ServiceState`'s fallback for a state with data (§11).

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
dependency policy and `cargo deny`; built hand-rolled, `wake.rs`); how the fake clock is
wired into `DaemonOptions` (built: `SleepConfig`, whose `second` is how long one second of a
`sleep_after` lasts, 1 ms in tests, and whose `sample_interval` is 50 ms there); the
wording of messages and hints within the cli-output rule.
