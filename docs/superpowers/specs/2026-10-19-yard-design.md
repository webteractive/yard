# Yard — design spec

Date: 2026-10-19
Status: approved; revised 2026-10-20 — nginx runs as the user, and 80/443 are an
opt-in (§3.1, §5.2, §5.5, §7, M1.1)

Yard is a local PHP development environment for macOS that replaces Laravel Herd.
The CLI and all logic are written in Rust; the desktop app (menu bar + window) is
written in Rust with GPUI (maintainer, 2026-11-06; until then SwiftUI) and is a pure client of the
Rust daemon.

## 1. Goals

1. **Replace Herd on the author's machine.** Serve the ~30 sites in `~/Herd`
   day to day; Herd is uninstalled once Yard is solid.
2. **Fix Herd's pain points:**
   - Cannot add PHP versions needed by legacy projects (need 7.0–7.3).
   - Cannot add arbitrary PHP extensions (Herd ships static PHP builds).
   - Config is not owned by the user / gets overwritten.
   - Services, Node management and similar features are behind a paywall.
3. **Ship to the team, later the public** — signed, notarized, auto-updating.
4. **Learning project** for Rust (Swift until 2026-11-06, when the maintainer made Yard.app Rust and
   GPUI too); code quality and clear boundaries matter, not just "it works".

### Success criteria

- Every site the maintainer links serves over `https://<site>.test` with its correct PHP version
  (on 443 with the `privileged` profile, on 8443 without it).
- A PHP 7.1 legacy site and a PHP 8.4 site run side by side.
- `yard ext add redis --php 7.1` and `yard ext add xdebug --php 8.4` work.
- MySQL and Redis run under Yard, and emailulator is reachable through Yard, with no paid
  tier.
- Herd can be uninstalled with no loss of function.

### Non-goals (v1)

- Linux or Windows support.
- Intel Macs (arm64 only for v1; revisit at distribution time).
- PHP older than 7.0. The `shivammathur/php` tap still carries 5.6, but the maintainer tabled it on
  2026-11-01: his oldest project is on 7.3, and the work goes into making 7.0–8.5 solid.
- Node older than 16 (no Apple Silicon build; it would need fnm's x64 builds under Rosetta),
  tabled with PHP 5.6 on 2026-11-01.
- MariaDB as a service engine. The maintainer wants it later (2026-11-01); it is one more engine table
  entry (M4b spec §5) when it comes.
- Building our own PHP binaries (we lean on Homebrew; see §4).
- Replacing nginx/dnsmasq with Rust (possible later behind the daemon API).
- Writing to project `.env` files.

## 2. Assumptions and dependencies

- macOS 14+ (Sonoma), Apple Silicon.
- **Homebrew is required** on every machine. PHP comes from the
  `shivammathur/php` tap, extensions from `shivammathur/extensions`,
  plus brew's `nginx`, `dnsmasq`, `fnm`, `mysql`, `mysql@8.0`, `postgresql@16`,
  `redis`, `minio`.
- **emailulator** (ours, `emailulator 0.1.3` on 2026-10-21) is the one service that does not
  come from brew: it is installed by its own release installer into `~/.local/bin` and keeps
  itself current with `emailulator self-update`. Yard finds it on the user's PATH and never
  installs or updates it.
- Homebrew is a package source only, never a source of truth. Yard never uses
  `brew services` and never edits brew-owned config files.

## 3. Architecture

### 3.1 Processes

```
 yard (CLI) ─┐
             ├─ JSON-RPC over unix socket ─▶ yardd (LaunchAgent, runs as user)
 Yard.app ───┘                                  │  supervises: nginx, php-fpm pools, dnsmasq,
                                                │  services (mysql, postgres, redis, minio)
                                                │
                                                └─ narrow socket ─▶ yard-helper
                                                                    (LaunchDaemon, root)
                                                                    /etc/resolver,
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
  `resolver.install`, `resolver.remove`, `ports.bind`, plus a read-only `status`.
  Trusting Yard's CA is not root's job (§5.4).
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
- **`wake.sock`** (M4e) is yardd's second socket, `run/wake.sock` (mode 0600, the user's),
  beside the JSON-RPC `yardd.sock`. It answers minimal HTTP/1.1 on one route,
  `GET /wake/<site>?wait=<s>`, with `204` (or `403` when another program holds a port the
  site's instances need, so nothing talks to it): nginx's `auth_request` before a PHP request
  and the `php`/`composer` shims ask it to wake the sleeping service instances a site's
  `.yard` lists. It can only start instances that exist and the user has not stopped; it
  adds, changes and removes nothing (M4e spec §5.1).
- No verb reads from the data root. The install checks that the data root is
  absolute and that every directory and symlink on its path, as written, is owned by
  the user or root, with no directory writable by group or others (sticky parents
  such as `/tmp` aside).
- The helper is installed by `sudo yard helper install` (binary to
  `/Library/PrivilegedHelperTools/`, plist to `/Library/LaunchDaemons/`), which is
  the only path until M7: Yard.app in M6 hands that command to Terminal, and registers the
  same helper through `SMAppService` from M7, which needs a signed app (M6 design §18 Q1). Upgrading from M1, whose helper ran
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
    yard-shim/               # php/composer/pecl/phpize/php-config and node/npm/npx shims (one multicall binary)
  app/                       # Yard.app (M6, GPUI): planned as a second Cargo workspace, M6 design §4
  docs/
```

`yard-core` does pure work (parse, render, decide) and is where most unit tests
live. Side effects (spawning, binding, brew) live in `yard-daemon`.

### 3.3 Data layout

Root: `~/.yard/` (moved from `~/Library/Application Support/Yard/` by the maintainer on 2026-10-25, so
no path Yard writes or runs from has a space in it; `YARD_ROOT` or `--root` overrides it).
yardd keeps the root `0700` at every start, since a home folder is often readable by other
local users and the old location inherited `~/Library`'s `0700`.

```
yard.toml                     # global: default php, default_node, tld override, secure_by_default, [php."<ver>".extensions], [tunnels.cloudflared] domains (M5)
sites.toml                    # links, per-site php and node, secure flag, docroot overrides, proxies, [sites.<site>.tunnel] (M5)
ports.toml                    # port registry and active profile (§7)
services.toml                 # the service instances (§8.2), one [services."<instance>"] table each, and [emailulator] path (M4d)
state/hashes.json             # hash of every generated file as Yard last wrote it
state/php-modules.json        # each PHP keg's compiled-in modules (`php -n -m`), keyed by its canonical path
state/tunnels.json            # M5, 0600: each named tunnel's Cloudflare id, name, credentials path and routed hosts, and the DNS records left (dns_left)
dnsmasq/dnsmasq.conf          # generated
php/<ver>/php.ini             # generated
php/<ver>/fpm.conf            # generated
php/<ver>/conf.d/             # generated: brew's conf.d mirrored (10-brew-<file>.ini) and extension inis (50-<ext>.ini)
php/<ver>/conf.d/99-overrides.ini   # USER-OWNED, never written by Yard except via `php config set`
php/<ver>/pecl.conf           # generated before each pecl run: a PEAR user config (§4.3)
nginx/nginx.conf              # generated
nginx/sites/<site>.conf       # generated; a tunnelled site's also has its tunnel listener on 127.0.0.1 (M5)
nginx/overrides/<site>.conf   # USER-OWNED, included inside the site's server block
nginx/tunnel-overrides/<site>.conf  # USER-OWNED, included only in the site's tunnel listener (M5)
tunnels/<site>.yml            # generated: the tunnel's cloudflared config (a managed directory, M5)
nginx/services/<name>.conf    # generated: minio.<tld>, the console proxied, and emailulator.conf for mail.<tld> (a managed directory)
nginx/overrides/mail.conf     # USER-OWNED, included in mail.<tld>'s server block (M4d)
nginx/default/index.html      # generated site list served for unknown hosts
nginx/setup/<site>.html       # generated setup page for a site whose .yard is not met (503)
ca/yard-ca.pem, ca/yard-ca.key
certs/<site>.pem, certs/<site>.key
certs/services/<name>.pem, .key    # certificates for service hostnames (minio.<tld>, and mail.<tld> as emailulator.pem, once secured)
services/<instance>/data/     # the engine's data dir; never re-initialized or deleted by Yard except `service remove`
services/<instance>/my.cnf    # generated (MySQL), or redis.conf (Redis); PostgreSQL and MinIO have none
sites/<name>/.yard            # USER-OWNED managed .yard (M4a.1): written by `yard init`, edited by isolate/secure/docroot, never deleted by `unlink`
sites/<name>/.yard-backup     # the managed file `yard init --replace` replaced
services/<instance>/overrides.conf  # USER-OWNED, created once, included by my.cnf (seeded with [mysqld]) / redis.conf / nginx's minio block; PostgreSQL has none (its data/postgresql.conf is the user's)
logs/                         # rotated logs per unit
run/                          # sockets, pid files (run/<instance>.pid for a service)
run/wake.sock                 # M4e: nginx and the shims ask yardd to wake a site's services
run/sleeping                  # generated (M4e): the linked sites that need a sleeping instance, emptied at yardd start
bin/                          # shims: php, composer, pecl, phpize, php-config, node, npm, npx
node/                         # FNM_DIR (M4c): written only by fnm, which Yard runs to install and uninstall
node/lts.toml                 # generated: LTS codename → major, refreshed on each install
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
  brew exits 1 when it pours a keg but cannot link it, which `php` (8.5) does while a
  `php@<v>` keg owns `bin/php` (brew links a keg-only `php@<v>` when no `php` is installed).
  Yard runs PHP from its keg and never needs that link, so the job succeeds with a line
  saying so when the keg is there afterwards; a cancelled job stays cancelled (found
  2026-11-06 on the Mac mini).
- `yard php uninstall <ver>` → `brew uninstall`, as a job. It is refused with
  `PHP_IN_USE`, before brew runs, while the version is the default, any site is
  isolated to it or names it in its `.yard` (hint: the edit to make in that file), or
  `yard.toml` records a brew, pecl or path extension for it (hint
  `yard ext remove <name> --php <ver>`); the job checks again when it starts, since it
  may wait behind another. After brew removes it, the version's `[php."<ver>"]` table,
  by then only bundled toggles, goes too.
- `yard php default <ver>` sets the global default (`default_php` in `yard.toml`);
  the version must be installed.
- `yard php list` shows each installed version (and the default if it is not
  installed), whether it is the default, the sites on it and its pool's state.
- FPM is launched by `yardd`, never by brew:
  ```
  <brew prefix>/opt/php@<ver>/sbin/php-fpm --nodaemonize \
      -y php/<ver>/fpm.conf -c php/<ver>/php.ini
  env PHP_INI_SCAN_DIR=<yard>/php/<ver>/conf.d
  ```
  PHP scans only Yard's `conf.d`, so Yard owns every ini PHP loads (decided
  2026-10-20; an extension formula's own ini in brew's `conf.d` would otherwise load
  it behind `ext disable`). Brew's `conf.d` is mirrored into it as
  `10-brew-<file>.ini`, except files that load an extension `yard.toml` manages or a
  bundled one it disables. `99-overrides.ini` sorts last, so user overrides always win.
  A brew ini that loads a `.so` by a path that is gone is not mirrored: `brew uninstall`
  of an extension leaves its ini, and brew keeps a user's edited ini, writing its current
  one beside it as `<file>.default`. Yard never writes brew's files, so `yard doctor` warns
  `brew-ini:<ver>/<file>` (brew's own `php` warns about it on every run), with the hint
  `rm <file>`, or `mv <file>.default <file>` when brew's current ini loads the same
  extension (decided 2026-11-02).
- `php.ini`, `fpm.conf` and the `conf.d` inis are rendered for every installed
  version, pool or not, so the CLI shims (§4.5) read the same configuration.

### 4.2 FPM pools

- One pool per PHP version, socket at `run/php-<ver>.sock`.
- `pm = ondemand`, `pm.max_children = 10`, `pm.process_idle_timeout = 30s`.
- Workers get no environment but the pool's `env[]` (php-fpm's `clear_env`), and every pool
  sets `env[PGGSSENCMODE] = disable`. libpq's default `gssencmode=prefer` looks for Kerberos
  credentials on each connect, which runs CoreFoundation in a php-fpm child that forked without
  exec'ing, where macOS does not make it safe: PHP 8.5's workers segfaulted on every PDO
  PostgreSQL connect after one yardd restart on the Mac mini (2026-11-06), and the same
  connect with `gssencmode=disable` worked. Yard's PostgreSQL never uses GSS; a DSN that sets
  `gssencmode` still wins.
- A pool is started only when at least one site resolves to that version, and
  stopped when none do.

### 4.3 Extensions

`yard ext add <name> [--php <ver>]` (default: global default version) resolves in order:

1. `brew install shivammathur/extensions/<name>@<ver>` if the formula exists.
2. `pecl install <name>` using that version's `pecl`/`phpize`
   (`<brew prefix>/opt/php@<ver>/bin/pecl`).
3. `yard ext add <path/to/ext.so> --php <ver>` copies the binary into
   `php/<ver>/ext/` and loads it by absolute path.

`<name>-<version>` (`xdebug-2.9.8`) pins a pecl release, which legacy PHP often needs:
it skips brew, runs `pecl install <name>-<version>`, and records the module as `<name>`.
`brew info --formula` decides step 1; pecl runs with the keg's `bin` then brew's `bin`
first on PATH (never Herd's `php`; phpize needs brew's `autoconf`), with
`CPPFLAGS=-I<prefix>/include` and `LDFLAGS=-L<prefix>/lib` (PHP's headers include brew's
`pcre2.h`), and with `-c <root>/php/<ver>/pecl.conf`, a PEAR user config Yard writes
before each run that sets only `php_ini = <root>/php/<ver>/pecl-php.ini`, a file that
never exists. brew's `pear.conf` names brew's `php.ini`, and pecl adds `extension=` to
it after an install; `-d`/`-D php_ini=` cannot stop that, because they store the value
under pecl's channel while the install reads it for `pear.php.net`. pecl installs into its own
`ext_dir` (`<prefix>/lib/php/pecl/<api>`), not PHP's `extension_dir`, so Yard asks
`pecl config-get ext_dir` and records the absolute `so` on the entry. A `.so` path's module name is its file name without `.so` or a
leading `php_`; the CLI sends it absolute. A name already in `yard.toml` for that
version is refused with `INVALID_ARGUMENT` (hint `yard ext enable <name>`).

Yard then records the extension in `yard.toml`, reconciles (which writes
`conf.d/50-<name>.ini` and restarts the version's pool) and verifies with
`<keg>/bin/php -m` under `PHPRC=php/<ver>` and `PHP_INI_SCAN_DIR=php/<ver>/conf.d`.
If the module is not listed, the job fails with `EXT_VERIFY_FAILED` and the
`php -m` output in `data.output`. A copied `.so` is deleted and `yard.toml` put back
byte for byte. Something brew or pecl installed stays installed but is recorded
`enabled = false`, since with no entry brew's own ini would load it again; the hint is
`yard ext remove <name> --php <ver>`.

Extensions are recorded per version in `yard.toml` (version keys written as Yard does,
`"8.4"`; `"08.4"` is `CONFIG_INVALID`), which the generated inis name as the file to edit:

```toml
[php."8.4".extensions]
redis = { source = "brew" }                       # brew | pecl | path | bundled
custom = { source = "path", so = "/abs/custom.so" }
opcache = { source = "bundled", enabled = false }
```

A `brew` extension loads the path its formula's ini in brew's `conf.d` names
(shivammathur's formulas keep the `.so` in their own keg); `pecl` loads its recorded `so`
(an entry without one loads `<name>.so` from `extension_dir`). A brew ini whose absolute `.so` is gone (`brew uninstall` leaves the
ini behind) is neither mirrored nor listed. `enabled` defaults to true. `path` needs an absolute `so`. `bundled` is an extension
PHP itself loads (opcache), listed only to disable it. `xdebug` and `opcache` load as
`zend_extension`. A disabled or bundled entry writes no `50-` ini.

Other commands:

- `yard ext list [--php]`: the managed extensions, plus each module brew's `conf.d`
  loads that `yard.toml` does not manage: as `brew` when its ini loads it by absolute
  path from under brew's prefix (an extension formula installed by hand), else as
  `bundled`.
- `yard ext enable|disable <name> [--php]` toggles `enabled` without uninstalling, and
  reconciles (which restarts the pool). Names match in any case. Disabling an unmanaged
  module writes `{ source = "bundled", enabled = false }` (or `"brew"`, as listed);
  enabling a bundled one drops that entry.
- `yard ext remove <name> [--php]` is a job: `brew uninstall` for `brew`,
  `pecl uninstall` for `pecl`, deleting the copy for `path`; then the entry goes. A
  bundled module is refused (hint `yard ext disable <name>`).
- `yard xdebug on|off [--php]` is `ext enable|disable xdebug`.
- A name with no entry and no bundled module is `EXT_NOT_FOUND`, hint
  `yard ext add <name> --php <ver>`.

### 4.4 php.ini

Generated defaults (dev-friendly): `memory_limit=512M`,
`upload_max_filesize=1G`, `post_max_size=1G`, `max_execution_time=0` (CLI) /
`300` (FPM), `display_errors=On`, `error_reporting=E_ALL`,
`opcache.enable=1`, `opcache.validate_timestamps=1`, `opcache.revalidate_freq=0`,
`date.timezone` from the system.

`yard php config set <key> <value> [--php <ver>]` and
`yard php config get|unset` edit `99-overrides.ini` only, line by line so the user's
comments and other lines stay, and reload that version's pool gracefully (SIGUSR2).
Unsetting a key that is not overridden succeeds and changes nothing, so scripts can
run it twice. A `99-overrides.ini` that is a symlink (into dotfiles, say) is written
through, and the file keeps its mode.
Keys are directive names (`[A-Za-z0-9_.]`); a value is one line without `"`, and is
quoted when it holds spaces or `;`.

### 4.5 Version resolution and isolation

For a site, the version is the first match of:
1. The `.yard` at the project's root (project-file spec §5): its `php`, a major.minor
   version. A site with a `.yard` refuses `yard isolate`,
   `secure` and `docroot` (`SITE_HAS_PROJECT_FILE`): the file owns them. Without one,
   the site's managed `.yard` (`sites/<name>/.yard`, managed-file spec §4), while its
   `project` is the site's path; those commands then edit it.
2. `sites.toml` isolation (`yard isolate <ver> [--site <name>]`).
3. The global default.

For the CLI shims (current working directory), the version is the first match of:
1. The nearest `.yard` in the cwd or any parent (project-file spec §5): its `php`, else
   the global default.
2. The managed `.yard` of the site containing the cwd (managed-file spec §4).
3. The isolation of the site containing the cwd.
4. A `.yard-php` file in the cwd or any parent.
5. The global default.

`composer.json` `require.php` is advisory only: `yard doctor` warns when the
resolved version does not satisfy it (check `php-constraint:<site>`). Constraints are
judged at major.minor (`^`, `~`, comparisons, `*`, `x.y.*`, and/or); one Yard cannot
read gives no check. The hint isolates the site to the highest installed version that
fits, else installs the lowest supported one that does.

Isolating a site regenerates its nginx config to `fastcgi_pass` the target
version's socket, ensures that pool is running, and reloads nginx. The version is
`php = "<ver>"` in the site's `sites.toml` entry; it must be installed, and only a
linked site can have one (a proxy runs no PHP). `yard unisolate [--site]` removes it,
and a pool no site resolves to stops.

Shims in `bin/` (`php`, `composer`, `pecl`, `phpize`, `php-config`) are small
Rust binaries (one multicall binary, dispatch on `argv[0]`) that resolve the
version and `exec` the brew binary. `composer` runs the composer phar under the
resolved PHP. `node`, `npm` and `npx` are the same binary and run the Node a directory
pins (§8.1). `yard install-shims` prints the PATH line for the user's shell
profile; Yard does not edit dotfiles.

- Before it execs, `php` or `composer` in a linked site reads `run/sleeping`, and when it
  lists the site asks yardd's `wake.sock` to wake the site's services, waiting up to 30 s
  (M4e spec §5.2). With yardd down it prints one line and runs anyway, so the shims still
  run without a daemon; they just cannot wake anything. When yardd refuses (another program
  holds the port of an instance the site needs), it exits 1 without running PHP.
- The shim reads `yard.toml` and `sites.toml` itself, with no daemon. The containing
  site is the linked site whose `path` is the longest ancestor of the cwd; one that is
  not isolated falls through to `.yard-php`. A bad `.yard-php` is `INVALID_ARGUMENT`
  naming the file, and a `.yard` that does not parse is `PROJECT_FILE_INVALID`. Only a
  regular file of at most 64 KiB is read, so a FIFO cannot hang the shim; a `.yard` or
  `.yard-php` that is there but unusable (a FIFO, a dangling link, too large) is an error
  naming it, never a fall-through to other settings, while a directory of that name is
  skipped. Site paths are compared resolved, so a `sites.toml` path spelled through a
  symlink still holds the cwd, and of sites sharing the innermost path the one whose
  managed `.yard` is for it wins. When a `.yard`
  names a PHP that is not installed, the error is `FORMULA_MISSING`, `PHP <v> is not
  installed (<file>)`, hint `yard init`. A `yard.toml` or `sites.toml` that does not parse is only a warning
  on stderr: the shim uses PHP 8.4 as the default, or ignores site isolation, so one
  half-saved file does not stop every `php` on the machine.
- It runs `<keg>/bin/<name>` (`composer`: `<keg>/bin/php <brew>/bin/composer`, and
  `FORMULA_MISSING`, hint `brew install composer`, without it). When the version's
  `php.ini` is rendered it sets `PHPRC=php/<ver>` and `PHP_INI_SCAN_DIR=php/<ver>/conf.d`,
  so the CLI loads the same extensions and overrides as FPM. PATH is the keg's `bin`,
  then Yard's `bin/`, then the caller's PATH without any `Herd` directory: nothing a
  shim starts can reach Herd's `php` or `composer` (decided 2026-10-20).
- Errors print `yard: <message>` and `  hint: <hint>` and exit 1. `YARD_SHIM_DEBUG=1`
  prints the chosen version and why. `YARD_BREW_PREFIX` overrides `/opt/homebrew`
  (tests), here and in `yardd` and the offline `yard doctor`; `yard helper install`
  ignores it, since the root helper must not trust the caller's environment.
- Reconcile keeps each `bin/<shim>` a symlink to the `yard-shim` installed next to
  `yardd`. A link to another `yard-shim` is moved; any other file there is left alone
  and reported.
- `yard install-shims` (offline) prints `export PATH="<root>/bin:$PATH"` and warns
  when the first `php` or `node` on the current PATH is not Yard's, naming whose it is
  (Herd's, nvm's, mise's, fnm's or brew's) when it can tell.

## 5. Serving, DNS, HTTPS

### 5.1 Sites

- **Explicit only (decided 2026-10-20):** a project is a Yard site only when it
  has been linked. Yard never serves a directory just because of where it sits.
  M1 shipped Herd-style parking (`yard park`/`unpark`, every directory inside a
  parked path became `<dirname>.<tld>`); M2 removed it. A `parked` key left in an
  M1 `yard.toml` is ignored with a warning. Moving a project from Herd is
  `yard link` (and `yard isolate`) run in it by hand.
- **Linked:** `yard link [name]` serves the cwd as `<name>.<tld>`;
  `yard unlink [name]`. Nothing is written into the project, except by
  `yard init --local`, which writes its `.yard` (project-file spec §4) and never
  overwrites one without `--force`. Plain `yard init` writes the managed `.yard` in
  Yard's data root instead (managed-file spec, M4a.1).
- **Proxied:** `yard proxy <name> <url>` serves `<name>.<tld>` as a reverse proxy
  (Node, Vite, etc.); `yard unproxy <name>`.
- **Docroot detection** order: `public/`, `web/`, `public_html/`, project root.
  Override: `yard docroot <path> [--site]`.
- `yardd` watches `yard.toml`, `sites.toml`, `ports.toml` and `services.toml` with FSEvents; an edit
  reconciles (debounced 500 ms), so a hand-added `[sites.<name>]` is served.

### 5.2 nginx

- Brew `nginx`, run by `yardd` as the user in the foreground (`daemon off;`),
  reloaded with `SIGHUP`. It stops when `yardd` stops.
- Every config change runs `nginx -t` (as the user, from `yardd`) before
  reload; on failure the previous config set is restored and the error returned.
- `yardd` watches `nginx/overrides/`, and MinIO's `services/minio/overrides.conf`, which
  `minio.<tld>`'s block includes (M4b). An edited override is checked with
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
- The TLD is `yard`; a `tld` in `yard.toml` sets it (for example `test` once Herd
  is gone). It is independent of the port profile (§7). It must match
  `^[a-z][a-z0-9-]{0,62}$`, or the file is `CONFIG_INVALID` at its line.
- `resolver.install` and `resolver.remove` only touch files carrying Yard's
  marker comment, so Herd's `/etc/resolver/test` is never overwritten or
  deleted.

### 5.4 HTTPS

- On first need (the first secured site, or `yard trust`) `yardd` generates a CA
  (`rcgen`, ECDSA P-256, 10-year validity, `CN = Yard CA (<user>)`) in `ca/`;
  `yard-ca.key` is mode 0600. It makes a new one when the files are missing, do not
  parse or do not match, and once less than a leaf's 825 days remain; the old pair is
  kept as `yard-ca.pem.bak` and `yard-ca.key.bak`. A CA file it cannot read is an error,
  never a reason to replace the CA. No leaf outlives the CA that signed it.
- **Trust is the user's, in the login keychain** (decided 2026-10-20, the maintainer): macOS 11+
  makes the user confirm a trust-settings change in a dialog, which a root
  LaunchDaemon cannot show, so there are no `ca.trust`/`ca.untrust` helper verbs.
  `yard trust` runs `/usr/bin/security add-trusted-cert -r trustRoot -k
  ~/Library/Keychains/login.keychain-db <ca.pem>` as the user (macOS asks for the
  password once); `yard untrust` runs `security delete-certificate -Z <SHA-256> -t`,
  and fails with `CA_STILL_TRUSTED` (hint `sudo security remove-trusted-cert -d <ca.pem>`)
  when trust set outside the login keychain remains. With no CA, `untrust` makes none.
  `ca.info` reports `trusted` from `security verify-cert -c <ca.pem> -l -L -q -p basic`.
  Safari and Chrome use the login keychain; curl in scripts takes `--cacert`.
- `yard secure [site]` issues a leaf cert (825-day validity, SAN = site host and
  `*.<site host>`) and serves the site on the https port, with its http port
  answering `301` to the https URL (8080→8443 on `standard`, 80→443 on
  `privileged`). `yard unsecure [site]` reverses it. The site defaults to the one
  containing the current directory. `sites.toml` gets `secure` only where it differs
  from `secure_by_default`.
- A host no secured site claims is refused on the https port (`ssl_reject_handshake`),
  never served by another site's certificate and app.
- `secure_by_default = true` in `yard.toml` secures every current and future site.
- Certs within 30 days of expiry are renewed automatically by `yardd`.
- Reconcile issues missing certs and re-issues ones a new CA did not sign or whose
  names no longer match the host, before it renders nginx; a new cert reloads nginx.
  It removes the files of sites no longer secured only once `nginx -t` passes, since
  until then the previous config still names them. A cert that cannot be issued fails
  the reconcile, so the previous config keeps serving. The periodic scan reconciles
  when a secured site's cert is due, so expiry and a deleted file are caught without
  a config change.
- `yard doctor` warns while a secured site's CA is untrusted (hint `yard trust`), and
  prints `NODE_EXTRA_CA_CERTS=<path>` for Node that does not run through Yard's shims (the
  shims set it themselves when it is unset, M4c spec §6) and, when Firefox is installed, the
  `security.enterprise_roots.enabled` setting to turn on. It edits neither.

### 5.5 Herd coexistence and migration

- **No takeover** (decided 2026-10-20, the maintainer): there is no `yard takeover`. Yard
  never stops, quits or reconfigures Herd. Both run side by side: Herd keeps `.test`,
  80/443 and `/etc/resolver/test`; Yard serves TLD `yard` on the `standard` profile
  (8080/8443).
- Yard never moves to a TLD whose `/etc/resolver/<tld>` lacks Yard's marker:
  the change is refused with `RESOLVER_FOREIGN` (naming Herd for `test`) and the
  previous TLD keeps serving, even when Herd is quit. Yard never edits or deletes a
  resolver file it did not write, so moving to `.test` is the user's own steps:
  uninstall Herd, remove its `/etc/resolver/test`, then set `tld = "test"`. URLs
  without a port are `yard ports profile privileged`, once nothing else holds 80/443.
- A TLD change re-issues every secured site's certificate for its new host (§5.4).
- No `yard migrate herd` (dropped 2026-10-20): Yard does not import Herd's
  config. Each project moves by running `yard link` in it.

### 5.6 Tunnels (M5)

A site Yard serves can be shared at a public HTTPS URL through cloudflared, designed in
`docs/superpowers/specs/2026-10-21-yard-tunnels-design.md`. `yard tunnel` makes a quick
tunnel (a random `trycloudflare.com` URL, held only in yardd's memory) or, with `--host`, a
named one on a domain set up with `yard tunnels domain add` (kept in `sites.toml`, with its
Cloudflare facts in `state/tunnels.json`). Each tunnel is a `tunnel-<site>` unit running brew's
cloudflared with a generated `tunnels/<site>.yml`, in front of its own nginx listener on
127.0.0.1 that serves only that site, with `HTTPS on` and the visitor's address from
`CF-Connecting-IP`. `yard untunnel` deletes the Cloudflare tunnel Yard made, and names the
DNS record it cannot delete; `yard doctor` reminds the user until the record is gone.

## 6. Daemon

### 6.1 Supervisor

- Every managed process is a `Unit` with `start`, `stop`, `reload` (optional),
  `health`, and a log sink.
- Health: socket connect (FPM) or TCP connect (registered port), every 5 s, and every
  200 ms (`start_poll`) while a starting unit has not passed yet, so a woken service is up as
  soon as it listens (M4e). A running FPM pool's socket is only checked to exist, never
  connected to: its `ondemand` pool forks a worker per connection and reaps one only after
  30 s idle, so a probe every 5 s kept every worker it ever forked alive, about 270 MB across
  11 pools on the Mac mini (found 2026-11-06). A pool that dies is seen when its process exits. Known gap: in the privileged profile yardd holds nginx's listeners, so a
  connect succeeds into their backlog even with no nginx serving. An HTTP check closed
  it but flaked the suite (unexplained resets on the inherited sockets), so it waits
  for a check that does not depend on them.
- Stop: SIGTERM, then SIGKILL after 5 s, or after the unit's own `stop_timeout` (service
  units get 30 s). A running nginx gets SIGQUIT instead, its graceful shutdown; a unit that
  has not passed a health check yet always gets SIGTERM.
- A unit whose spec has a `pid_file` has it written on every spawn and removed when the
  process is gone; `orphans::stop` reads it to stop what a killed `yardd` left behind, which
  yardd does at startup for every service instance, before its first reconcile.
- Crash → restart with exponential backoff (1 s → 30 s). Five failures within
  60 s → `failed` state, retains the last 50 log lines, emits an event; no further
  restarts until `yard restart <unit>`.
- Logs: `logs/<unit>.log`, rotated at 10 MB, keep 5.
- **Reconcile loop:** on startup and on any state change, `yardd` reads the TOML
  files, renders all configs in memory, diffs against disk, writes changed
  files, and restarts or reloads only affected units. Reconcile is idempotent.
  A PHP pool the reconcile starts must pass its health check (its socket answers) before
  nginx reloads, waiting at most 5 s and warning after that, so the call that linked or
  isolated a site returns only once its first request can reach PHP, not a 502 (found
  2026-11-05 on a Mac where no other site kept the pool running).
  A failed reconcile keeps the last good state serving; its error is kept and
  shown by `status` (`config_error`) and `doctor` until a reconcile succeeds.
  A linked project whose docroot (e.g. `public/`) appears later is picked up by
  the periodic scan.
- A service instance with `sleep_after` set (M4e) falls asleep after that long without a
  client: its sites are listed in `run/sleeping` first and its clients counted again, then
  the reconcile leaves it out of the desired units, so `apply` stops it with its own
  signal, and a wake puts it back (from the last good units when a TOML does not parse). Sleep is runtime state in yardd, never written to a TOML,
  and every instance starts awake with yardd.
- A tunnel unit (M5) is health-checked by a TCP connect to cloudflared's metrics port, so
  losing the network never restarts it (a quick tunnel would lose its URL); its readiness
  (`connecting` or `ready`) is read from `/ready` by yardd's tunnel monitor every 2 s. Tunnel
  units get a 30 s start timeout. A quick tunnel ends with yardd.
- On `yardd` shutdown, all child units are stopped, nginx included.

### 6.2 API

- JSON-RPC 2.0 over `yardd.sock` (mode 0600, owned by the user).
- Request/response methods (non-exhaustive, same surface as the CLI):
  `status`, `doctor`, `sites.list|link|unlink|proxy|isolate|secure|unsecure|docroot|init`,
  `php.list|install|uninstall|default|config.get|config.set|config.unset`,
  `ext.list|add|remove|enable|disable`, `xdebug.set`,
  `ports.list|check|set|pin|unpin|range|profile`,
  `services.list|add|remove|start|stop|restart|logs|port|sleep.set`, `sites.info`,
  `sources.list|trust|untrust`,
  `ca.info`, `node.list|install|uninstall|default|use|unuse`,
  `tunnels.list|add|remove|domains|domain_add|domain_remove` (M5, tunnels spec §12), `units.restart`,
  `jobs.cancel`, `env.print`.
- Long operations (brew installs) return `{ job_id }` immediately;
  progress arrives as `job_progress` events (one per output line) and the result as
  one `job_finished` event (`error` set on failure, `JOB_FAILED` with the last 20
  lines in `data.tail` when a command exits non-zero), on the `jobs` topic. Jobs run
  one at a time, in start order; a job queued behind another first reports
  `waiting for <job_id>`. Brew jobs run with `HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1`,
  and pecl as §4.3 says. A client subscribes to `jobs` before it calls.
- `jobs.cancel { job_id }` (decided 2026-10-20: cancel, no timeout, since a brew build
  can rightly take half an hour). A queued job finishes `JOB_CANCELLED` without
  running. A running job's command, started in its own process group, gets SIGTERM
  and, after 10 s, SIGKILL; the job then cleans up as on a failure (`ext add` rolls
  back) and finishes `JOB_CANCELLED`. An unknown or finished id is `JOB_NOT_FOUND`.
  In the CLI, Ctrl-C during a job cancels it and waits; a second Ctrl-C stops waiting
  and exits 130.
- Subscriptions (`subscribe` with topic list: `units`, `sites`, `logs`, `ports`,
  `jobs`, `tunnels`, and from protocol 2.11 `state`) push events tagged `unit_state_changed`,
  `site_added`, `site_removed`, `job_progress`, `job_finished`, `log_line`, `port_conflict`,
  `tunnel_changed` (M5, when a tunnel's state, URL or last error changes) and `reconciled`
  (2.11, topic `state`: once after every reconcile, with `config_error`, the reconcile's error
  or `null`, so a client refetches what it shows). From protocol 2.10 `job_finished` carries an
  optional `result`, the job's answer on success (`tunnels.add` sends its `TunnelInfo`).
- Types are defined once in `yard-proto` (serde) and used unchanged by the CLI and, from M6,
  Yard.app. A daemon and an app from different builds still meet, so every enum a newer yardd
  may extend carries a fallback variant it never sends (M6 design §6).
- Protocol version is exchanged on connect; mismatched major versions are
  refused with a clear upgrade message.
- `wake.sock` (§3.1) is not part of this API: it is not JSON-RPC and has no types in
  `yard-proto`.

### 6.3 Errors

Every error is `{ code, message, hint, data? }`. Codes are stable strings, e.g.
`PORT_IN_USE`, `PHP_NOT_INSTALLED`, `PHP_IN_USE`, `JOB_FAILED`, `JOB_CANCELLED`, `JOB_NOT_FOUND`,
`BREW_MISSING`, `NGINX_CONFIG_INVALID`,
`HELPER_NOT_INSTALLED`, `EXT_VERIFY_FAILED`, `EXT_NOT_FOUND`, `CA_NOT_TRUSTED`, `CA_STILL_TRUSTED`,
`SITE_HAS_PROJECT_FILE`, `NEEDS_INSTALL`, `SITE_NAME_TAKEN`, `PROJECT_FILE_INVALID`,
`PROJECT_FILE_EXISTS`, `SERVICE_NOT_FOUND`, `SERVICE_EXISTS`, `SERVICE_IN_USE`, `SERVICE_VERSION_UNSUPPORTED`,
`FORMULA_UNTRUSTED`, `NEEDS_CONFIRMATION`, `NODE_MISSING`, `NODE_IN_USE` (M4c), `EMAILULATOR_NOT_FOUND`
(`data.searched`, hint emailulator's installer) and `SERVICE_EXTERNAL` (`data { label, pid }`, hint
`emailulator service status`, or `emailulator service uninstall` for a remove; M4d), and for tunnels (M5,
tunnels spec §12) `TUNNEL_NOT_LOGGED_IN`, `TUNNEL_DOMAIN_NOT_SET_UP`, `TUNNEL_KIND_REQUIRED`,
`TUNNEL_EXISTS`, `TUNNEL_NOT_FOUND`, `TUNNEL_CREDENTIALS_MISSING`, `TUNNEL_DNS_EXISTS`,
`TUNNEL_DNS_FAILED`, `TUNNEL_NOT_READY` and `SITE_TUNNELLED`. The CLI prints message and hint;
the app shows them inline with an action button when the hint maps to a command.

### 6.4 CLI

- Every command supports `--json` (machine-readable, stable shape from
  `yard-proto`); default output is human tables/text.
- If `yardd` is not running, commands that need it print
  `yardd is not running → yard start` and exit 3 (with `--json`, the error code is
  `DAEMON_NOT_RUNNING`). `doctor` and `ports check` run in-process instead.
- `yard --version` and `yard version --json` report CLI, daemon, helper and
  protocol versions plus build commit.
- A command that would install something (`yard init`, and `yard link` for a project
  whose `.yard` needs a PHP, extension or service) lists what it would install and asks on a
  terminal; `--yes` skips the question, and without a terminal it installs nothing and
  exits 1 with `NEEDS_INSTALL` (project-file spec §6). Trust is a separate question
  (`FORMULA_UNTRUSTED`, M4b spec §8): `--yes` never answers it.

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
"mysql-8.4"  = { port = 3307,  mode = "pinned" }   # a service: allocated once, at `yard service add`
redis        = { port = 6380,  mode = "pinned" }
emailulator-smtp = { port = 1025,  mode = "external" }   # emailulator's own LaunchAgent (M4d): written and
emailulator-http = { port = 8025,  mode = "external" }   # moved by reconcile to follow the agent, removed when none serves
"proxy:api"  = { port = 3000,  mode = "external" }
tunnel-blog        = { port = 33012, mode = "auto" }   # M5: cloudflared's metrics server
nginx-tunnel-blog  = { port = 33011, mode = "auto" }   # M5: nginx's tunnel listener for blog
```

Each tunnel (M5) has these two `auto` entries, allocated when it starts and removed when it
ends; nginx owns `nginx-tunnel-*`.

**Modes**

- `pinned`: exactly this port. If unavailable, the unit does not start. Service instances
  (M4b) are recorded `pinned`: `yard service add` allocates once, `--port` or else the
  first free port at or above the engine's usual one that no other entry holds and no
  other database app reserves (DBngin.app reserves 3306, 5432 and 6379, Postgres.app 5432,
  while installed; m4b spec §6), and nothing moves it afterwards.
- `preferred`: try `preferred` first. If unavailable: with `auto_fallback`,
  allocate the lowest free port in `range` and record it; otherwise fail with
  `PORT_IN_USE` and the hint to set a port or enable fallback.
- `auto`: allocated from `range`.
- `external`: not bound by Yard (proxy targets, and emailulator's agent). Tracked so Yard
  never allocates it and can warn when the target is not listening. emailulator's two entries
  follow its LaunchAgent: reconcile writes them on the ports `emailulator service status --json`
  reports while the agent is installed and loaded, moves them when it moves, and removes them
  when it goes (M4d).

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
2. **Pre-start:** a real bind probe on `127.0.0.1` and `::1` of every port the unit has
   (`UnitSpec.ports`; the first one held fails the unit); for 80 and 443,
   which a user may not bind on loopback, a connect (free when nothing answers),
   and in the `privileged` profile the helper's `ports.bind` is the check. Before
   the first nginx start, `yardd` stops an nginx an earlier, killed `yardd` left
   running (the master in `run/nginx.pid`, if it is brew's nginx and launchd is
   its parent). On failure, the
   holder is identified via `lsof`, which reads libproc as the user (pid, process name, parent; labelled as
   Herd, DBngin, Postgres.app, brew services, Docker, or unknown when recognisable) and returned in
   `PORT_IN_USE.data`.
3. **Runtime:** health probes detect a unit losing its port; a scan every 30 s
   detects foreign listeners on registered-but-idle ports and emits
   `port.conflict`.

**Commands:** `yard ports` (table: unit, port, mode, status, holder),
`yard ports check` (dry-run probe of all registered ports, also part of
`doctor` and run before `yard ports profile`), `yard ports set <unit> <port>`,
`yard ports pin|unpin <unit>`, `yard ports range <a-b>`,
`yard ports profile <name>`.

Changing a port restarts only that unit and reloads dependents (nginx for
proxies and `mail.<tld>`). `yard env` always prints the actual assigned ports.

## 8. Node and services

### 8.1 Node

Designed in `docs/superpowers/specs/2026-10-25-yard-m4c-node-design.md`, which this
section only summarizes.

- Node comes from brew's `fnm`, run by absolute path with `FNM_DIR=<root>/node`; Yard never
  uses fnm's own directory, nvm's or Herd's. brew installs fnm the first time a
  `node.install` job needs it.
- `yard node list [--available [<major>]]`, `install [<spec>]`, `uninstall <version>`,
  `default <spec>`, `use <spec> [--site <name>]` and `unuse [--site <name>]`. A spec is a
  major (`22`), a major.minor, an exact version or an LTS name (`lts/iron`, `lts/*`); the
  default is `22`.
- The `node`/`npm`/`npx` shims walk up from the cwd and the nearest pin wins; in one
  directory, a `.yard`'s `node` → the containing site's setting in its root (its managed
  `.yard`, else `sites.toml`) → `.nvmrc` → `.node-version`. With none, `default_node`. They run the newest installed version the spec
  matches and never download; a missing one is `NODE_MISSING` with the install hint.
- A missing Node never stops a site serving. `yard link` and `yard init` offer to install it,
  and `yard info` and `yard doctor` name it.

### 8.2 Services

Each service instance is a supervised `Unit` with data in `services/<instance>/data/` and its
port(s) in the registry, listed in `services.toml` (M4b design:
`2026-10-23-yard-m4b-services-design.md`). Instances are named `<engine>-<line>`.

| Instance | Formula | Notes |
|---|---|---|
| `mysql-<line>` (`8.0`, `8.4`, …) | `mysql@<line>`, else the plain `mysql` when it is on that line | `root` with empty password, local only; one instance per line |
| `postgres-<line>` (`15`–`18`) | `postgresql@<line>` | the macOS user, no password |
| `redis` | `redis` | no auth |
| `minio` | `minio/stable/minio` (a homebrew/core `minio` already installed is used as it is) | `minioadmin`; console proxied at `minio.<tld>` |
| emailulator | not brew; runs as its own LaunchAgent (see below) | SMTP and HTTP on the agent's ports, recorded `external`; web UI proxied at `mail.<tld>` (M4d) |

The lines are brew's: `add` runs `brew search --formula "/^<base>(@|$)/"` and `brew info
--json=v2` for each name found, once per add. A formula brew reports as disabled is not a
line; a deprecated one is, with a job line naming brew's disable date.

As built (daemon methods, and the CLI commands that call them):

- `services.list` answers a `ServiceInfo` per instance in `services.toml`.
- `services.add { engine, version?, port?, path? }` is a job (`path` is for emailulator, below). Before it starts: an unknown engine,
  a `version` for Redis or MinIO or one that is not a line, and `port` 0 are
  `INVALID_ARGUMENT`; an instance that exists is `SERVICE_EXISTS`; a line brew does not have
  is `SERVICE_VERSION_UNSUPPORTED` naming the lines it has; a formula brew lists but cannot
  describe is `FORMULA_MISSING`; and a formula from a non-core tap that is yet to be
  installed goes through the trust gate (`FORMULA_UNTRUSTED`, M4b §8). The job re-checks
  that the instance does not exist (jobs run one at a time, so a second add queued behind
  the first meets its entry), runs `brew install <formula>` when the keg is missing,
  allocates the ports in `ports.toml` (pinned: `--port` exactly, else the engine's usual
  port or the next free one, skipping DBngin's and Postgres.app's), runs the engine's init
  into an absent or empty `data/` (`mysqld --no-defaults --initialize-insecure`, with
  `mysql_native_password` as the default on a MySQL 8 line (M4b §16), `initdb`;
  otherwise the job says `reusing the data in <dir>`), writes the entry, reconciles, and waits
  up to 130 s for the unit to run (longer than a database's 120 s start timeout; `failed` ends it at once with the last lines). A failure or cancel
  removes the entry and port entries it wrote and what it created in `services/<i>/`, never
  data that was there before or a file the user put there.
- `services.remove { name, keep_data? }` refuses an unknown instance (`SERVICE_NOT_FOUND`) and
  one a linked site's `.yard` asks for (`SERVICE_IN_USE`, hint `edit <that .yard>`). Otherwise
  it removes the entry and port entries, reconciles, waits until the unit is gone, and only
  then deletes `services/<i>/`, keeping `data/` with `keep_data`. It never runs `brew
  uninstall`. Confirmation is the CLI's.
- `services.stop` and `services.start { name }` set `stopped = true` on the entry, or remove the
  key (never `stopped = false`, so the user's file is clean again), through `TomlFile` (an
  entry a remove deleted since the handler looked is `SERVICE_NOT_FOUND`, never a stub entry),
  then reconcile; both answer a `ServiceInfo`, and a start answers once the unit is spawned
  (`starting` until its health check passes). `services.restart { name }` is `units.restart` for
  the instance and refuses a stopped one with `INVALID_ARGUMENT`, hint `yard service start <i>`;
  it never clears `stopped`. `services.logs { name, lines? }` answers the last `lines` (default
  100, at most 10000) of `logs/<i>.log`, read from the end of the file (at most 8 MiB), and an empty list while there is no file.
  `services.port { name, port }` pins the instance's first port (MinIO's API port) with
  `allocate_exact` and reconciles, which restarts a running instance on it; a stopped one moves
  too and stays stopped. A port that is registered to another unit (MinIO's console included) or
  busy is `PORT_IN_USE` naming the holder, hint `yard service port <i> <port>`; its own port is
  not a move, and port 0 is `INVALID_ARGUMENT`. Each of the five answers an unknown name with
  `SERVICE_NOT_FOUND`.

**emailulator** replaced Mailpit on 2026-10-21 (maintainer): it is our own mail catcher, with a
web UI, a REST API and an MCP server, so agents can read the mail a site sent.

- `yard service add emailulator` needs emailulator installed: the CLI sends the one on the
  user's PATH as `services.add { path }`, and yardd falls back to the recorded
  `[emailulator] path` in `services.toml`, then the installer's `~/.local/bin/emailulator`.
  Without one it fails with `EMAILULATOR_NOT_FOUND`, hint emailulator's installer. Yard never
  downloads it (§2). A `version` is `INVALID_ARGUMENT`, hint `emailulator self-update`.
- **emailulator runs itself; Yard never runs it (M4d spec §4, option B, decided
  2026-10-26).** It runs as its own LaunchAgent (`emailulator service install`, label
  `com.webteractive.emailulator`), built around being the one server on the Mac, and keeps
  catching mail while yardd is stopped, for Herd's sites too. Yard records the agent's two
  ports as `external` entries and points `mail.<tld>` and `yard env` at them, and
  `yard doctor` says whether it is serving mail. `services.add` for emailulator is a job: it
  records `[emailulator] path`, then, unless an agent already catches mail, runs
  `emailulator service install` (`-- --smtp-addr 127.0.0.1:<port>` with `--port`), probes and
  reconciles. `yard service start|stop|restart|remove|port|sleep-after emailulator` are refused
  with `SERVICE_EXTERNAL`, hinting emailulator's own command; `yard service logs emailulator`
  reads the log the agent names. `yard services` lists it, while its agent is installed, with
  MANAGED BY `LaunchAgent <label>` and its web UI. yardd asks the binary (`--version` and
  `service status --json`, 5 s each) at every reconcile and every 30 s port scan, never under
  the reconcile lock, and keeps the last answer when one cannot be read (an emailulator it
  could not read is never a `.yard` need). The scan reconciles when the agent appears, goes,
  crashes, recovers or moves, so `yard doctor`, `yard services` and `yard env` follow it.
- A `.yard` may say `emailulator = true` in `[services]` (no version: it updates itself). Any
  agent Yard can reach meets it; without one it is a need, `yard service add emailulator`
  when the binary is there and a manual one naming the installer when it is not.
- emailulator keeps its own message store, so `emailulator app`, `emailulator mcp` and
  `emailulator inbox|key` see the same mail.
- `yard env` prints `MAIL_MAILER=smtp`, `MAIL_HOST=127.0.0.1`, `MAIL_PORT=<smtp>` and
  `MAIL_USERNAME`, `MAIL_PASSWORD`, `MAIL_ENCRYPTION` and `MAIL_SCHEME` as `null`, as an
  `emailulator` block, with a warning on stderr while the agent is loaded but not answering.
- `mail.<tld>` proxies the agent's web UI with its `Host` rewritten to the agent's address
  (emailulator answers 403 to any other host), unless a linked site is named `mail`; then
  `yard doctor` warns `mail-host`, hint `yard unlink mail`.

Commands: `yard services` (list), `yard service add <engine> [--version] [--port] [--yes]`,
`yard service remove <instance> [--keep-data] [--yes]`,
`yard service start|stop|restart <instance>`, `yard service logs <instance> [-f] [-n]`,
`yard service port <instance> <port>`, `yard env [--site] [--service]`, `yard info [site]`,
`yard sources` and `yard source trust|untrust <formula|tap> [--tap]`. The M4b design spec
(§7 and §8) has each command's output and confirmation rules.

Services bind `127.0.0.1` only. `yard env [--site]` prints `.env` lines
(`DB_*`, `REDIS_*`, `MAIL_*`) using actual ports; it never writes files.

## 9. Yard.app

- Rust with GPUI (Zed's UI framework), macOS 14+ (maintainer, 2026-11-06). The M6 design
  (`2026-11-05-yard-m6-app-design.md`, a draft) settles the toolkit's details: where GPUI comes
  from, the app's toolchain, and whether the menu bar is a native `NSMenu` or a GPUI window.
- **Menu bar**: health indicator, quick toggles for services
  and Xdebug, recent sites (open in browser), badges for failed units and port
  conflicts.
- **Main window** (a sidebar and a detail pane):
  - Sites: search, per-site PHP picker, secure toggle, open in browser /
    editor / Terminal, "Edit overrides" (opens the override file).
  - PHP: installed versions, install with progress, extensions per version,
    `99-overrides.ini` editor.
  - Services: start/stop, version, port, live logs, "Copy .env".
  - Ports: registry table with resolve actions on conflicts.
  - Logs: unified filterable stream.
  - Doctor: checks with fix buttons.
- **Client:** the app speaks JSON-RPC over the socket with `yard-proto`'s own types, through a
  client shared with the CLI; its state updates from event subscriptions — no polling.
- **Onboarding:** check Homebrew → locate `yard` and start `yardd` → install the helper
  (M6: `sudo yard helper install` handed to Terminal; M7: `SMAppService`) → offer the
  `privileged` profile (80/443) when nothing else holds those ports.
- From M7 the app bundle carries `yard`, `yardd`, `yard-helper` and the shim binary in
  `Contents/Helpers/`; M6 uses the installed `yard` (M6 design §18 Q6).

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
  and the 80/443 profile refused with Herd named; without Herd, the Herd steps are skipped and
  its resolver must stay absent). `scripts/e2e-m2.sh` adds M2 over HTTP:
  PHP 7.1 beside 8.4, a site isolated to 7.1, redis and xdebug added and checked with
  `php -m` through the shim, every `php` and `php-fpm` brew's (never Herd's), and
  `yard xdebug off`. `scripts/e2e-m3.sh` adds HTTPS: a secured site answers on
  `https://<site>.yard:8443` under `curl --cacert <ca.pem>`, its http URL answers
  `301` to the https one, its key is mode 0600, and `yard unsecure` serves http again
  and removes the certificate. It never runs `yard trust`, which macOS asks a password for.
  `scripts/e2e-m4a.sh` adds the `.yard`: a scratch project's file picks its PHP, an edit
  to it is served within seconds, a PHP that is not installed gives the 503 setup page
  naming `yard init`, and `yard isolate` is refused. It installs nothing.
  `scripts/e2e-m4b.sh` adds services: `mysql-8.4`, `postgres-17`, `redis` and `minio` are
  added (brew-installing what is missing, so it is asked for first), each answers with its
  own client by absolute keg path, `yard env` prints the ports `yard services` reports,
  `minio.<tld>` answers 200, brew's installed minio is the formula `services.toml` records,
  a scratch project's `[services]` entry shows met in `yard info`, and every instance it
  added is removed. It stops before changing anything if one of those instances already
  exists, backs up and restores `services.toml`, `ports.toml` and `sites.toml`, and fails if
  the listeners on 3306, 5432 and 6379 (DBngin, Postgres.app) or Herd's resolver changed.
  `scripts/e2e-m4d.sh` checks emailulator against the agent already on the machine and never
  installs, starts or stops it: `yard doctor` names the agent, `yard ports` has
  `emailulator-smtp` `external` on its SMTP port, `mail.<tld>` answers 200 while the agent's
  own port answers 403 to that `Host` (the rewrite is what makes it work), and a scratch
  project whose `.yard` says `emailulator = true` serves and prints the agent's `MAIL_PORT`.
- **Yard.app:** its client against a fake socket server and a real yardd; state tests driven by
  event streams; GPUI's own test harness for render smoke tests (M6 design).

## 11. Distribution

- Developer ID signed, notarized `.dmg` of Yard.app (arm64). Helper registered
  with `SMAppService`.
- Team updates via appdater; public updates via Sparkle if/when public.
- Homebrew tap formula for CLI-only installs (`yard`, `yardd`, helper, shims).
- Details follow the existing macOS release-signing and appdater conventions at
  the time of M7.

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
   (sites are explicit links only, §5.1). Done 2026-10-20, verified by
   `scripts/e2e-m2.sh` on the maintainer's machine.
3. **M3 HTTPS:** CA, secure/unsecure, secure_by_default, renewal (and re-issue on a
   TLD change). No `takeover` (dropped 2026-10-20, §5.5). Done 2026-10-21, verified by
   `scripts/e2e-m3.sh` on the maintainer's machine, where the maintainer ran `yard trust`.
4. **M4 Node and services**, in five plans (split 2026-10-22; M4d split off 2026-10-23, M4e added 2026-10-26):
   - **M4a the `.yard` project file and `yard init`**, designed in
     `docs/superpowers/specs/2026-10-22-yard-project-file-design.md`: `php`, `docroot`,
     `secure` and `extensions`, the setup page, and the shims following the file.
     **M4a.1** (2026-10-25) adds the managed `.yard` in Yard's data root, designed in
     `docs/superpowers/specs/2026-10-24-yard-managed-project-file-design.md` and planned in
     `docs/superpowers/plans/2026-10-25-yard-m4a1-managed-project-file.md`; verified by
     `scripts/e2e-m4a1.sh` on the maintainer's machine on 2026-10-25.
   - **M4b services:** MySQL, Postgres, Redis, MinIO, `yard env`, `yard info`, the
     file's `[services]`, and installs from third-party taps gated on brew's own trust
     (`yard sources`, `yard source trust|untrust`). Designed in
     `docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md`, which refines §7
     and §8.2 for services, and planned in
     `docs/superpowers/plans/2026-10-23-yard-m4b-services.md`. Built and verified with
     `scripts/e2e-m4b.sh` on the maintainer's machine on 2026-10-24, and merged into main.
   - **M4c Node:** the fnm wrapper and the file's `node`. Designed in
     `docs/superpowers/specs/2026-10-25-yard-m4c-node-design.md`, which refines §8.1, and
     planned in `docs/superpowers/plans/2026-10-26-yard-m4c-node.md`; built on main on
     2026-10-26, and verified with `scripts/e2e-m4c.sh` on the maintainer's machine on 2026-10-26.
   - **M4d emailulator** (moved out of M4b by the maintainer on 2026-10-23): emailulator as a
     service, `mail.<tld>`, its `MAIL_*` lines and `emailulator = true` in the file.
     Designed in `docs/superpowers/specs/2026-10-23-yard-emailulator-design.md`, planned in
     `docs/superpowers/plans/2026-10-23-yard-m4d-emailulator.md`; emailulator keeps its own
     LaunchAgent and Yard only points at it (option B, decided 2026-10-26). Spec and plan
     approved 2026-10-26; built on main and verified with `scripts/e2e-m4d.sh` on
     the maintainer's machine on 2026-11-01.
   - **M4e on-demand services** (approved 2026-10-26, the fifth plan): service
     instances sleep when idle (opt-in when built; on by default, 15 min, since
     2026-11-03) and wake on a PHP request, a shim or
     `yard service start`. Designed in
     `docs/superpowers/specs/2026-10-26-yard-m4e-on-demand-services-design.md`, whose §13
     records the maintainer's answers to its open questions, and planned in
     `docs/superpowers/plans/2026-10-26-yard-m4e-on-demand-services.md` (approved 2026-10-26);
     built on main on 2026-10-26, before M4d, and verified with `scripts/e2e-m4e.sh` on
     the maintainer's machine on 2026-10-26.
5. **M5 Tunnels:** share a site at a public URL, or with whoever has the link, through
   cloudflared, for webhooks and similar, behind a provider seam so ngrok and Tailscale Funnel
   can follow. Designed in `docs/superpowers/specs/2026-10-21-yard-tunnels-design.md` and
   planned in `docs/superpowers/plans/2026-11-02-yard-m5-tunnels.md` (approved 2026-11-03);
   built on main on 2026-11-03 and, for a quick tunnel, verified with `scripts/e2e-m1.sh` and
   `e2e-m5.sh` passing against yardd built from 0df393e (maintainer approved the run; its first pass
   found that this Mac's resolver cached the NXDOMAIN of a URL asked for before Cloudflare
   published it, so the script now resolves through 1.1.1.1). A named tunnel is the maintainer's manual
   check on his own zone.
6. **M6 Yard.app:** client, menu bar, main window, onboarding.
7. **M7 Distribution:** signing, notarization, appdater, brew tap.

The maintainer reordered M4–M7 on 2026-10-21: Node and services first, then tunnels, then the
app, then distribution (they were app, Node and services, distribution).
