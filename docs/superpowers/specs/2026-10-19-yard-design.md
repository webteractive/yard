# Yard — design spec

Date: 2026-10-19
Status: approved; revised 2026-10-20 — nginx runs as the user, and 80/443 are an
opt-in (§3.1, §5.2, §5.5, §7, M1.1)

Yard is a local PHP development environment for macOS that replaces Laravel Herd.
The CLI and all logic are written in Rust; the desktop app (menu bar + window) is
written in Swift/SwiftUI and is a pure client of the Rust daemon.

## 1. Goals

1. **Replace Herd on the author's machine.** Serve the ~30 sites in `~/Herd`
   day to day; Herd is uninstalled once Yard is solid.
2. **Fix Herd's pain points:**
   - Cannot add PHP versions needed by legacy projects (need 7.0–7.3).
   - Cannot add arbitrary PHP extensions (Herd ships static PHP builds).
   - Config is not owned by the user / gets overwritten.
   - Services, Node management and similar features are behind a paywall.
3. **Ship to the team, later the public** — signed, notarized, auto-updating.
4. **Learning project** for Rust and Swift; code quality and clear boundaries
   matter, not just "it works".

### Success criteria

- Every site the maintainer links serves over `https://<site>.test` with its correct PHP version
  (on 443 with the `privileged` profile, on 8443 without it).
- A PHP 7.1 legacy site and a PHP 8.4 site run side by side.
- `yard ext add redis --php 7.1` and `yard ext add xdebug --php 8.4` work.
- MySQL, Redis and Mailpit run under Yard with no paid tier.
- Herd can be uninstalled with no loss of function.

### Non-goals (v1)

- Linux or Windows support.
- Intel Macs (arm64 only for v1; revisit at distribution time).
- PHP older than 7.0.
- Building our own PHP binaries (we lean on Homebrew; see §4).
- Replacing nginx/dnsmasq with Rust (possible later behind the daemon API).
- Writing to project `.env` files.

## 2. Assumptions and dependencies

- macOS 14+ (Sonoma), Apple Silicon.
- **Homebrew is required** on every machine. PHP comes from the
  `shivammathur/php` tap, extensions from `shivammathur/extensions`,
  plus brew's `nginx`, `dnsmasq`, `fnm`, `mysql`, `mysql@8.0`, `postgresql@16`,
  `redis`, `mailpit`, `minio`.
- Homebrew is a package source only, never a source of truth. Yard never uses
  `brew services` and never edits brew-owned config files.

## 3. Architecture

### 3.1 Processes

```
 yard (CLI) ─┐
             ├─ JSON-RPC over unix socket ─▶ yardd (LaunchAgent, runs as user)
 Yard.app ───┘                                  │  supervises: nginx, php-fpm pools, dnsmasq,
                                                │  services (mysql, redis, mailpit, …)
                                                │
                                                └─ narrow socket ─▶ yard-helper
                                                                    (LaunchDaemon, root)
                                                                    /etc/resolver, CA trust,
                                                                    80/443 sockets (opt-in)
```

- **`yardd`** owns all state and logic. Long-running, Tokio-based. Its
  LaunchAgent label is `dev.more.yard.daemon`; the helper's is
  `dev.more.yard.helper`.
- **nginx runs as the user**, master and workers, supervised by `yardd` like every
  other unit. Root never runs nginx and never reads its config. A root nginx
  reading a user-owned config is a no-password path to root for anything running
  as the user (nginx config can `include` any file and `load_module`), and nothing
  nginx does needs root except binding 80/443 on loopback.
- **`yard-helper`** runs as root and accepts only a fixed verb set:
  `resolver.install`, `resolver.remove`, `ca.trust`, `ca.untrust`,
  `ports.bind`, plus a read-only `status`.
  It takes no free-form paths or commands: the owning user, Yard's data
  directory and the Homebrew prefix are fixed when the helper is installed, and
  the remaining arguments (TLD, port) are validated. It listens on
  `/var/run/dev.more.yard.helper.sock` and serves only the owning user's uid.
- **`ports.bind`** exists only for the `privileged` profile (§7). It accepts port 80
  or 443 and nothing else, binds that port on `127.0.0.1` and `::1`, and returns
  the listening sockets to `yardd` as `SCM_RIGHTS` on the helper socket. `yardd`
  passes them to the nginx it starts (nginx's `NGINX="fd;fd;…"` inheritance), so
  root opens two sockets and does nothing else. macOS lets any user bind a low
  port on the wildcard address but not on loopback, and listening on the wildcard
  would put every site on the network, so this is the one job root does for nginx.
- The only verb that reads from the data root is `ca.trust` (M3), which reads
  the CA certificate. The data root must be absolute, and every directory and
  symlink on its path, as written, must be owned by the user or root, with no
  directory writable by group or others (sticky parents such as `/tmp` aside);
  the install and `ca.trust` check this, since another user who could write the
  root could get their own certificate trusted.
- The helper is installed by `sudo yard helper install` (binary to
  `/Library/PrivilegedHelperTools/`, plist to `/Library/LaunchDaemons/`), which is
  the only path while there is no app (M1–M3). Yard.app registers the same
  helper through `SMAppService` from M4. Upgrading from M1, whose helper ran
  nginx as root, the install first asks the old helper to stop that nginx and
  then removes the two files it left owned by root (`logs/nginx-error.log`,
  `run/nginx.pid`); it removes rather than chowns them, so a hard link planted
  there cannot hand a root file to the user.
- **`yard`** is a thin client. A few commands work offline without the daemon:
  `start`, `stop`, `version`, `doctor`, `ports check`, `helper install|uninstall`.
- **Yard.app** holds no business logic; everything it can do, the CLI can do.

### 3.2 Repository layout

```
yard/
  Cargo.toml                 # workspace
  crates/
    yard-core/               # domain: sites, php, ports, config rendering, state files
    yard-proto/              # API types (serde) + JSON Schema generation
    yard-daemon/             # yardd binary: supervisor, registry, RPC server
    yard-helper/             # root helper binary
    yard-cli/                # yard binary
  app/                       # Swift package/Xcode project for Yard.app
  docs/
```

`yard-core` does pure work (parse, render, decide) and is where most unit tests
live. Side effects (spawning, binding, brew) live in `yard-daemon`.

### 3.3 Data layout

Root: `~/Library/Application Support/Yard/`

```
yard.toml                     # global: default php, tld override, secure_by_default
sites.toml                    # links, per-site php, secure flag, docroot overrides, proxies
ports.toml                    # port registry and active profile (§7)
state/hashes.json             # hash of every generated file as Yard last wrote it
dnsmasq/dnsmasq.conf          # generated
php/<ver>/php.ini             # generated
php/<ver>/fpm.conf            # generated
php/<ver>/conf.d/             # generated extension inis (50-<ext>.ini)
php/<ver>/conf.d/99-overrides.ini   # USER-OWNED, never written by Yard except via `php config set`
nginx/nginx.conf              # generated
nginx/sites/<site>.conf       # generated
nginx/overrides/<site>.conf   # USER-OWNED, included inside the site's server block
nginx/default/index.html      # generated site list served for unknown hosts
ca/yard-ca.pem, ca/yard-ca.key
certs/<site>.pem, certs/<site>.key
services/<name>/data/         # service data dirs
logs/                         # rotated logs per unit
run/                          # sockets, pid files
bin/                          # shims: php, composer, pecl, phpize, php-config, node, npm, npx
yardd.sock
```

The helper's socket is not here: it lives in `/var/run` so a root process never
creates files inside a directory the user can rewrite.

**Config ownership rule:** every generated file begins with a
`# Generated by Yard — edit <overrides path> instead` header and has a
user-owned sibling that Yard includes but never rewrites. Yard stores a hash of
each generated file; `yard doctor` reports generated files that were hand-edited.

## 4. PHP versions and extensions

### 4.1 Versions

- Supported: 7.0, 7.1, 7.2, 7.3, 7.4, 8.0–8.5.
- `yard php install <ver>` → `brew install shivammathur/php/php@<ver>`, streamed
  as a job (§6.2). Installed versions are read from brew's kegs, not registered in
  `yard.toml` (decided 2026-10-20), so a version installed with brew directly counts too.
- `yard php uninstall <ver>` → `brew uninstall`, as a job. It is refused with
  `PHP_IN_USE`, before brew runs, while the version is the default or any site is
  isolated to it.
- `yard php default <ver>` sets the global default (`default_php` in `yard.toml`);
  the version must be installed.
- `yard php list` shows each installed version (and the default if it is not
  installed), whether it is the default, the sites on it and its pool's state.
- FPM is launched by `yardd`, never by brew:
  ```
  <brew prefix>/opt/php@<ver>/sbin/php-fpm --nodaemonize \
      -y php/<ver>/fpm.conf -c php/<ver>/php.ini
  env PHP_INI_SCAN_DIR=<brew etc>/php/<ver>/conf.d:<yard>/php/<ver>/conf.d
  ```
  Brew-installed extensions load automatically; Yard's `conf.d` is scanned last,
  and `99-overrides.ini` sorts last within it, so user overrides always win.

### 4.2 FPM pools

- One pool per PHP version, socket at `run/php-<ver>.sock`.
- `pm = ondemand`, `pm.max_children = 10`, `pm.process_idle_timeout = 30s`.
- A pool is started only when at least one site resolves to that version, and
  stopped when none do.

### 4.3 Extensions

`yard ext add <name> [--php <ver>]` (default: global default version) resolves in order:

1. `brew install shivammathur/extensions/<name>@<ver>` if the formula exists.
2. `pecl install <name>` using that version's `pecl`/`phpize`
   (`<brew prefix>/opt/php@<ver>/bin/pecl`).
3. `yard ext add <path/to/ext.so> --php <ver>` copies the binary into
   `php/<ver>/ext/` and loads it by absolute path.

Yard then writes `conf.d/50-<name>.ini`, restarts the version's pool, and
verifies with `php -m`. A failed verify rolls back the ini and reports the
`php -m` error output.

Other commands: `yard ext list [--php]`, `yard ext enable|disable <name> [--php]`
(toggles the ini without uninstalling), `yard ext remove <name> [--php]`,
`yard xdebug on|off [--php]`.

### 4.4 php.ini

Generated defaults (dev-friendly): `memory_limit=512M`,
`upload_max_filesize=1G`, `post_max_size=1G`, `max_execution_time=0` (CLI) /
`300` (FPM), `display_errors=On`, `error_reporting=E_ALL`,
`opcache.enable=1`, `opcache.validate_timestamps=1`, `opcache.revalidate_freq=0`,
`date.timezone` from the system.

`yard php config set <key> <value> [--php <ver>]` and
`yard php config get|unset` edit `99-overrides.ini` only, line by line so the user's
comments and other lines stay, and reload that version's pool gracefully (SIGUSR2).
Keys are directive names (`[A-Za-z0-9_.]`); a value is one line without `"`, and is
quoted when it holds spaces or `;`.

### 4.5 Version resolution and isolation

For a site, the version is the first match of:
1. `sites.toml` isolation (`yard isolate <ver> [--site <name>]`).
2. The global default.

For the CLI shims (current working directory), the version is the first match of:
1. The isolation of the site containing the cwd.
2. A `.yard-php` file in the cwd or any parent.
3. The global default.

`composer.json` `require.php` is advisory only: `yard doctor` warns when the
resolved version does not satisfy it.

Isolating a site regenerates its nginx config to `fastcgi_pass` the target
version's socket, ensures that pool is running, and reloads nginx. The version is
`php = "<ver>"` in the site's `sites.toml` entry; it must be installed, and only a
linked site can have one (a proxy runs no PHP). `yard unisolate [--site]` removes it,
and a pool no site resolves to stops.

Shims in `bin/` (`php`, `composer`, `pecl`, `phpize`, `php-config`) are small
Rust binaries (one multicall binary, dispatch on `argv[0]`) that resolve the
version and `exec` the brew binary. `composer` runs the composer phar under the
resolved PHP. `yard install-shims` prints the PATH line for the user's shell
profile; Yard does not edit dotfiles.

## 5. Serving, DNS, HTTPS

### 5.1 Sites

- **Explicit only (decided 2026-10-20):** a project is a Yard site only when it
  has been linked. Yard never serves a directory just because of where it sits.
  M1 shipped Herd-style parking (`yard park`/`unpark`, every directory inside a
  parked path became `<dirname>.<tld>`); M2 removed it. A `parked` key left in an
  M1 `yard.toml` is ignored with a warning. Moving a project from Herd is
  `yard link` (and `yard isolate`) run in it by hand.
- **Linked:** `yard link [name]` serves the cwd as `<name>.<tld>`;
  `yard unlink [name]`. Nothing is written into the project.
- **Proxied:** `yard proxy <name> <url>` serves `<name>.<tld>` as a reverse proxy
  (Node, Vite, etc.); `yard unproxy <name>`.
- **Docroot detection** order: `public/`, `web/`, `public_html/`, project root.
  Override: `yard docroot <path> [--site]`.
- `yardd` watches `yard.toml`, `sites.toml` and `ports.toml` with FSEvents; an edit
  reconciles (debounced 500 ms), so a hand-added `[sites.<name>]` is served.

### 5.2 nginx

- Brew `nginx`, run by `yardd` as the user in the foreground (`daemon off;`),
  reloaded with `SIGHUP`. It stops when `yardd` stops.
- Every config change runs `nginx -t` (as the user, from `yardd`) before
  reload; on failure the previous config set is restored and the error returned.
- `yardd` watches `nginx/overrides/`. An edited override is checked with
  `nginx -t` and reloaded; a rejected one leaves nginx serving the old config
  and is reported as `NGINX_CONFIG_INVALID` by `status` and `doctor` until it
  is fixed. `yard restart nginx` checks the config first and refuses a broken one.
- Per-site server block: front-controller `try_files $uri $uri/ /index.php?$query_string`,
  static files served directly, websocket upgrade headers, `client_max_body_size 1G`,
  `fastcgi_read_timeout 3600` (step debugging), `include nginx/overrides/<site>.conf`.
- Default server returns an HTML page listing known sites for unknown hosts.

### 5.3 DNS

- Brew `dnsmasq`, supervised by `yardd`, listening on `127.0.0.1:53535`
  (port from the registry), answering `*.<tld>` → `127.0.0.1` and `::1`.
- `yard-helper resolver.install` writes `/etc/resolver/<tld>` pointing at it.
  Only the Yard TLD is affected; system DNS is untouched.
- The TLD is `yard` until `yard takeover` and `test` after it; a `tld` in
  `yard.toml` sets it. It is independent of the port profile (§7). It must match
  `^[a-z][a-z0-9-]{0,62}$`, or the file is `CONFIG_INVALID` at its line.
- `resolver.install` and `resolver.remove` only touch files carrying Yard's
  marker comment, so Herd's `/etc/resolver/test` is never overwritten or
  deleted.

### 5.4 HTTPS

- First run: generate a CA (`rcgen`, ECDSA P-256, 10-year validity) in `ca/`;
  `ca.key` is mode 0600. `yard-helper ca.trust` adds it to the System keychain as
  a trusted root (one admin prompt).
- `yard secure [site]` issues a leaf cert (825-day validity, SAN = site host and
  `*.<site host>`), switches the server block to 443 with an 80→443 redirect.
  `yard unsecure [site]` reverses it.
- `secure_by_default = true` in `yard.toml` secures every current and future site.
- Certs within 30 days of expiry are renewed automatically by `yardd`.
- `yard doctor` prints `NODE_EXTRA_CA_CERTS=<path>` for Node and can enable
  Firefox's `security.enterprise_roots.enabled` policy.

### 5.5 Herd coexistence and migration

- **Before takeover:** TLD `yard` on the `standard` profile (8080/8443), so Herd
  keeps `.test`, 80/443 and `/etc/resolver/test`, and both run side by side.
- Yard never moves to a TLD whose `/etc/resolver/<tld>` lacks Yard's marker:
  the change is refused with `RESOLVER_FOREIGN` (naming Herd for `test`) and the
  previous TLD keeps serving, even when Herd is quit. Takeover is the only path
  that moves `.test` to Yard; how it treats Herd's resolver file is M3's to
  decide, and it asks first.
- No `yard migrate herd` (dropped 2026-10-20): Yard does not import Herd's
  config. Each project moves by running `yard link` in it.
- `yard takeover`: stops Herd (quits Herd.app and its services), moves the TLD
  to `test`, re-issues certs and reloads. It then offers the `privileged` profile
  for URLs without a port, running `yard ports check` against it first.
  `yard takeover --undo` moves back to TLD `yard` on the `standard` profile and
  tells the user to restart Herd.

## 6. Daemon

### 6.1 Supervisor

- Every managed process is a `Unit` with `start`, `stop`, `reload` (optional),
  `health`, and a log sink.
- Health: socket connect (FPM, mysql socket) or TCP connect (registered port),
  every 5 s. Known gap: in the privileged profile yardd holds nginx's listeners, so a
  connect succeeds into their backlog even with no nginx serving. An HTTP check closed
  it but flaked the suite (unexplained resets on the inherited sockets), so it waits
  for a check that does not depend on them.
- Stop: SIGTERM, then SIGKILL after 5 s. A running nginx gets SIGQUIT instead, its
  graceful shutdown; a unit that has not passed a health check yet always gets SIGTERM.
- Crash → restart with exponential backoff (1 s → 30 s). Five failures within
  60 s → `failed` state, retains the last 50 log lines, emits an event; no further
  restarts until `yard restart <unit>`.
- Logs: `logs/<unit>.log`, rotated at 10 MB, keep 5.
- **Reconcile loop:** on startup and on any state change, `yardd` reads the TOML
  files, renders all configs in memory, diffs against disk, writes changed
  files, and restarts or reloads only affected units. Reconcile is idempotent.
  A failed reconcile keeps the last good state serving; its error is kept and
  shown by `status` (`config_error`) and `doctor` until a reconcile succeeds.
  A linked project whose docroot (e.g. `public/`) appears later is picked up by
  the periodic scan.
- On `yardd` shutdown, all child units are stopped, nginx included.

### 6.2 API

- JSON-RPC 2.0 over `yardd.sock` (mode 0600, owned by the user).
- Request/response methods (non-exhaustive, same surface as the CLI):
  `status`, `doctor`, `sites.list|link|unlink|proxy|isolate|secure|unsecure|docroot`,
  `php.list|install|uninstall|default|config.get|config.set|config.unset`,
  `ext.list|add|remove|enable|disable`, `xdebug.set`,
  `ports.list|check|set|pin|unpin|range|profile`,
  `services.list|add|remove|start|stop|restart|logs`,
  `node.list|install|use`, `units.restart`, `takeover`, `env.print`.
- Long operations (brew installs, takeover) return `{ job_id }` immediately;
  progress arrives as `job_progress` events (one per output line) and the result as
  one `job_finished` event (`error` set on failure, `JOB_FAILED` with the last 20
  lines in `data.tail` when a command exits non-zero), on the `jobs` topic. Jobs run
  one at a time, in start order. A client subscribes to `jobs` before it calls.
- Subscriptions (`subscribe` with topic list: `units`, `sites`, `logs`, `ports`,
  `jobs`) push events tagged `unit_state_changed`, `site_added`, `site_removed`,
  `job_progress`, `job_finished`, `log_line`, `port_conflict`.
- Types are defined once in `yard-proto` (serde). `cargo xtask schema` emits
  JSON Schema; the Swift `Codable` models are generated from it and checked in
  CI for drift.
- Protocol version is exchanged on connect; mismatched major versions are
  refused with a clear upgrade message.

### 6.3 Errors

Every error is `{ code, message, hint, data? }`. Codes are stable strings, e.g.
`PORT_IN_USE`, `PHP_NOT_INSTALLED`, `PHP_IN_USE`, `JOB_FAILED`, `BREW_MISSING`, `NGINX_CONFIG_INVALID`,
`HELPER_NOT_INSTALLED`, `EXT_VERIFY_FAILED`. The CLI prints message and hint;
the app shows them inline with an action button when the hint maps to a command.

### 6.4 CLI

- Every command supports `--json` (machine-readable, stable shape from
  `yard-proto`); default output is human tables/text.
- If `yardd` is not running, commands that need it print
  `yardd is not running → yard start` and exit 3 (with `--json`, the error code is
  `DAEMON_NOT_RUNNING`). `doctor` and `ports check` run in-process instead.
- `yard --version` and `yard version --json` report CLI, daemon, helper and
  protocol versions plus build commit.

## 7. Port registry

All ports Yard uses or routes to are recorded in `ports.toml`. No unit binds a
port the registry has not granted.

```toml
range = "33000-33999"
auto_fallback = false
profile = "standard"   # or "privileged"

[ports]
nginx-http   = { port = 8080,  mode = "pinned" }
nginx-https  = { port = 8443,  mode = "pinned" }
dnsmasq      = { port = 53535, mode = "pinned" }
mysql-8-0    = { port = 3306,  mode = "preferred", preferred = 3306 }
redis        = { port = 6379,  mode = "preferred", preferred = 6379 }
mailpit-smtp = { port = 1025,  mode = "preferred", preferred = 1025 }
mailpit-http = { port = 33001, mode = "auto" }
"proxy:api"  = { port = 3000,  mode = "external" }
```

**Modes**

- `pinned`: exactly this port. If unavailable, the unit does not start.
- `preferred`: try `preferred` first. If unavailable: with `auto_fallback`,
  allocate the lowest free port in `range` and record it; otherwise fail with
  `PORT_IN_USE` and the hint to set a port or enable fallback.
- `auto`: allocated from `range`.
- `external`: not bound by Yard (proxy targets). Tracked so Yard never allocates
  it and can warn when the target is not listening.

**Profiles:** `standard` (nginx on 8080/8443, the default) and `privileged`
(nginx on 80/443, opt-in) are named sets of pinned values; `yard ports profile
<name>` switches them. Only `privileged` uses root: `yardd` asks the helper's
`ports.bind` for the two sockets when it first needs them, holds them while the
profile stays `privileged` (every nginx start inherits them; `nginx -t` binds
nothing, so it needs none), and closes them when it switches back, when it stops,
and while nginx is `failed`, so a connection is refused rather than left waiting and
something else may take the port (§3.1). The switch itself is refused unless the
helper is installed and hands the sockets over. Neither
profile sets the TLD (§5.3). M1's `trial` and `default` are read as `standard`
and `privileged`.

**Checks**

1. **Allocation:** the registry rejects two units on the same port; a
   `ports.toml` with two non-`external` entries on one port is
   `CONFIG_INVALID` at the later entry's line.
2. **Pre-start:** a real bind probe on `127.0.0.1` and `::1`; for 80 and 443,
   which a user may not bind on loopback, a connect (free when nothing answers),
   and in the `privileged` profile the helper's `ports.bind` is the check. Before
   the first nginx start, `yardd` stops an nginx an earlier, killed `yardd` left
   running (the master in `run/nginx.pid`, if it is brew's nginx and launchd is
   its parent). On failure, the
   holder is identified via `lsof`, which reads libproc as the user (pid, process name, parent; labelled as
   Herd, brew services, Docker, or unknown when recognisable) and returned in
   `PORT_IN_USE.data`.
3. **Runtime:** health probes detect a unit losing its port; a scan every 30 s
   detects foreign listeners on registered-but-idle ports and emits
   `port.conflict`.

**Commands:** `yard ports` (table: unit, port, mode, status, holder),
`yard ports check` (dry-run probe of all registered ports, also part of
`doctor` and run before `takeover`), `yard ports set <unit> <port>`,
`yard ports pin|unpin <unit>`, `yard ports range <a-b>`,
`yard ports profile <name>`.

Changing a port restarts only that unit and reloads dependents (nginx for
proxies and `mail.<tld>`). `yard env` always prints the actual assigned ports.

## 8. Node and services

### 8.1 Node

- Wraps brew `fnm`. `yard node install <ver>`, `yard node list`,
  `yard node use <ver> [--site <name>]`.
- `node`/`npm`/`npx` shims resolve: site setting → `.nvmrc` / `.node-version`
  in cwd or parents → global default.

### 8.2 Services

Each service is a supervised `Unit` with data in `services/<name>/data/` and its
port(s) in the registry.

| Service | Formula | Notes |
|---|---|---|
| MySQL 8.4 / 8.0 | `mysql`, `mysql@8.0` | one instance per version; `root` with empty password, local only |
| PostgreSQL 16 | `postgresql@16` | |
| Redis | `redis` | |
| Mailpit | `mailpit` | SMTP on registry port; UI proxied at `mail.<tld>` |
| MinIO | `minio` | optional; console proxied at `minio.<tld>` |

Commands: `yard service add <name> [--version] [--port]`,
`yard service remove <name> [--keep-data]`, `yard service start|stop|restart|logs <name>`,
`yard services` (list).

Services bind `127.0.0.1` only. `yard env [--site]` prints `.env` lines
(`DB_*`, `REDIS_*`, `MAIL_*`) using actual ports; it never writes files.

## 9. Yard.app

- SwiftUI, macOS 14+, Swift 6 strict concurrency.
- **Menu bar** (`MenuBarExtra`): health indicator, quick toggles for services
  and Xdebug, recent sites (open in browser), badges for failed units and port
  conflicts.
- **Main window** (`NavigationSplitView`):
  - Sites: search, per-site PHP picker, secure toggle, open in browser /
    editor / Terminal, "Edit overrides" (opens the override file).
  - PHP: installed versions, install with progress, extensions per version,
    `99-overrides.ini` editor.
  - Services: start/stop, version, port, live logs, "Copy .env".
  - Ports: registry table with resolve actions on conflicts.
  - Logs: unified filterable stream.
  - Doctor: checks with fix buttons.
- **Client:** one `YardClient` actor speaks JSON-RPC over the socket; models are
  generated from `yard-proto` schema. `@Observable` stores update from event
  subscriptions — no polling.
- **Onboarding:** check Homebrew → install CLI symlink and `yardd` LaunchAgent →
  register helper via `SMAppService` → offer
  takeover, and with it 80/443.
- The app bundle carries `yard`, `yardd`, `yard-helper` and the shim binary in
  `Contents/Helpers/`.

## 10. Testing

- **yard-core:** unit tests for config rendering (snapshot tests with `insta`
  for nginx, fpm, php.ini, dnsmasq), version resolution, port allocation, Herd
  config parsing (fixtures copied from a real Herd install, secrets-free).
- **yard-daemon:** integration tests with fake units (scripts that bind ports,
  crash, or hang) covering supervision/backoff, port conflicts and holder
  identification, reconcile diffs, job events. Never binds 80/443.
- **yard-cli:** command output tests (human and `--json`) against a mock daemon.
- **End-to-end** (local + macOS CI runner): `standard` profile, install PHP 8.3 and
  7.1, link a fixture Laravel app and a 7.1 app, `curl` both over
  `https://<site>.yard:8443` and assert `PHP_VERSION`; add and verify an
  extension. M1 runs a first cut over HTTP until M3 (`scripts/e2e-m1.sh`: `standard`
  profile on :8080, a linked PHP 8.4 app and a proxy, Herd's resolver unchanged,
  and the 80/443 profile refused with Herd named).
- **Swift:** `YardClient` against a mock socket server; store tests driven by
  event streams; schema drift check in CI.

## 11. Distribution

- Developer ID signed, notarized `.dmg` of Yard.app (arm64). Helper registered
  with `SMAppService`.
- Team updates via appdater; public updates via Sparkle if/when public.
- Homebrew tap formula for CLI-only installs (`yard`, `yardd`, helper, shims).
- Details follow the existing macOS release-signing and appdater conventions at
  the time of M6.

## 12. Milestones

Each milestone is usable on its own and gets its own implementation plan.

1. **M1 Core serving (CLI only):** workspace, `yardd`, helper, port registry,
   nginx, dnsmasq, parked/linked/proxied sites, single PHP version, the
   standard profile, `doctor`.
   **M1.1 nginx as the user (before M2, done 2026-10-20):** nginx becomes a `yardd` unit run as the
   user; the helper's nginx verbs go and `ports.bind` arrives with the
   `privileged` profile; profiles stop setting the TLD.
2. **M2 Multi-PHP:** install/uninstall, pools per version, isolate, shims,
   extensions, overrides, `php config`, and removing parking
   (sites are explicit links only, §5.1).
3. **M3 HTTPS:** CA, secure/unsecure, secure_by_default, renewal, `takeover`.
4. **M4 Yard.app:** client, menu bar, main window, onboarding.
5. **M5 Node and services:** fnm wrapper, MySQL, Postgres, Redis, Mailpit,
   MinIO, `yard env`.
6. **M6 Distribution:** signing, notarization, appdater, brew tap.
