# Yard service recipes: engines as a contract, and engines from files — design

Status: **approved by the maintainer on 2026-11-16; built on main the same day (protocol 2.13), the real-machine check (`scripts/e2e-engines.sh`) still to run. §16 lists what the build changed.** A builder read it cold the same
day; its two questions are answered in §2 (the unknown `.yard` engine, and pinned versions). It changes two M4b decisions (the
engine table is a closed set, and services come only from brew; M4b spec §2, §3 non-goals, §5) and
builds on the M4b service model (`docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md`),
M4e's sleep (`docs/superpowers/specs/2026-10-26-yard-m4e-on-demand-services-design.md`) and `yard db`
(`docs/superpowers/specs/2026-11-15-yard-db-design.md`).

Decisions marked **(maintainer)** are the maintainer's, from the 2026-11-16 conversation. Decisions
marked **(builder's call)** were made while writing this and can be reversed on their own.

## 1. Intent

The maintainer asked for more services (Memcached, Meilisearch "and some other useful ones"), then
asked the better question: how easy is it to add one? Today it is not. The four engines are a
closed `enum Engine` in `yard-core`, matched 245 times across 19 source files (render, daemon,
doctor, sleep, info, db, CLI, Yard.app), so each new engine is a change in a dozen places and a
release.

What he wants is the shape Laravel gives its filesystem: a contract, and a driver that implements
it, so a new backend is one new thing that plugs in **(maintainer)**. And a new service should
need **no recompiling** **(maintainer: "if we can engineer a tool that can easily integrate these
services without recompiling then that's the better one")**. He is the only author of engines
**(maintainer: "it's always for me")**; Yard ships to the team, so the engines Yard itself carries
must work on their Macs too.

## 2. Decisions

- **A contract in Rust, with recipes as one of its drivers (maintainer: approach chosen).** A
  `ServiceEngine` trait in `yard-core` is the contract every engine implements. MySQL, PostgreSQL
  and MinIO stay Rust implementations: MySQL's legacy-PHP login, PostgreSQL's locale and `initdb`,
  `yard db`, and MinIO's tap and keg-owner checks are code, not data. A second implementation,
  the **recipe engine**, is built from a TOML file read at runtime. Adding a recipe needs no
  rebuild and no restart.
- **Recipes may run programs from four sources (maintainer): `brew`, `path`, `download`, `npm`**
  (§5). Services are no longer brew-only. PHP, nginx and dnsmasq stay brew-only (homebrew rule);
  this is about service engines.
- **Two places recipes come from (builder's call).** Yard carries a set of recipes compiled into
  the binary (§8); the user's own live in `~/.yard/engines/*.toml`. A user recipe with the same key
  replaces the carried one. Only these two: Yard never fetches a recipe from the network.
- **Redis becomes a carried recipe (builder's call).** It proves the recipe format against an
  engine the e2e scripts already cover (`e2e-m4b.sh`, `e2e-m4e.sh`), and it removes Redis's arms
  from the code. Existing `redis` instances keep working unchanged (§10).
- **Carried recipes this round (builder's call): Redis, Valkey, Memcached, Meilisearch and
  Typesense** (§8). Typesense is the one from `download`, so each source but `npm` has a carried
  user; `npm` and `path` are proved by test recipes (§13).
- **Nothing on disk changes for existing users.** `services.toml`, `.yard` `[services]` and the
  wire keep naming an engine by its key string (`engine = "redis"`, `redis = true`). The one
  addition is for new `download` and `npm` instances, which record the version they run (§5.1).
- **A `.yard` naming an engine this Mac does not know is an unmet need, not an invalid file
  (builder's call, from the builder check).** Recipes are per machine and a `.yard` is committed, so
  a teammate without the maintainer's own recipe must keep the site and `composer install`. The
  site gets the setup page naming the engine (`ENGINE_UNKNOWN`, hint: add
  `~/.yard/engines/<key>.toml`); the `php` and `composer` shims, which read `.yard` without a
  daemon, ignore `[services]` keys they do not know and never load recipes (§9).
- **An instance keeps the program version it was installed with (builder's call, from the builder
  check).** A Yard update that bumps a carried recipe's pin, or an edit to the user's own, never
  breaks a running instance or exposes its data to a newer program on its own (§5.1).
- **Scope is service engines. Three kinds of pluggable thing (maintainer, 2026-11-16):**
  *services* (daemons a project connects to: this design), *runtimes* (per-project versioned tools
  behind shims: PHP, Composer, Node; the next design, Composer first, since brew's Composer 2.3+
  cannot run under PHP 7.0 and 7.1 and those projects need 2.2 LTS), and the *server* (nginx and
  dnsmasq, shared by every site; a contract only if an alternative such as Caddy is wanted, after
  runtimes). FrankenPHP is a runtime, not a server: a second driver beside php-fpm for the process
  that runs PHP, which nginx would proxy to over HTTP instead of FastCGI (maintainer); it brings its
  own PHP build and extensions, not brew's. It is tabled (maintainer).
  Runtimes reuse this design's recipe format and sources where they fit. Tunnel providers may
  follow the same pattern later.

## 3. Goals, non-goals, anti-goals

Goals:

- A new engine is one TOML file in `~/.yard/engines/`. `yard service add <key>`, a `.yard`'s
  `<key> = true`, `yard env`, `yard info`, `yard services`, doctor, sleep and Yard.app's Add picker
  all see it within about 2 s, with no restart.
- The engine contract is the only place the rest of Yard learns engine facts: no code outside
  `services/engines/` matches on an engine's key.
- A recipe mistake is reported with its file and line, and never stops Yard from serving sites or
  running other instances.

Non-goals:

- Sharing or installing recipes from anywhere (a registry, a URL, a git repo).
- Recipes for MySQL, PostgreSQL or MinIO, or `yard db` list/create/dump/import for a recipe engine.
- Release lines for recipe engines: a recipe engine has one instance, named for its key, as Redis
  and MinIO do today (§11, left for later).
- An HTTP health check (TCP only this round, §6.4).
- Intel Macs (main spec non-goal): a `download` names one Apple Silicon build.

Anti-goals (a failure even if it works):

- A recipe that can make Yard run something from PATH, through a shell, or from Herd's, DBngin's or
  Postgres.app's folders.
- A broken user recipe that takes down sites, other instances, or yardd.
- A Redis instance that behaves differently after the move to a recipe (ports, data, config,
  overrides, sleep, `yard env`, `yard db connect`).
- Built-ins and recipes looking different to the user: the CLI and Yard.app treat every engine the
  same way.

## 4. The contract

`yard-core/src/services/engines/` holds the trait, the registry and the implementations
(`mysql.rs`, `postgres.rs`, `minio.rs`, `recipe.rs`). The trait's exact shape is the **builder's
call**; it must carry everything the `match` arms carry today:

| Fact | Today | On the contract |
|---|---|---|
| key, display name | `Engine::key`, `yard_proto::engine_display` | `key()`, `display()` |
| source | `base_formula`, line resolution | `source()` (§5); release lines stay MySQL's and PostgreSQL's |
| ports | `usual_ports` | `ports()`: names and usual ports, the first is the main port |
| account | `account` | `account(macos_user)` |
| data setup | `init_command`, reconcile's Redis `data` dir | `init(ctx)`, `dirs()` |
| run | `command`, `config_file`, `user_overrides` | `command(ctx)`, `config_file(ctx)`, `user_overrides()` |
| supervision | `start_timeout`, stop signal, `Health::Tcp` | `start_timeout()`, `stop_signal()`, `health()` |
| `.env` | `env_lines` | `env_lines(ctx)` |
| console | MinIO's `minio.<tld>` | `console()`: which port, and whether to send the upstream `Host` |
| clients | `supports_db`, `supports_connect`, app's `connect_clients` | `databases()` (built-ins only), `connect()`: URL, shell command, client hints |
| sleep | every instance may sleep | `may_sleep()` |

`ctx` carries the instance name, the install root, `Paths`, the instance's registry ports and the
macOS user. The registry is built from the built-ins plus every valid recipe, and is published the
way the reconcile snapshot is, so a reload never blocks a reader **(builder's call)**. `Engine`
stops being an enum: an engine is named by its key (a validated newtype), and code that needs facts
asks the registry. A key the registry does not know (a recipe deleted while an instance uses it) is
a state the code must handle (§7).

## 5. Sources

Every source ends in an **install root** (`{home}` in a recipe) and an absolute program. Nothing is
looked up on PATH, nothing runs through a shell.

| Source | Recipe says | Install root | `yard service add` does |
|---|---|---|---|
| `brew` | `formula = "valkey"` (a tap's formula in full: `typesense/tap/typesense-server`) | `<prefix>/opt/<last segment>` | `brew install`, behind brew's trust gate for a tap (M4b §8), exactly as today |
| `path` | `program = "/abs/path/to/bin"` | none; the command uses `{program}` | checks it exists and is executable; installs nothing |
| `download` | `version`, `url`, `sha256`, optional `strip` (leading path parts to drop) | `~/.yard/programs/<key>/<version>/` | downloads, checks the hash, unpacks |
| `npm` | `package`, exact `version`, `node` (a major or exact version) | `~/.yard/programs/<key>/<version>/` | installs with Yard's Node (M4c) |

- **`brew`** keeps every M4b rule: `brew info --json=v2`, a deprecated formula's warning with
  brew's disable date, a disabled one refused, a tap's formula refused with `FORMULA_UNTRUSTED`
  until `brew trust`, `--yes`, `init` and `link` never granting trust. Unversioned: the formula
  brew has is the one used.
- **`path`** is refused when the path is not absolute, does not exist, or lies under `~/Herd`,
  Herd's `~/Library/Application Support/Herd`, `/Applications/Herd.app`, `/Applications/DBngin.app`,
  DBngin's `~/Library/Application Support/com.tinyapp.DBngin`, or `/Applications/Postgres.app`
  (`PATH_NOT_ALLOWED`) **(builder's call)**. The program is the user's; Yard never installs, updates
  or removes it.
- **`download`** is HTTPS only and needs `sha256`; a recipe without it does not load. Yard runs
  `/usr/bin/curl -fsSL` into a staging directory beside the install root, checks the SHA-256 with
  `sha2` (already a dependency), unpacks with `/usr/bin/tar -xf` (which reads `.tar.gz`, `.tgz`,
  `.tar.xz` and `.zip`; a file that is none of them is a bare program and is made executable), and
  renames the stage into place, so a cancelled or failed download leaves nothing. A hash mismatch is
  `DOWNLOAD_HASH_MISMATCH` naming both hashes. The file is not quarantined (curl sets no
  `com.apple.quarantine`), so Gatekeeper is not involved; Yard does not check signatures.
- **`npm`** needs the recipe's Node installed in Yard's fnm (`FNM_DIR=~/.yard/node`, M4c spec): the
  add job installs it the way `yard node install` does when it is missing, since adding the service
  is the user asking for it **(builder's call)**. It runs that Node's own `npm` by absolute path:
  `npm install --prefix <stage> --no-save --no-audit --no-fund <package>@<version>`, with PATH set
  to that Node's `bin` first (an npm bin's `#!/usr/bin/env node` must find Yard's Node, never
  nvm's or Herd's) and `npm_config_python=/usr/bin/python3` (Machines notes: brew's Python has no
  distutils for node-gyp). Install scripts run: the recipe is the user's own. A version range is
  refused when the recipe loads (`version` must be exact), so the same recipe installs the same
  thing. The unit runs with the same PATH.

### 5.1 Versions over time

- A `download` or `npm` instance records the version it was installed with as `version` in its
  `services.toml` entry (`version = "30.1.2"`), and keeps running `~/.yard/programs/<key>/<version>/`
  whatever the recipe pins later. For these entries `formula` is absent. Brew and `path` entries
  are unchanged: brew's keg moves with `brew upgrade` as it does today, and a `path` program is the
  user's. `yard services` and `ServiceInfo` show the recorded version.
- When the recipe pins a different version, `yard services` and doctor (`engine-version:<i>`, `info`
  level) say so, naming both. `yard engines install <key>` installs the pinned version and moves the
  instance to it (rewriting its `version`, restarting it); that is the only way an instance moves.
  The recipe's current `command` is used with the instance's recorded install root: a recipe whose
  flags changed between versions is the author's to keep compatible **(builder's call)**.
- An instance whose recorded install root is missing (deleted by hand) shows `failed` with
  `ENGINE_NOT_INSTALLED` and the hint `yard engines install <key>`. Yard never installs on its own at
  reconcile, at start or after an update: an install is a job the user starts.
- `yard service remove` and `yard engines install` delete install roots that no instance records and
  that are not the current pin of any recipe **(builder's call)**.

## 6. The recipe file

One file per engine, `~/.yard/engines/<key>.toml`; the file's stem must be its `key`. Unknown keys
are refused (`deny_unknown_fields`), as Yard's other TOMLs do. An example of every field:

```toml
schema = 1                       # the recipe format's version; required
key = "meilisearch"              # [a-z][a-z0-9-]*; not mysql, postgres, minio, emailulator
name = "Meilisearch"             # what people see
description = "Search engine for Laravel Scout"

[source]
kind = "brew"
formula = "meilisearch"
# kind = "download": version = "30.1.2", url = "https://…", sha256 = "…", strip = 1
# kind = "npm":      package = "@soketi/soketi", version = "1.6.1", node = "18"
# kind = "path":     program = "/Users/me/bin/thing"

[ports]                           # in order; the first is the main port
main = 7700                       # registry entry "<instance>"
# console = 8108                  # any other name: "<instance>-<name>"

[run]
command = ["{home}/bin/meilisearch", "--http-addr", "127.0.0.1:{port.main}",
           "--db-path", "{data}", "--env", "development", "--no-analytics"]
env = { MEILI_NO_ANALYTICS = "true" }
stop_signal = "TERM"              # TERM (default), INT, QUIT
start_timeout = "30s"             # default: the supervisor's grace
dirs = ["data"]                   # made by reconcile under services/<i>/
init = []                         # optional one-time command, run when data/ is empty
sleep = true                      # default true; false never sleeps it

[run.config]                      # optional generated file, rewritten by reconcile
file = "meilisearch.toml"
text = """
…{port.main}…
{include_overrides}
"""
overrides = "# USER-OWNED — Yard never rewrites this file.\n"   # seeds overrides.conf once

[health]
tcp = "main"

[console]                         # optional: <key>.<tld>, proxied by nginx (M4b §5.4)
port = "main"
host_header = "upstream"          # or "client" (default)

[env]                             # yard env's lines for this instance
lines = ["MEILISEARCH_HOST=http://127.0.0.1:{port.main}", "MEILISEARCH_KEY="]

[account]                         # shown by yard info and connect; optional
user = ""
password = ""

[connect]                         # optional: yard db connect / shell and Yard.app's Connect…
url = "http://127.0.0.1:{port.main}"
shell = []                        # e.g. Redis: ["{home}/bin/redis-cli", "-p", "{port.main}"]
clients = ["Any HTTP client: the URL above"]
```

### 6.1 Placeholders

`{home}` (install root; not for `path`), `{program}` (`path` only), `{data}`
(`services/<i>/data`), `{dir}` (`services/<i>`), `{run}` (`<root>/run`), `{instance}`,
`{port.<name>}`, `{conf}` (the generated config's path), `{overrides}` (`services/<i>/overrides.conf`),
`{include_overrides}` (only in `run.config.text`: the line the recipe writes as its include, given
by `run.config.include`, e.g. `include {overrides}` for Redis), `{user}` (macOS user), `{node}`
(`npm` only: Yard's Node binary). A placeholder that does not exist, or one used where it has no
value, fails the recipe at load with its line. Placeholders are filled as whole values, never
re-parsed: a path with a space stays one argument.

### 6.2 Load-time checks

A recipe loads only if: it parses; `schema` is one Yard knows; the stem is the key; the key is
valid and not a built-in's or a Yard command's name (§9.1); every placeholder is known for its source; `command[0]` expands to
something under the install root (or is `{program}`, or `{node}`); `ports` has one to eight entries
with distinct names and usual ports 1024–65535; `health.tcp` and `console.port` name a port;
`download` has an `https://` URL and a 64-hex `sha256`; `npm`'s `version` is exact. Whether the
program exists is not a load check (it is the install's job) **(builder's call)**.

### 6.3 Binding

Yard cannot force a third-party program onto 127.0.0.1; the recipe's command must say so. Doctor
checks each running recipe instance's listeners through `lsof` and warns `service-exposed:<i>`
when one listens on a wildcard or non-loopback address **(builder's call)**.

### 6.4 Health and supervision

Health is a TCP connect to the named port, as for every engine today. Stop, backoff, failure and
the orphan sweep (M4b §15) are the supervisor's, unchanged. Logs go to `logs/<instance>.log` as
today.

## 7. Loading, watching and a broken recipe

- yardd loads the carried recipes, then `~/.yard/engines/*.toml`, at start and whenever that
  directory changes (the watcher that follows `nginx/overrides/`, about 2 s), then reconciles.
- A user recipe that fails to load is reported (`ENGINE_RECIPE_INVALID`, `<file>:<line>: <why>`)
  by `yard status`, `yard doctor` (`engine:<key>`) and Yard.app, and **the last good version of
  that recipe stays in force** for an instance already using it, as nginx keeps the last good
  config **(builder's call)**. With no last good version (first load, or yardd started with it
  broken), the key is unknown and §7's next rule applies. A broken user recipe that shadows a
  carried one falls back to the carried one.
- An instance whose engine is unknown (its recipe deleted or never valid) is listed by
  `yard services` as `failed` with `ENGINE_UNKNOWN` and the hint to restore
  `~/.yard/engines/<key>.toml` or `yard service remove`. Its data is never touched, and
  `yard service remove` still works on it.
- A changed recipe changes the instance's unit spec; the supervisor restarts the unit as for any
  other changed unit spec. A recipe change never re-runs `init` on data that exists.

## 8. Carried recipes

They live in the repo as `crates/yard-core/engines/*.toml` and are compiled in with `include_str!`
**(builder's call)**: one source for a dev build and a release, no bundle path to find. A unit test
loads each one. The builder writes each from the program's own docs and checks it on a real Mac.

| Key | Source | Main port | `yard env` | Notes |
|---|---|---|---|---|
| `redis` | brew `redis` | 6379 | `REDIS_HOST`, `REDIS_PORT`, `REDIS_PASSWORD=null` | today's `redis.conf` and `overrides.conf`, byte for byte (§10) |
| `valkey` | brew `valkey` | 6379 | the same `REDIS_*` lines (Laravel's redis client talks to it) | `valkey.conf` like Redis's |
| `memcached` | brew `memcached` | 11211 | `MEMCACHED_HOST`, `MEMCACHED_PORT` | `-l 127.0.0.1`, `-U 0`; in memory, so a sleep empties it (a cache, so acceptable) |
| `meilisearch` | brew `meilisearch` | 7700 | `MEILISEARCH_HOST`, `MEILISEARCH_KEY={secret}` | `--env development` with `--master-key {secret}` (§16), dumps and snapshots under its data dir, console at `meilisearch.<tld>` (its dashboard, which asks for the key) |
| `typesense` | download, pinned (30.2) | 8108, peering 8107 | `TYPESENSE_HOST`, `TYPESENSE_PORT`, `TYPESENSE_PROTOCOL=http`, `TYPESENSE_API_KEY={secret}` | `--api-key {secret}`, `--data-dir {data}`, `--api-address 127.0.0.1`, `--peering-address 127.0.0.1`, no CORS (§16) |

`yard env` gives connection lines only, never a driver choice (`CACHE_STORE`, `SCOUT_DRIVER`):
picking the driver is the project's **(builder's call)**. Valkey and Redis share 6379 as their
usual port; the registry gives the second one the next free port, as with any taken port.

**Meilisearch's data is version-bound.** A `brew upgrade meilisearch` across a data-format change
makes it refuse to start on its old data. The instance then shows failed with Meilisearch's own
log line; the data is a search index the project rebuilds (`php artisan scout:import`), so the fix
is `yard service remove meilisearch` and add again. The recipe does not pass
`--experimental-dumpless-upgrade` **(builder's call)**.

## 9. CLI

- `yard service add|remove|start|stop|restart|port|sleep <key>` take any engine's key, unchanged.
- `yard engines` lists every engine: key, name, source, where it came from (`built-in`, `carried`,
  `~/.yard/engines/<file>`), whether its program is installed, and its error if it failed to load.
  `--json` as every list.
- `yard engines check <file>` loads a recipe file without installing it and prints its errors or
  the instance it would make (command line, ports, `yard env` lines). It works with no daemon.
- `yard engines install <key>` installs (or reinstalls) the recipe's program as a job, for a
  changed `download` or `npm` version or a removed `path` program's replacement.
- `.yard` `[services]` takes any key that is a valid engine key. Whether the engine exists is
  decided when the need is checked, not when the file is parsed: an engine the registry does not
  know is an unmet need (`ENGINE_UNKNOWN`, §2), so the file stays valid on every Mac. A key that is
  not a valid engine key (`My Service`) is still `PROJECT_FILE_INVALID`. The shims skip
  `[services]` keys they do not know and load no recipes; `yard init`, `yard info` and the setup
  page ask yardd.
- `yard db` is replaced by per-engine commands (§9.1).

### 9.1 Per-engine commands (maintainer, 2026-11-16)

`yard db` is too abstract for a Yard that runs many kinds of service **(maintainer)**. Each engine
gets its own command for *using* it; *running* it stays in `yard service add|remove|start|stop|…
<instance>` **(maintainer)**, since an instance name (`mysql-8.4`) already names its engine and
`yard services` lists them all in one place.

```
yard mysql                      # the mysql prompt on the project's MySQL (yard db shell today)
yard mysql list|create|dump|import|connect|open   [--service mysql-8.0] [--site <s>]
yard postgres …                 # the same verbs; `yard pgsql` is an alias
yard redis                      # the redis-cli prompt; yard redis connect
yard meilisearch open           # opens meilisearch.<tld>
yard <recipe-key> …             # whatever the recipe supports
```

- **The verbs follow the contract (builder's call).** An engine with `databases()` has `list`,
  `create`, `dump` and `import`, with today's `yard db` flags and behaviour unchanged (db spec); an
  engine with `[connect]` has the prompt (the bare command, also `shell`) and `connect`; an engine
  with a console, or a `connect` URL macOS can open, has `open`. A verb the engine lacks is a usage
  error naming the verbs it has. Extra arguments after `--` go to the tool, as with `yard db`.
- **Which instance:** the db spec §5 rule, limited to the command's engine: `--service` (which must
  be an instance of that engine), else what the project's `.yard` asks for, else the machine's only
  instance of that engine.
- **Every engine the CLI knows gets one, recipes included.** The CLI loads the carried recipes and
  `~/.yard/engines` itself (as `yard engines check` does), so `yard redis` works with yardd down
  as far as the verb allows. `yard --help` lists the engine commands in their own section.
- **A recipe key may not be one of Yard's command names** (`link`, `env`, `info`, `service`,
  `engines`, …, and `db` and `pgsql`): the load check refuses it, so an engine never shadows a Yard
  command (builder's call). A test keeps the reserved list equal to the CLI's commands.
- **`yard postgres`, not `yard pgsql`, is the name (builder's call):** it is the engine's key in
  `.yard` and `services.toml`. `pgsql` (Laravel's driver name) is an alias.
- **`yard db` goes away over one release (builder's call).** It shipped in 0.1.8: the release that
  brings the engine commands keeps `yard db <verb>` working and prints `yard db is going away: use
  yard <engine> <verb>` on stderr; the release after removes it. Yard.app and `scripts/e2e-db.sh`
  move to the new commands in the same change.
- **MariaDB,** when it comes, is a built-in with MySQL's database support and gets `yard mariadb`
  with no CLI work.

Help text and README move with these (Docs In Step).

## 10. Redis's move

The Redis recipe must reproduce today's behaviour exactly: the generated `redis.conf` (byte for
byte, so the reconcile writes nothing on the first run after an upgrade), `overrides.conf` seeded
the same and never rewritten, `services/<i>/data` created by reconcile, the supervisor's start
grace, SIGTERM, TCP health, `yard env`'s three lines, `yard db connect`'s URL and clients, and
`yard db shell`'s `redis-cli`. A snapshot test compares the recipe's rendered unit and file with
the current code's output before the Rust arms are deleted.

## 11. Protocol and Yard.app

Protocol 2.13 **(builder's call on names)**:

- `engines.list` returns each engine's key, name, description, source kind, origin, installed,
  error, ports, whether it has a console, `connect`, and whether `yard db` supports it.
- `engines.install { key }` is a job; it also moves the instance to the pinned version (§5.1).
- `ServiceInfo` gains `engine_name` (the display name), so clients stop using
  `yard_proto::engine_display`, which keeps its table only for an older yardd.
- Event: `engines.changed` after a reload, so Yard.app's picker follows a new recipe.
- New error codes: `ENGINE_UNKNOWN`, `ENGINE_RECIPE_INVALID`, `ENGINE_NOT_INSTALLED`,
  `DOWNLOAD_HASH_MISMATCH`, `PATH_NOT_ALLOWED`.

Yard.app's Add service picker lists `engines.list` instead of its own list, shows a recipe
engine's load error in the Services screen, and its Connect… takes `connect.clients` from the
engine instead of `yard-app-model::db::connect_clients`'s table. Its screens stay as they are
otherwise; a recipe engine looks like a built-in.

Release lines for recipe engines, an HTTP health check, and making other parts of Yard pluggable
are left for later.

## 12. Doctor

New checks: `engine:<key>` (a recipe that failed to load, with its line), `engine-program:<i>` (an
instance's program missing, with the `yard engines install` or `brew reinstall` hint),
`service-exposed:<i>` (§6.3), `engine-version:<i>` (§5.1), and `programs-unused` (install roots nothing names, with the size,
`info` level). The existing per-instance checks (`service:<i>`, `service-data:<i>`) take any
engine.

## 13. Testing

- `yard-core`: recipe parsing and every load check (a table of broken recipes, each with its
  expected error and line); placeholder expansion (spaces, unknown names, wrong source); the
  carried recipes all load; the Redis recipe's snapshot against the old code (§10).
- `yard-daemon`: recipe engines run on `yard-fake-unit` (as the built-ins' tests do), covering add,
  sleep and wake, a recipe edited while running, a recipe broken while running (the last good one
  stays), a recipe deleted (`ENGINE_UNKNOWN`, data untouched). `download` against a local HTTPS
  stand-in or a `file://` URL allowed only under `cfg(test)` **(builder's call)**, with a good
  and a bad hash and a cancel; `npm` against a local package tarball so tests need no network;
  `path` with an allowed and a refused path; a pin changed under a running instance (it keeps its
  version until `engines.install`); a `.yard` naming an unknown engine (setup page, shims run).
- `scripts/e2e-engines.sh` (real machine, maintainer's approval): adds `valkey`, `memcached`,
  `meilisearch` and `typesense`, asks each its own question (`valkey-cli PING`, memcached's
  `version` over TCP, `GET /health` on the two search engines), checks `yard env` and the
  `meilisearch.<tld>` console, a scratch `path` recipe and a scratch `npm` recipe (a small exact
  package with a bin that listens on a port), sleep and wake on one of them, and removes all it
  added. It stops before changing anything when one of its instances exists, backs up and restores
  `services.toml`, `ports.toml` and `sites.toml`, and fails if Herd's resolver or the listeners on
  3306, 5432 and 6379 changed. `e2e-m4b.sh` and `e2e-m4e.sh` must pass unchanged against the
  Redis recipe.
- Yard.app's `checklist.sh` gains an item that drops a recipe into the scratch root's `engines/`
  and finds it in the Add picker without a restart.

## 14. Docs that move

The M4b spec's §2, §3 (the "MariaDB, Valkey, MongoDB, Meilisearch and other engines" non-goal) and
§5 point here. AGENTS.md's Status, Key decisions (services) and Build & test (`e2e-engines.sh`),
README (`yard engines`, writing a recipe), DEVELOPMENT.md, `--help` text and the `homebrew` rule
(services may come from other sources) change with the build.

## 15. Left to the builder

The trait's exact methods and how the registry is shared; the `Engine` newtype's name; the order
in which the built-ins move onto the trait; each carried recipe's exact flags (checked on a real
Mac); Typesense's pinned version and hash; the `engines.*` method and event names; how the `npm`
source's Node install reuses M4c's code; the size limit for a download (a sane cap, refused above).

## 16. What the build changed (2026-11-16)

Decided while planning and building, each recorded in the plan's ledger with its reason:

- **Names.** The engine set is `Engines` (`Snapshot.registry` is the port registry). `key()` and
  `display()` borrow; the trait gained `health_port()`, `has_connect()` and `needs_secret()`.
- **Loading.** Recipes load at the start of every reconcile from a `RecipeLoader` in reconcile's
  state; the watcher only reconciles on a change in `~/.yard/engines`. `status` carries them as
  `StatusInfo.engine_errors`. The merge (`Engines::with_user`) is pure, in yard-core, so the offline
  doctor, the start-up orphan sweep and the CLI read the same set.
- **An unknown `.yard` engine is `Need::Manual`** (what: the service this Mac has no recipe for;
  command: add `~/.yard/engines/<key>.toml`), not a new `Need` variant.
- **The recipe format (§6) gained:** `{{`/`}}` for literal braces (a lone `}` or unclosed `{` is an
  error); `{name:quoted}` (double-quoted, `\` and `"` escaped, as Redis's conf needs);
  `run.config.comment` (default `#`, empty for none), with Yard heading every generated recipe
  config with the line naming its `overrides.conf` (or "Generated by Yard from the <key> recipe");
  the main port is the one named `main` (TOML tables keep no order); and `{secret}`.
- **`{secret}` (after an automated security review):** a recipe that names it gets the instance's
  own random key, 32 hex characters in `services/<i>/secret` (mode 0600, written atomically), made
  before anything renders; an empty or damaged file is replaced, and an instance whose recipe needs
  a secret never starts without a valid one. Typesense's API key and Meilisearch's master key are
  it, `yard env` prints it, and Typesense runs without CORS: with a fixed key and CORS, or no master
  key, any web page in a local browser could read or change their data.
- **Typesense** is 30.2's Apple Silicon download (`strip = 0`), with `--api-address`/`--api-port`
  (its strings mark `listen-*` deprecated) and a second port, `peering`, on loopback.
- **CLI.** `yard <engine> <verb>` re-parses its arguments as `yard db <verb>`, so the verbs keep
  `yard db`'s flags; `--service` of another engine is yardd's refusal (`INVALID_ARGUMENT`); `open`
  on an engine with a console opens the console; client lines come from `ConnectInfo.clients`.
  `yard engines check` shows a stand-in home and `<secret>`.
- **Doctor** has no info level, so `engine-version` and `programs-unused` are `ok` checks with a
  hint.
- **Wake.** A sleeping recipe instance wakes as any does (a PHP request, the shims, `yard service
  start`), not on a bare TCP connection: nothing listens while it sleeps.
- **Ports.** An instance's other ports are `<instance>-<name>`, owned by the longest instance name
  that fits (`port_owner` takes the services list), so removal works for an unknown engine. Needs and
  `yard env` blocks come out in key order.

