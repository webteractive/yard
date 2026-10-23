# Yard M4b: services, `yard env`, and brew sources — design

Status: approved by the maintainer 2026-10-23; built on branch `m4b-services` (plan
`docs/superpowers/plans/2026-10-23-yard-m4b-services.md`, approved 2026-10-24); the real-machine
check `scripts/e2e-m4b.sh` has yet to run. Part of M4 (main spec §12). It refines the
main spec's §7 and §8.2 (`docs/superpowers/specs/2026-10-19-yard-design.md`) and fills in
`[services]` in the project-file spec §4
(`docs/superpowers/specs/2026-10-22-yard-project-file-design.md`). emailulator moved out
of M4b to M4d on 2026-10-23 (`docs/superpowers/specs/2026-10-23-yard-emailulator-design.md`);
this spec provides the service model M4d builds on.

Decisions marked **(maintainer)** are the maintainer's, from the 2026-10-23 conversation. Decisions
marked **(builder's call)** were made while writing this and can be reversed on their own.

## 1. Intent

Herd paywalls services, so the maintainer runs four tools beside it: DBngin (MySQL, Redis),
Postgres.app (PostgreSQL), and emailulator. M4b moves the databases into Yard, the way
DBngin does it: add an instance of an engine at a version, run several versions side by
side, start and stop them, and see their ports. Legacy projects keep their old MySQL, and
new ones can try the newest release.

A project's `.yard` says which services it needs, so a clone gets them from `yard init`.
`yard env` prints the `.env` lines to connect to them.

Homebrew supplies every engine. Homebrew 6+ has its own trust model for third-party
taps; Yard uses it instead of building one, and stops granting trust silently.

## 2. Decisions

- **Services work like DBngin (maintainer).** `yard service add <engine> --version <v>` creates
  an instance; several versions of one engine run side by side.
- **Engines in M4b: MySQL, PostgreSQL, Redis, MinIO (maintainer).** MySQL 8.0 and later only;
  no 5.7, which brew no longer ships and the maintainer's projects do not need **(maintainer)**. MinIO
  stays in although its upstream is archived (maintainer, after being told; §5.4).
- **No MariaDB or Valkey in M4b (builder's call).** Engines are one table entry each
  (§5), so either can follow if a project runs on it in production.
- **No data import (maintainer).** Yard never reads DBngin's or Postgres.app's data; moving a
  project is a dump and a re-import into the Yard instance.
- **Yard never creates databases, users or buckets (maintainer).** That stays with the user
  and the framework (`php artisan migrate` offers to create a missing database).
- **No ports in `.yard` (maintainer agreed).** A port belongs to the machine. Instances are
  shared, so a project cannot own one's port.
- **Ports are chosen once and kept (builder's call).** An instance takes its engine's
  usual port, else the next free one above it, and keeps it until the user moves it
  (§6).
- **Brew's trust store is the only record of trust (maintainer).** Yard proxies
  `brew trust`/`untrust` as `yard source trust|untrust`, and lists sources with
  `yard sources` (maintainer chose the name). Only `yard source trust` and the install prompt
  grant trust; no other Yard command does, `--yes` included (§8).
- **emailulator is M4d (maintainer).** Nothing in M4b runs, finds or proxies it.
- **`yard info` shows one site in full, service logins included (maintainer asked for it)**
  (§7.4, §7.5).

## 3. Goals, non-goals, anti-goals

Goals:

1. `yard service add mysql --version 8.0` and `--version 8.4` give two running MySQL
   servers on 127.0.0.1, each on its own port, with data under Yard's data root.
2. PostgreSQL 15–18, Redis and MinIO the same way.
3. A `.yard` with `[services]` brings the instances up on `yard init` / `yard link`, after
   the same y/N as a missing PHP.
4. `yard env` prints the connection lines for a project's services.
5. Every install from a third-party tap needs the user's trust, recorded by brew.
6. DBngin, Postgres.app and Herd keep working beside Yard.

Non-goals:

- emailulator (M4d), Node (M4c).
- Importing data from DBngin or Postgres.app; creating databases, roles or buckets.
- Backups, replication, `pg_upgrade`, or moving an instance's data between versions.
- Per-project instances (`.yard` cannot ask for its own private server).
- MariaDB, Valkey, MongoDB, Meilisearch and other engines.
- Upgrading brew formulae (`brew upgrade` stays the user's).

Anti-goals (a failure even if it works):

- A service listening on anything but 127.0.0.1.
- An instance's port changing without the user asking.
- Yard granting trust to a third-party tap or formula as a side effect of another
  command (an install, `--yes`, `yard init`).
- Yard taking a port from DBngin, Postgres.app or Herd, or touching their files.
- `brew services`, or any file written under the brew prefix (homebrew rule).
- Losing an instance's data on `yard service remove` without the user asking for it
  (`--keep-data` is the safe opt-out, but plain `remove` deletes; §7.2).

## 4. Instances

An instance is one engine on one release line, named `<engine>-<line>`: `mysql-8.4`,
`mysql-8.0`, `postgres-17`. An engine brew ships as a single formula has one instance,
named after the engine: `redis`, `minio` **(builder's call)**. The name is the unit name,
the registry entry, the log file and the directory under `services/`.

**A line is a formula, not a patch release.** `mysql-8.4` is whatever `mysql@8.4` is
after `brew upgrade` (8.4.13 today); the name never carries a patch number, so an upgrade
never renames an instance or unmeets a `.yard` (§9).

Instances live in a new user TOML, `services.toml`, edited only through `TomlFile`
(config-ownership rule) **(builder's call)**:

```toml
# services.toml: the service instances Yard runs. `yard service` edits it.
[services."mysql-8.4"]
engine = "mysql"
version = "8.4"
formula = "mysql@8.4"

[services."postgres-17"]
engine = "postgres"
version = "17"
formula = "postgresql@17"
stopped = true          # the user stopped it; reconcile leaves it stopped
```

- `formula` is resolved once, at `add` (§5), and recorded, so reconcile never asks brew.
- `stopped = true` is written by `yard service stop` and removed by `start`. A stopped
  instance stays stopped across `yard restart`, yardd restarts and reboots. Every other
  instance starts when yardd starts, which is at login.
- `yard stop` stops yardd and with it every instance (main spec §6.1).
- Unknown keys are `CONFIG_INVALID` with file, line and column, like the other TOMLs; a
  parse error keeps the last good snapshot serving.

Data layout additions (main spec §3.3):

```
services.toml                         # USER TOML, edited via TomlFile
services/<instance>/data/             # the engine's data directory
services/<instance>/<engine>.conf     # generated (mysql: my.cnf, redis: redis.conf)
services/<instance>/overrides.conf    # USER-OWNED, included last, never rewritten
run/<instance>.sock                   # mysql's socket; postgres's socket dir is run/
logs/<instance>.log                   # the unit's log, rotated like every unit's
nginx/services/minio.conf             # generated: minio.<tld> (§5.4)
```

## 5. Engines

Each engine is one entry in a table in `yard-core` (`services::Engine`, **builder's
call** on the exact shape): its `.yard` key, the versions it accepts and how a version
maps to a formula, the default version, its usual ports, how to initialize data, its unit
command and generated config, its health check, and its `yard env` lines. Adding MariaDB
or Valkey later is one more entry. M4d adds emailulator as one.

All binaries are absolute keg paths under `<prefix>/opt/<formula>/` (homebrew rule);
nothing is looked up on PATH.

| Engine | `.yard` key | Versions | Default (`true`) | Formula | Usual ports |
|---|---|---|---|---|---|
| MySQL | `mysql` | a line brew has a `mysql@<line>` formula for: `"8.0"`, `"8.4"`, `"9.7"` today, plus the plain `mysql`'s line (`"26.10"` today) | `"8.4"` | `mysql@<line>`, else plain `mysql` | 3306 |
| PostgreSQL | `postgres` | `"15"`–`"18"` (`"14"` until brew disables it) | `"17"` | `postgresql@<line>` | 5432 |
| Redis | `redis` | `true` only | — | `redis` | 6379 |
| MinIO | `minio` | `true` only | — | `minio/stable/minio` | 9000 (API), 9001 (console) |

Defaults are **builder's calls**: MySQL's is 8.4, the long-term-support line, not the
newest; PostgreSQL's is the newest but one. A default is used only by `add` and by a
`.yard`'s `true` when no instance of the engine exists yet; it never unmeets anything
(§9).

**Line to formula.** A line resolves the way PHP 8.5 does (homebrew rule): `mysql@8.4`
when brew has it, else the plain formula when its version has that major.minor. `add`
resolves it with `brew info --json=v2`, which also lists the lines brew has; a line brew
does not have is `SERVICE_VERSION_UNSUPPORTED`, listing the ones it has. A deprecated
formula (`mysql@8.0`, `postgresql@14`) installs with a warning naming brew's disable date
**(builder's call)**; a formula brew has already disabled is not offered as a line. `add`
asks brew once per formula, and a winning formula brew lists but cannot describe is
`FORMULA_MISSING` (`brew could not describe <formula>`), not a missing line.

**The plain formula moves.** `brew upgrade mysql` takes the plain `mysql` from 26.10 to
the next line; mysqld upgrades the instance's data in place and cannot go back. The
instance keeps the name it was added with (`mysql-26.10`), and `yard doctor` says it now
runs a newer line. A `.yard` asking for `"26.10"` stays met on this machine; on a machine
where brew has moved on it cannot be added, which is the truth (§9) **(builder's call)**.
Projects that need a stable MySQL use a versioned line.

### 5.1 MySQL

- Initialize: `mysqld --no-defaults --initialize-insecure --datadir=<data> --basedir=<keg>`, so
  `root` has an empty password (main spec §8.2). `--no-defaults` comes first: mysqld reads
  `/etc/my.cnf` and `~/.my.cnf` otherwise, and a `datadir`, `port` or `basedir` in a user's
  file would shape what Yard creates. The instance's own run reads only its `my.cnf`
  (`--defaults-file`).
- Run: `mysqld --defaults-file=<root>/services/<i>/my.cnf`. The generated `my.cnf` sets
  `port`, `bind-address = 127.0.0.1`, `socket = <root>/run/<i>.sock`, `datadir`,
  `mysqlx = OFF` (the X Protocol would otherwise take 33060 for every instance), and
  ends with `!include <root>/services/<i>/overrides.conf`, created empty once.
  The include path is the one unquoted path in Yard's output: mysqld takes the rest of the
  line and keeps a quote as part of the path (checked against mysqld 8.0.33, 2026-10-24).
- Health: TCP connect. Stop: SIGTERM (mysqld shuts down cleanly).
- Legacy PHP: MySQL keeps its defaults; PHP 7.1.16+ connects to it (§16).

### 5.2 PostgreSQL

- Initialize: `initdb -D <data> -U <macOS user> --auth=trust -E UTF8`. The superuser is
  the macOS user with no password, as in Postgres.app **(builder's call)**.
- Run: `postgres -D <data> -c listen_addresses=127.0.0.1 -c port=<p> -c
  unix_socket_directories=<root>/run`. `postgresql.conf` in the data directory is the
  user's file: `initdb` writes it, Yard never touches it, and the `-c` settings win over it.
- Health: TCP connect. Stop: SIGINT (PostgreSQL's fast shutdown) **(builder's call)**.
- Every PostgreSQL formula is versioned, so a `brew upgrade` is never a new major.

### 5.3 Redis

- No initialization, but reconcile creates `services/<i>/data` (redis-server exits at config
  load when `dir` is missing; MySQL and PostgreSQL need theirs absent or empty for init, and
  MinIO makes its own). Run: `redis-server <root>/services/<i>/redis.conf`, generated with
  `bind 127.0.0.1`, `port`, `dir <data>`, `daemonize no`, and a last line
  `include <root>/services/<i>/overrides.conf`.
- Health: TCP connect. Stop: SIGTERM.

### 5.4 MinIO

- **Upstream is archived.** homebrew/core's `minio` is deprecated ("repo archived") and
  stops installing on 2027-02-17; the `minio/stable` tap is frozen at
  `RELEASE.2025-09-06T17-38-46Z`, and the free edition's console is a file browser only.
  The maintainer chose to include it knowing this. Yard installs `minio/stable/minio` so the
  install keeps working after the core formula is disabled **(builder's call)**; it is the
  one M4b engine from a third-party tap, so it goes through the trust gate (§8).
- `.yard` takes `minio = true` only: releases are dated, not versioned.
- Run: `minio server <data> --address 127.0.0.1:<api> --console-address
  127.0.0.1:<console>` with `MINIO_ROOT_USER=minioadmin` and
  `MINIO_ROOT_PASSWORD=minioadmin` (MinIO's defaults; local only).
- Health: TCP connect to the API port. Stop: SIGTERM.
- **`minio.<tld>`** proxies the console. It is a generated nginx file,
  `nginx/services/minio.conf`, not a site: it is not in `sites.toml`, `yard sites` does
  not list it, and it cannot be tunnelled. A linked site named `minio` takes precedence.
  It is HTTPS when `secure_by_default` is on, with a certificate under
  `certs/services/`. The block sends `Host` as the upstream's address when the engine asks
  for it (MinIO's console may check it, as emailulator's does), so M4d's `mail.<tld>`
  reuses this mechanism **(builder's calls)**. The console URL `yard info` and `ServiceInfo`
  show is `http(s)://minio.<tld>` with nginx's port for that scheme (`:8080`/`:8443`, left out
  for 80/443), not the console's own port; it is `https` under `secure_by_default`, and absent
  while a site holds the name. The certificate is `certs/services/minio.pem`, issued by
  reconcile and renewed by the periodic scan like a site's.

## 6. Ports

- Every instance port is a port registry entry named after the instance
  (`mysql-8.4`; MinIO's are `minio` and `minio-console`).
- **Allocation, once, at `add`:** `--port` if given (refused with `PORT_IN_USE` naming the
  holder if busy, with a hint to pass a different `--port` or leave it out); else the engine's usual port; else the next port above it that is
  free and not registered to another unit (3306 → 3307 → 3308). On this machine today
  that gives `mysql-8.4` 3307 (DBngin holds 3306), `postgres-17` 5433 (Postgres.app),
  `redis` 6380 (DBngin), and `minio` 9000/9001.
- **Other servers' usual ports are skipped even while they are down.** A probe only sees
  live listeners, so adding MySQL while DBngin's is stopped would pin 3306 and keep DBngin
  from starting later. While DBngin.app is installed, Yard skips 3306, 5432 and 6379; while
  Postgres.app is installed, 5432. Yard detects them read-only, by the app bundle in
  `/Applications` or `~/Applications`, and reads none of their files. `--port` overrides,
  and once they are uninstalled `yard service port` moves an instance onto the usual port
  **(builder's call)**.
- **The entry is recorded as `pinned`.** The registry's `preferred` mode moves a unit
  back to its preferred port when that frees up (`Registry::ensure`), which would change
  `DB_PORT` under a project's `.env`. A pinned entry never moves; if its port is taken
  while the instance is down, the start fails with `PORT_IN_USE` naming the holder
  (`mysqld (pid 77034, DBngin)`) and does not back off (port-registry rule). The new
  registry function is the "next free at or above" search; no new mode
  **(builder's call)**. This supersedes the `preferred` entries in main spec §7's example.
- **Moving an instance:** `yard service port <instance> <port>` (for MinIO, the API port;
  `yard ports set minio-console <port>` moves the console). It refuses a busy port, or one
  registered to another unit (the console's included), with `PORT_IN_USE` naming the holder
  and a hint at another `yard service port`; then restarts the instance. A stopped instance's
  port moves too and it stays stopped, and asking for its own port changes nothing
  **(builder's call)**. Once DBngin is gone, `yard service port mysql-8.4 3306` puts MySQL
  back on 3306. Yard never does this by itself.
- If a user deletes an instance's entry from `ports.toml`, reconcile pins it again the same
  way (usual port, or the next free one, around the reserved ports) and the instance
  keeps running; an instance whose port cannot be had is a reconcile warning with no unit.
- `yard service remove` drops the instance's registry entries.
- A unit with two ports (MinIO) probes both before it starts, so `UnitSpec.port` became
  `UnitSpec.ports: Vec<(u16, Proto)>`; the first one held fails the unit, without backoff,
  and the conflict names that port and its holder. `minio-console` belongs to the `minio`
  unit (`reconcile::port_owner`), so it shows as `yard` while MinIO runs. M4d needs the same
  for emailulator.

## 7. CLI

### 7.1 Commands

```
yard services                                     list instances
yard service add <engine> [--version <v>] [--port <p>] [--yes]
yard service remove <instance> [--keep-data] [--yes]
yard service start|stop|restart <instance>
yard service logs <instance> [-f] [-n <lines>]
yard service port <instance> <port>
yard env [--site <name>] [--service <instance>]
yard info [<site>]
yard sources
yard source trust <formula> [--tap]
yard source untrust <formula> [--tap]
```

`yard services` prints one row per instance: INSTANCE, ENGINE, VERSION, PORT,
STATE (`running`, `stopped`, `starting`, `failed`), ACCOUNT (§7.5), and USED BY, the
linked sites whose `.yard` asks for it. `--json` gives `ServiceInfo` objects. Output
follows the cli-output rule.

### 7.2 Behaviour

- **`add`** is a job (main spec §6.2: one at a time, progress events, Ctrl-C cancels). It
  resolves the formula; runs the trust gate for a third-party tap (§8); `brew install`s
  the formula if its keg is missing; allocates ports (§6); initializes the data directory
  if it is empty; writes the `services.toml` entry; reconciles; and waits for the
  instance to pass its health check, then prints the `yard env` lines. A job that fails
  or is cancelled after it began removes the entry and the ports it wrote, and what it
  created in `services/<i>/`: the data directory it initialized, and any file that was not
  there before (a `my.cnf` reconcile wrote, say). It never deletes data that was already there
  or a file the user put there first (`overrides.conf`). Adding an instance that exists is
  `SERVICE_EXISTS`: checked by the handler, and again as the job starts, since jobs run one
  at a time and a second add queued behind the first must not touch the first's ports or
  data. Adding into a `services/<i>/data/` left by `remove --keep-data` reuses that data and
  says so (`reusing the data in <dir>`); the init runs only into an absent or empty `data/`.
  Init runs before the entry is written, because reconcile starts only instances that have one
  and runs no init.
- **`remove`** removes the instance's entry and ports, reconciles to stop it, waits until the
  unit is gone (so a running mysqld never writes into a directory being deleted), and then
  deletes `services/<i>/`, or everything in it but `data/` with `--keep-data`. On a terminal it asks `Delete mysql-8.4 and its
  data? [y/N]` (`--yes` skips); without a terminal and without `--yes` it refuses with
  `NEEDS_CONFIRMATION` **(builder's call)**. It refuses with `SERVICE_IN_USE` when a
  linked site's `.yard` asks for it, naming the sites, the way `yard php uninstall`
  refuses a PHP a `.yard` runs. It never runs `brew uninstall`: the keg may be the user's.
- **`stop`/`start`** write or clear `stopped` and reconcile. `start` removes the key rather
  than writing `stopped = false`, so the file is clean again. **`restart`** is
  `units.restart` for the instance; on a stopped instance it refuses (`INVALID_ARGUMENT`, hint
  `yard service start <instance>`) and never clears `stopped` **(builder's call)**.
- **`logs`** prints the last 100 lines of `logs/<instance>.log` (`lines` sets the count, at most
  10000), read from the end of the file in blocks (at most 8 MiB, so one huge line cannot make
  it read the whole file), and nothing while there is no file; `-f` follows it through
  `log_line` events.
- **`port`** is §6.

Settled in M4b task 13 (`yard-cli` `commands/services.rs`) **(builder's calls)**:

- `add` prints the job's lines, then `Added <i>. For your .env:` and the `yard env` lines of
  the instance it created (found as the one new instance of that engine in `services.list`,
  since the daemon picks the default line). With `--json` it prints only the job's events,
  as `php install` does. `--yes` is accepted and does nothing: `add` asks nothing but the
  trust question, which `--yes` never answers.
- `remove` checks the instance before it asks anything (the maintainer's ruling): it reads `services.list`,
  and an unknown instance is `SERVICE_NOT_FOUND`, one a linked site's `.yard` lists is
  `SERVICE_IN_USE` (`<i> is used by blog, shop, from its .yard`, hint `edit <file>` from the
  site's `project_file`, with `data.sites` and `data.files` as the daemon gives them). That
  holds with or without `--yes` and without a terminal, and nothing is asked. Only then does
  it ask, or fail `NEEDS_CONFIRMATION`; the daemon checks again when the delete arrives.
  Declining is `NEEDS_CONFIRMATION` too, as declining `Install these?` is `NEEDS_INSTALL`.
  With `--keep-data` the hint keeps the flag, so it never turns into a delete of the data.
- `start`, `restart` and `port` answer while the instance is `starting`, so the CLI polls
  `services.list` every 200 ms for up to 60 s. `failed` prints the last 10 log lines the
  daemon gave, then `error: <i> failed to start: <last line>` with hint `yard service logs
  <i>` and exits 1; a timeout is the same error code with its own message.
- `logs -n <lines>` sets the tail (the daemon's `lines`). `-f` prints the tail, then every
  `log_line` event for that unit until Ctrl-C, which exits 0; with `--json` each line is a
  `log_line` event on its own line.
- ACCOUNT is `<user> (no password)`, `— (no auth)` or `<user> / <password>`, from the
  `Account` the daemon returns (never a name hard-coded in the CLI). The PORT column is the
  instance's first port (MinIO's API port).

### 7.3 `yard env`

Prints `KEY=value` lines for `.env`, grouped under a `# <instance>` comment per service.
It never writes a file (main spec non-goal) and always prints the real port.

- In a project (or `--site`), it prints the services its `.yard` lists. `--service`
  prints one instance. Outside a project, with no flag, it prints every instance.
- An instance the `.yard` asks for that does not exist yet is a comment naming the fix
  (`# mysql-8.4 is not added: yard service add mysql --version 8.4`).

| Engine | Lines |
|---|---|
| MySQL | `DB_CONNECTION=mysql`, `DB_HOST=127.0.0.1`, `DB_PORT=<p>`, `DB_USERNAME=root`, `DB_PASSWORD=` |
| PostgreSQL | `DB_CONNECTION=pgsql`, `DB_HOST=127.0.0.1`, `DB_PORT=<p>`, `DB_USERNAME=<macOS user>`, `DB_PASSWORD=` |
| Redis | `REDIS_HOST=127.0.0.1`, `REDIS_PORT=<p>`, `REDIS_PASSWORD=null` |
| MinIO | `AWS_ACCESS_KEY_ID=minioadmin`, `AWS_SECRET_ACCESS_KEY=minioadmin`, `AWS_DEFAULT_REGION=us-east-1`, `AWS_ENDPOINT=http://127.0.0.1:<api>`, `AWS_USE_PATH_STYLE_ENDPOINT=true` |

No `DB_DATABASE` or `AWS_BUCKET`: Yard creates neither (maintainer). A project listing both a
MySQL and a PostgreSQL service gets both `DB_*` blocks and a comment saying the second
one overrides the first in a `.env` **(builder's call)**.

Settled in M4b task 11 (`env.print`, `rpc/info.rs`) **(builder's calls)**:

- The lines carry the port the registry pinned the instance to. An instance with no
  registry entry (it has none until reconcile allocates one) gets no lines and a note, never
  the engine's usual port, which is not where it runs.
- A site found by `--site` or by the directory is the innermost linked project that
  contains it, when projects nest. A linked project with no `.yard` asks for nothing, so it
  prints no blocks; a directory in no linked project prints every instance, like no flag.
  `--service` with an unknown instance is `SERVICE_NOT_FOUND`.
- The CLI prints a block as `# <instance>`, then its `# note`, then its lines, with a blank
  line between blocks. A block with a note and no lines is just `# <note>`, which already
  names the instance. With no blocks it prints nothing on stdout, and says why in one line on
  stderr: the site's `.yard` lists no services (it asks `sites.info` which), there are no
  instances yet, or the lookup failed and the line says so. The spec has no flag for "every instance": `yard env` run outside a linked
  project prints them all, and `--service` prints one. It sends the canonicalized
  absolute directory, never a relative path.
- The `both set DB_*` comment goes on the second block whenever two printed blocks both
  set `DB_*` (a project cannot list two MySQL lines, but "every instance" can have two),
  and never on a block with no lines. A missing versioned engine's fix names its line
  even for `true`: `yard service add postgres --version 17`.

### 7.4 `yard info`

The maintainer asked for one command that shows what a project runs on (2026-10-23). `yard status`
already covers the machine (profile, TLD, units), so `yard info` covers one site
**(maintainer agreed to the scope)**: run in a linked project's directory, or `yard info <site>`
from anywhere. Outside a linked project, with no name, it is `SITE_NOT_FOUND` with the
hint `yard info <site>` or `yard link`.

```
$ cd ~/Herd/eepower && yard info
eepower   http://eepower.yard:8080          serving
  path        ~/Herd/eepower
  config      .yard
  php         7.3.33       from .yard
  docroot     public       from .yard
  https       off          default
  extensions  redis, imagick

  services                                   account
  mysql-8.4   running   127.0.0.1:3307       root          (no password)
  redis       running   127.0.0.1:6380       —             (no auth)

  needs       nothing
```

- Each setting names where it came from: `.yard`, `sites.toml` (an isolated or secured
  site without a `.yard`), or `default` **(builder's call)**.
- `php` shows the full version the keg runs, not just the line.
- `services` lists what the `.yard` asks for, each with its instance, state, address and
  account; an instance that does not exist yet shows as a need. A site without a `.yard`
  has no services section. MinIO also shows its console URL.
- `needs` lists unmet needs with the command that fixes each, as the setup page does.
- A proxied site shows its target instead of `php`, `docroot` and `extensions`.
- `--json` gives a `SiteDetail` object: `SiteInfo` plus the per-setting sources, the PHP
  patch version and the `ServiceInfo` of each service.

Settled in M4b task 13 (`yard-cli` `commands/info.rs`) **(builder's calls)**:

- `SiteDetail` carries no extension list, so the human output asks `ext.list` for the site's
  PHP line and shows the extensions Yard added that are enabled (not the bundled ones; `none`
  when there are none). If that call fails the line is left out. `--json` makes no extra call.
- `path` shows the home directory as `~`; `docroot` is relative to the path; `config` is
  `.yard`, or `none` for a site without one. A service the `.yard` asks for that is not added
  shows as `not added` in the table (`mysql-8.4`, `redis`), and `needs` carries the command.
  MinIO's console URL is a `<i> console  <url>` line under the table. A `.yard` that does not
  parse is an `error` line (with its hint) above `needs`.
- The account column is two columns, the user and what follows it (`(no password)`,
  `(no auth)`, `/ <password>`), so the users line up as in the layout above.

Settled in M4b task 11 (`sites.info`, `rpc/info.rs`) **(builder's calls)**:

- A site is found by name or by the innermost linked project containing `path`; neither
  matching is `SITE_NOT_FOUND` with the hint `yard info <site> or yard link`.
- The source of `php`, `docroot` and `secure` follows what the daemon serves: a project
  with a `.yard` owns them, so each is `project_file` where the file sets it and `default`
  where it does not (its `sites.toml` entry is not read then); a project with no `.yard` says
  `sites_toml` where the entry sets it, else `default`. A proxied site has only `secure`.
- `php_full` is the Cellar directory `opt/php@<v>` links to, without brew's `_N` revision
  (`7.3.33_8` is `7.3.33`); it is empty for a proxy, a version that is not installed, or a
  keg that is not a link into the Cellar.
- `services` has one entry per `[services]` key, in engine order, each with the instance that
  meets it (the default line's, else the first by name, for `true`) or none while it is
  missing. A site with no `.yard`, or one that does not parse, has none; the broken file is
  `project_error` on the site.

### 7.5 Accounts

`yard info`, `yard services` and `yard env` show each instance's login: MySQL `root` with
no password, PostgreSQL the macOS user with no password, Redis no auth, MinIO
`minioadmin` / `minioadmin` (maintainer agreed).

That is safe because **Yard holds no secrets**: every account it shows is a default it set
up on a server that listens only on 127.0.0.1. Yard never stores a password the user
chose. If the user sets one themselves (say a MySQL root password), Yard does not learn
it; a failed health login is not something Yard can tell apart, so the account column
keeps showing the default with the note `(as Yard set it up)` **(builder's call)**. If a
later milestone ever stores a real secret, it is hidden by default and shown only on an
explicit flag.

## 8. Brew sources and trust

Homebrew 6.0+ refuses to load a formula from a non-official tap unless it is trusted
(`brew trust --tap|--formula`, stored in `~/.homebrew/trust.json` or under
`$XDG_CONFIG_HOME`). But a fully qualified `brew install tap/name/formula` trusts that
formula itself, without asking, and that is how Yard installs. On this machine,
`php@7.1`, `php@7.3`, `xdebug@8.4` and `apcu@8.4` are trusted one formula at a time,
which is what those silent grants look like. M4b closes that.

- **Brew is the only record (maintainer).** Yard reads trust with `brew trust --json=v1` and
  changes it only by running `brew trust` / `brew untrust`. It never reads or writes
  `trust.json` itself and keeps no list of its own.
- **The gate.** Before any `brew install` of a formula from a tap other than
  homebrew/core — `yard php install`, `yard ext add` (brew extensions), `yard service add
  minio` — Yard checks that the formula or its tap is trusted. An extension brew has no
  formula for is built with pecl, which needs no tap, so before refusing `ext add` looks
  in the tap's files under brew's prefix and lets it through only when they do not list
  the formula, renames and aliases included (`brew search` finds real names only, so it
  would miss `redis@8.4`, a rename of `phpredis@8.4`); names are compared lower-cased, as brew does; a tap that is not there, or a file
  it cannot read (or that is over 1 MiB), keeps the gate. If not trusted:
  - on a terminal the CLI asks `Trust shivammathur/php/php@8.3 (from the
    shivammathur/php tap)? [y/N]`; yes runs `yard source trust` for that formula and
    retries; **(builder's call)**
  - otherwise it fails with `FORMULA_UNTRUSTED`, hint
    `yard source trust shivammathur/php/php@8.3`, and installs nothing.
  - **`--yes` never grants trust**, and neither do `yard init`, `yard link` or any
    install **(builder's call)**. It still skips the "Install these?" question.
- **What counts as agreeing.** `yard source trust` is an explicit act, as `brew trust`
  is, and it works without a terminal. Refusing it off a terminal would add nothing,
  since `brew trust` runs anywhere; what Yard guarantees is that trust is only ever
  granted by a command whose whole purpose is granting it **(builder's call)**. So
  `scripts/e2e-m2.sh`, which installs from `shivammathur/*` unattended, needs its
  formulae trusted first; on this machine they already are.
- **homebrew/core formulae need no step**: brew always trusts them, and its checksum and
  signed-API checks apply. Every M4b database is in core.
- **`yard sources`** lists the taps Yard installs from (`shivammathur/php`,
  `shivammathur/extensions`, `minio/stable`), whether each is tapped and trusted as a
  whole, and the formulae from them that are trusted or installed, with their state.
- **`yard source trust <formula>`** runs `brew trust --formula <formula>`; with `--tap`,
  `brew trust --tap <tap>`. The default is one formula, as Homebrew's docs advise; `--tap`
  covers every version at once **(builder's call)**. `untrust` mirrors it. Both run in
  yardd (the CLI stays a pure client) and print brew's output on failure.
- **Untrusting something installed** does not stop it serving: Yard runs kegs by path.
  brew will no longer load the formula, so installing, upgrading or removing it fails;
  `yard doctor` warns `php@7.1 is installed but no longer trusted` with the
  `yard source trust` hint.
- **Never weaken brew.** Yard never sets `HOMEBREW_NO_REQUIRE_TAP_TRUST`,
  `HOMEBREW_NO_VERIFY_ATTESTATIONS`, `HOMEBREW_NO_INSTALL_FROM_API` or any other
  `HOMEBREW_NO_*` security variable. When brew refuses, Yard's `JOB_FAILED` carries
  brew's own message.
- **Whose environment.** yardd runs brew with launchd's environment, not the user's
  shell, and the CLI forwards no `HOMEBREW_*` variables **(builder's call)**: a forwarded
  variable could switch a check off as easily as on. Brew's own config files
  (`~/.homebrew/brew.env`, `/etc/homebrew/brew.env`) apply in every context, so a policy
  like `HOMEBREW_FORBIDDEN_FORMULAE` belongs there. `yard doctor` warns when the CLI's
  shell sets `HOMEBREW_FORBIDDEN_*` or `XDG_CONFIG_HOME` (which moves brew's trust file)
  and yardd's environment does not, naming `brew.env` as the fix.
- **A brew older than 6.0** has no `brew trust` and no trust requirement. Yard then skips
  the gate and `yard doctor` warns that brew predates tap trust **(builder's call)**.

## 9. `.yard` `[services]`

Fills in project-file spec §4.

```toml
[services]
mysql = "8.4"
postgres = "17"
redis = true
minio = true
```

- Keys: `mysql`, `postgres`, `redis`, `minio` (and `emailulator` from M4d). Any other key
  is `PROJECT_FILE_INVALID` naming the file and key.
- Values: a line from §5's table, or `true`. `"latest"`, a patch release (`"8.4.13"`),
  a table, a port or a number are invalid. MySQL takes major.minor, PostgreSQL a major,
  Redis and MinIO only `true` **(builder's calls)**. A line brew does not have is not a
  parse error; it is a need whose install fails with `SERVICE_VERSION_UNSUPPORTED`.
- **Needs.** A line is met when that instance (`mysql-8.4`) exists in `services.toml`.
  `true` is met by any instance of the engine; when there are several, `yard env` uses
  the default line's if it exists, else the first by name **(builder's call)**. A missing
  one is `Need::Service { engine, version }` (the version is `None` for `true`, and adds
  the default): described as `MySQL 8.4`, fixed by
  `yard service add mysql --version 8.4`. `yard init` / `yard link` list it with
  the other needs and add it after the y/N (or `--yes`), through the same `settle` flow as
  PHP, running services after PHP and extensions. Until it is met the site serves the 503
  setup page, which names the command.
- **A stopped or failed instance does not send the site to the setup page.** The site is
  served; `yard sites` shows the status and `yard doctor` warns `eepower's .yard asks for
  mysql-8.4, which is stopped`. Yard never starts an instance the user stopped
  **(builder's call)**.
- Instances are shared: two projects asking for `mysql = "8.4"` use one instance. Yard
  never removes or stops an instance because a `.yard` stopped listing it.
- `yard init` writes, commented out, one line per engine (`# mysql = "8.4"`,
  `# postgres = "17"`, `# redis = true`, `# minio = true`) under `# [services]`, since Yard
  does not read `.env` (project-file spec §6). `--force` keeps `[services]` as written.

## 10. Doctor

New checks, by id. A check with nothing to say is left out (as `php-constraint:<site>` is),
except `service:<i>` and `service-data:<i>`, which say `ok` for a healthy instance.

- `service:<i>`, one per instance. `fail` for a failed instance: its last lines, hint `yard
  service restart <i>`. `warn` for one yardd has no unit for, one that is not running, or one in
  backoff. `ok` when running, starting, or stopped by the user. With no daemon (the offline
  doctor) only "stopped by the user" is known, so the rest are skipped, as unit state is today.
- An instance's port held by something else: the existing `ports` check (`portscan::infos`),
  which names the instance's pinned entry and the holder (`mysqld (pid 77034, DBngin)`).
- `service-data:<i>`, MySQL and PostgreSQL only, the engines whose init makes `data/`: `fail`
  when `services/<i>/data/` is missing or empty, hint `yard service remove <i> && yard service
  add <engine> --version <line>` (Yard never re-initializes data on its own). Redis's directory is
  made by reconcile and may be empty, and MinIO makes its own, so neither is checked.
- A generated `my.cnf`/`redis.conf` edited by hand: the existing `generated` check, naming
  `overrides.conf`.
- `service-need:<site>`: `warn` for a site whose `.yard` asks for an instance that is stopped
  (hint `yard service start <i>`), failed (hint `yard service restart <i>`; the daemon must be
  running to know) or not added (hint `yard service add <engine> [--version <line>]`; the site's
  `project-file:<site>` check fails for the same thing, with `yard init`). The message names the
  site and the instance: `eepower's .yard asks for mysql-8.4, which is stopped`.
- `service-line:<i>`: `warn` for an instance on the plain `mysql` formula whose keg
  (`opt/<formula>`'s Cellar version, `9.7.1_1` being line `9.7`) is on a different line than the
  instance's name (§5), hint `yard service add mysql --version 8.4`, a line that does not move.
- `service-deprecated:<i>`: `warn` for an instance whose formula brew has deprecated (with its
  disable date) or disabled. One `brew info --json=v2` names the instances' formulae, leaving out
  any brew would refuse: real brew 6 fails the whole call over one formula from an untrusted tap, and
  `info <tap>/<formula>` may tap a tap, which a read-only doctor must not do. So a formula is named only
  when it is homebrew/core's, or its tap is already tapped and trusted (or brew predates trust); when trust
  cannot be read, only homebrew/core's. An untrusted `minio/stable` is `sources:trust`'s to report, and it
  costs the other instances nothing. A brew that cannot answer the call (an unknown formula, a timeout of
  30 s) is one `warn` with id `service-deprecated`, never an error. A core MinIO is noted here, with
  brew's disable date (§15).
- `sources:trust`: `warn`, one per installed formula from a Yard tap that brew does not trust (§8),
  hint `yard source trust <formula>`. "Installed" is `sources::installed`: the PHP kegs, the brew
  extensions in `yard.toml`, and MinIO's tap formula only when `services.toml` records
  `minio/stable/minio` for the instance or, with no MinIO instance, `brew info --json=v2 minio`
  (30 s; a timeout, like any failure, means not the tap's) says the keg is the tap's: a core MinIO (`formula = "minio"`) is never reported as an untrusted
  `minio/stable/minio`. A brew that cannot be asked is a `warn` with the same id.
- `sources:brew`: `warn` for a brew older than 6.0 (§8); the trust check then has nothing to read.
- `sources:env`: `warn` when `client_env` (the CLI's `XDG_CONFIG_HOME` and `HOMEBREW_FORBIDDEN_*`,
  sent as `DoctorParams`, which an older client leaves out) has a key that yardd's environment lacks
  or sets differently, hint `put it in ~/.homebrew/brew.env`. Only the key names are printed, never
  a value. The in-process doctor (no daemon) has the CLI's own environment and skips it.
- `php7.0-mysql:<site>`: `warn` for a PHP 7.0 site whose `.yard` asks for MySQL, naming
  "authentication method unknown" (§16).

Doctor never runs `brew install`, `brew trust` or `brew services`: it only asks. Trust is read once per run
(`brew --version`, `brew trust --json=v1`, under one 30 s bound) and shared by the checks; each `brew info`
has its own 30 s, and the calls are sequential, so a hung brew costs up to the sum. A
`services.toml` that does not parse is the `config` check's to report, and adds no service check.

## 11. Pre-existing fix

Yard's proxy block sends `X-Forwarded-Host $host`, which drops the port, so an app on
8080/8443 that compares `Origin` with it refuses form posts through `yard proxy`. M4b
sends `$http_host` in both the site proxy block and the new service block, since
`minio.<tld>` is built on the same block **(builder's call)**. Found while writing M4d.

## 12. RPC and errors

- Methods (main spec §6.2): `services.list|add|remove|start|stop|restart|logs|port`,
  `env.print { path?, site?, service? }`, `sites.info { path?, site? }`,
  `sources.list|trust|untrust`. `services.add` and
  `sources.trust` are jobs; the rest answer directly.
- Types: `ServiceInfo { name, engine, version, formula, ports, state, account, used_by }`;
  `SiteDetail` (§7.4);
  `Need::Service { engine, version }`; `EnvBlock { instance, lines }`;
  `SourceInfo { tap, tapped, trusted, formulae }`.
- New error codes: `SERVICE_NOT_FOUND`, `SERVICE_EXISTS`, `SERVICE_IN_USE`,
  `SERVICE_VERSION_UNSUPPORTED`, `FORMULA_UNTRUSTED`, `NEEDS_CONFIRMATION`.
- A minor protocol bump: every change is additive.

## 13. Testing

- **yard-core:** engine table (version → formula, defaults, `.yard` value validation),
  rendered `my.cnf`/`redis.conf`/`nginx/services/minio.conf` as `insta` snapshots, the
  "next free at or above" allocation, `yard env` lines, the trust-store JSON parse.
- **yard-daemon:** service units with `yard-fake-unit` standing in for `mysqld`,
  `postgres`, `redis-server` and `minio` under a fake brew prefix; a fake `brew` for
  `info`, `install` and `trust`. Tests never run a real database, never run real brew, and
  never touch `~/.homebrew/trust.json` (as `security` is faked for M3).
- **yard-cli:** `services`, `service *`, `env`, `sources`, `source *` against
  `MockDaemon`, human and `--json`, including the trust prompt and `--yes` not granting it.
- **`scripts/e2e-m4b.sh`** on the maintainer's machine, after `e2e-m1.sh`, asked first: adds
  `mysql-8.4`, `postgres-17`, `redis` and `minio` (installing what is missing, which
  needs the maintainer's approval), checks each answers with its own client by absolute keg path
  (`mysql`, `psql`, `redis-cli`, `curl …/minio/health/live`), checks `yard env` against the
  ports `yard services` reports, links a scratch project whose `.yard` lists them, then
  removes every instance it added. DBngin's, Postgres.app's and Herd's ports and files
  come out unchanged.

## 14. When built

The same change updates the main spec (§3.3 data layout, §6.2 methods, §6.3 codes, §7's
example and allocation for services, §8.2, §10, §12), the project-file spec §4 and §6,
README, `--help` text, AGENTS.md, and the hydra rules: port-registry (pinned service
ports, multi-port probe), homebrew (the trust gate, never `HOMEBREW_NO_*`), config-ownership
(`services.toml`, `services/<i>/overrides.conf`), cli-output and rpc-protocol as their
checklists say. The M4d plan is re-checked against the M4b plan before M4d is built.

## 15. Supervision details

- **Stop timeout.** The supervisor sends SIGKILL 5 s after its stop signal, which can
  cut a database off mid-checkpoint. Service units get 30 s **(builder's call)**.
- **Leftovers.** If yardd is killed, its service processes keep running and hold their
  ports and data directories. On start, before the first reconcile, yardd stops a
  process recorded in `run/<instance>.pid` when it is the instance's own binary under
  `launchd`, the way `NginxCommand::stop_orphan` does for nginx **(builder's call)**.
  `orphans::stop(pid_file, program, grace, signal)` does it, async: the unit's own stop signal
  (SIGINT for an orphaned postgres, its fast shutdown; SIGTERM for the others), SIGKILL after
  `grace` (30 s, a service's stop timeout), then it removes the file. yardd runs it for every
  instance in `services.toml` before the first reconcile. A file that is unreadable, names a dead process or a different program is
  stale and removed too; a live process of that program whose parent is not launchd belongs to a
  running yardd, so it and its file stay. The supervisor writes the file on every spawn
  (`UnitSpec.pid_file`) and removes it when the process is gone, and a unit's own timeout is
  `UnitSpec.stop_timeout` (`None` = the supervisor's 5 s).
- **Which MinIO is installed.** homebrew/core's `minio` and `minio/stable/minio` share
  the `opt/minio` keg, so Yard checks the installed keg's tap (`brew info --json=v2`),
  not only that `opt/minio` exists. A core `minio` already installed is used as it is,
  and `yard doctor` notes brew's disable date **(builder's call)**.

## 16. MySQL and legacy PHP

MySQL 8 logs in with `caching_sha2_password` and defaults to the `utf8mb4_0900_ai_ci`
collation (charset id 255). PHP's mysqlnd understands both from 7.1.16 and 7.2.4; PHP 7.0
does not, and fails with "authentication method unknown" or "charset (255) unknown".

**the maintainer's oldest project runs PHP 7.3 (maintainer, 2026-10-23), so MySQL keeps its defaults.**
Yard sets no `mysql_native_password` and no charset override. `yard doctor` warns when a
PHP 7.0 site's `.yard` asks for MySQL, naming the error it will hit **(builder's call)**.

## 17. Left to the builder

The exact Rust shape of the engine table and `ServiceInfo`; how the fake brew is wired
into `DaemonOptions`; log formatting; the order of rows in `yard services` and
`yard sources`; the wording of messages and hints within the cli-output rule.
