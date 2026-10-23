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
- MySQL, Redis and emailulator run under Yard with no paid tier.
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
                                                │  services (mysql, redis, emailulator, …)
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
- No verb reads from the data root. The install checks that the data root is
  absolute and that every directory and symlink on its path, as written, is owned by
  the user or root, with no directory writable by group or others (sticky parents
  such as `/tmp` aside).
- The helper is installed by `sudo yard helper install` (binary to
  `/Library/PrivilegedHelperTools/`, plist to `/Library/LaunchDaemons/`), which is
  the only path while there is no app (M1–M5). Yard.app registers the same
  helper through `SMAppService` from M6. Upgrading from M1, whose helper ran
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
    yard-shim/               # php/composer/pecl/phpize/php-config shims (one multicall binary)
  app/                       # Swift package/Xcode project for Yard.app
  docs/
```

`yard-core` does pure work (parse, render, decide) and is where most unit tests
live. Side effects (spawning, binding, brew) live in `yard-daemon`.

### 3.3 Data layout

Root: `~/Library/Application Support/Yard/`

```
yard.toml                     # global: default php, tld override, secure_by_default, [php."<ver>".extensions]
sites.toml                    # links, per-site php, secure flag, docroot overrides, proxies
ports.toml                    # port registry and active profile (§7)
state/hashes.json             # hash of every generated file as Yard last wrote it
state/php-modules.json        # each PHP keg's compiled-in modules (`php -n -m`), keyed by its canonical path
dnsmasq/dnsmasq.conf          # generated
php/<ver>/php.ini             # generated
php/<ver>/fpm.conf            # generated
php/<ver>/conf.d/             # generated: brew's conf.d mirrored (10-brew-<file>.ini) and extension inis (50-<ext>.ini)
php/<ver>/conf.d/99-overrides.ini   # USER-OWNED, never written by Yard except via `php config set`
php/<ver>/pecl.conf           # generated before each pecl run: a PEAR user config (§4.3)
nginx/nginx.conf              # generated
nginx/sites/<site>.conf       # generated
nginx/overrides/<site>.conf   # USER-OWNED, included inside the site's server block
nginx/default/index.html      # generated site list served for unknown hosts
nginx/setup/<site>.html       # generated setup page for a site whose .yard is not met (503)
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
- `php.ini`, `fpm.conf` and the `conf.d` inis are rendered for every installed
  version, pool or not, so the CLI shims (§4.5) read the same configuration.

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
   `secure` and `docroot` (`SITE_HAS_PROJECT_FILE`): the file owns them.
2. `sites.toml` isolation (`yard isolate <ver> [--site <name>]`).
3. The global default.

For the CLI shims (current working directory), the version is the first match of:
1. The nearest `.yard` in the cwd or any parent (project-file spec §5): its `php`, else
   the global default.
2. The isolation of the site containing the cwd.
3. A `.yard-php` file in the cwd or any parent.
4. The global default.

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
resolved PHP. `yard install-shims` prints the PATH line for the user's shell
profile; Yard does not edit dotfiles.

- The shim reads `yard.toml` and `sites.toml` itself, with no daemon. The containing
  site is the linked site whose `path` is the longest ancestor of the cwd; one that is
  not isolated falls through to `.yard-php`. A bad `.yard-php` is `INVALID_ARGUMENT`
  naming the file, and a `.yard` that does not parse is `PROJECT_FILE_INVALID`. Only a
  regular file of at most 64 KiB is read, so a FIFO cannot hang the shim. When a `.yard`
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
  when the first `php` on the current PATH is not Yard's, naming Herd's.

## 5. Serving, DNS, HTTPS

### 5.1 Sites

- **Explicit only (decided 2026-10-20):** a project is a Yard site only when it
  has been linked. Yard never serves a directory just because of where it sits.
  M1 shipped Herd-style parking (`yard park`/`unpark`, every directory inside a
  parked path became `<dirname>.<tld>`); M2 removed it. A `parked` key left in an
  M1 `yard.toml` is ignored with a warning. Moving a project from Herd is
  `yard link` (and `yard isolate`) run in it by hand.
- **Linked:** `yard link [name]` serves the cwd as `<name>.<tld>`;
  `yard unlink [name]`. Nothing is written into the project, except by `yard init`,
  which writes its `.yard` (project-file spec §4) and never overwrites one without
  `--force`.
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
  prints `NODE_EXTRA_CA_CERTS=<path>` for Node and, when Firefox is installed, the
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
  `status`, `doctor`, `sites.list|link|unlink|proxy|isolate|secure|unsecure|docroot|init`,
  `php.list|install|uninstall|default|config.get|config.set|config.unset`,
  `ext.list|add|remove|enable|disable`, `xdebug.set`,
  `ports.list|check|set|pin|unpin|range|profile`,
  `services.list|add|remove|start|stop|restart|logs|port`, `sites.info`,
  `sources.list|trust|untrust`,
  `ca.info`, `node.list|install|use`, `units.restart`, `jobs.cancel`, `env.print`.
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
  `jobs`) push events tagged `unit_state_changed`, `site_added`, `site_removed`,
  `job_progress`, `job_finished`, `log_line`, `port_conflict`.
- Types are defined once in `yard-proto` (serde). `cargo xtask schema` emits
  JSON Schema; the Swift `Codable` models are generated from it and checked in
  CI for drift.
- Protocol version is exchanged on connect; mismatched major versions are
  refused with a clear upgrade message.

### 6.3 Errors

Every error is `{ code, message, hint, data? }`. Codes are stable strings, e.g.
`PORT_IN_USE`, `PHP_NOT_INSTALLED`, `PHP_IN_USE`, `JOB_FAILED`, `JOB_CANCELLED`, `JOB_NOT_FOUND`,
`BREW_MISSING`, `NGINX_CONFIG_INVALID`,
`HELPER_NOT_INSTALLED`, `EXT_VERIFY_FAILED`, `EXT_NOT_FOUND`, `CA_NOT_TRUSTED`, `CA_STILL_TRUSTED`,
`SITE_HAS_PROJECT_FILE`, `NEEDS_INSTALL`, `SITE_NAME_TAKEN`, `PROJECT_FILE_INVALID`,
`SERVICE_NOT_FOUND`, `SERVICE_EXISTS`, `SERVICE_IN_USE`, `SERVICE_VERSION_UNSUPPORTED`,
`FORMULA_UNTRUSTED`, `NEEDS_CONFIRMATION`. The CLI prints message and hint;
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
  whose `.yard` needs a PHP or extension) lists what it would install and asks on a
  terminal; `--yes` skips the question, and without a terminal it installs nothing and
  exits 1 with `NEEDS_INSTALL` (project-file spec §6).

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
emailulator-smtp = { port = 1025,  mode = "preferred", preferred = 1025 }
emailulator-http = { port = 33001, mode = "auto" }
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
`doctor` and run before `yard ports profile`), `yard ports set <unit> <port>`,
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
| emailulator | not brew (see below) | SMTP and HTTP on registry ports; web UI proxied at `mail.<tld>` |
| MinIO | `minio` | optional; console proxied at `minio.<tld>` |

**emailulator** replaced Mailpit on 2026-10-21 (maintainer): it is our own mail catcher, with a
web UI, a REST API and an MCP server, so agents can read the mail a site sent.

- `yard service add emailulator` needs `emailulator` on the user's PATH; without it, it fails
  with a hint naming emailulator's installer. Yard never downloads it (§2).
- It runs as `emailulator run --smtp-addr 127.0.0.1:<smtp> --http-addr 127.0.0.1:<http>`, the
  ports from the registry entries `emailulator-smtp` and `emailulator-http`. It keeps
  emailulator's own message store, not `services/emailulator/data/`, so `emailulator app`,
  `emailulator mcp` and `emailulator inbox|key` see the same mail.
- emailulator can run itself as a LaunchAgent (`emailulator service install`, label
  `com.webteractive.emailulator`), and while it does, `emailulator run` refuses to start a
  second server. Yard then runs no unit: it records the agent's two ports as `external`
  entries and points `mail.<tld>` and `yard env` at them. `yard doctor` says which of the two
  is serving mail.
- `yard env` prints `MAIL_MAILER=smtp`, `MAIL_HOST=127.0.0.1` and `MAIL_PORT=<smtp>`.

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
  the `privileged` profile (80/443) when nothing else holds those ports.
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
  and the 80/443 profile refused with Herd named). `scripts/e2e-m2.sh` adds M2 over HTTP:
  PHP 7.1 beside 8.4, a site isolated to 7.1, redis and xdebug added and checked with
  `php -m` through the shim, every `php` and `php-fpm` brew's (never Herd's), and
  `yard xdebug off`. `scripts/e2e-m3.sh` adds HTTPS: a secured site answers on
  `https://<site>.yard:8443` under `curl --cacert <ca.pem>`, its http URL answers
  `301` to the https one, its key is mode 0600, and `yard unsecure` serves http again
  and removes the certificate. It never runs `yard trust`, which macOS asks a password for.
  `scripts/e2e-m4a.sh` adds the `.yard`: a scratch project's file picks its PHP, an edit
  to it is served within seconds, a PHP that is not installed gives the 503 setup page
  naming `yard init`, and `yard isolate` is refused. It installs nothing.
- **Swift:** `YardClient` against a mock socket server; store tests driven by
  event streams; schema drift check in CI.

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
4. **M4 Node and services**, in four plans (split 2026-10-22; M4d split off 2026-10-23):
   - **M4a the `.yard` project file and `yard init`**, designed in
     `docs/superpowers/specs/2026-10-22-yard-project-file-design.md`: `php`, `docroot`,
     `secure` and `extensions`, the setup page, and the shims following the file.
   - **M4b services:** MySQL, Postgres, Redis, MinIO, `yard env`, `yard info`, the
     file's `[services]`, and installs from third-party taps gated on brew's own trust
     (`yard sources`, `yard source trust|untrust`). Designed in
     `docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md`, which refines §7
     and §8.2 for services.
   - **M4c Node:** the fnm wrapper and the file's `node`.
   - **M4d emailulator** (moved out of M4b by the maintainer on 2026-10-23): emailulator as a
     service, `mail.<tld>`, its `MAIL_*` lines and `emailulator = true` in the file.
     Designed in `docs/superpowers/specs/2026-10-23-yard-emailulator-design.md`, planned in
     `docs/superpowers/plans/2026-10-23-yard-m4d-emailulator.md`; whether Yard or
     emailulator's own LaunchAgent runs it is open for the maintainer.
5. **M5 Tunnels:** share a site at a public URL, or with whoever has the link, through
   cloudflared, for webhooks and similar, behind a provider seam so ngrok and Tailscale
   Funnel can follow. Designed in `docs/superpowers/specs/2026-10-21-yard-tunnels-design.md`.
6. **M6 Yard.app:** client, menu bar, main window, onboarding.
7. **M7 Distribution:** signing, notarization, appdater, brew tap.

The maintainer reordered M4–M7 on 2026-10-21: Node and services first, then tunnels, then the
app, then distribution (they were app, Node and services, distribution).
