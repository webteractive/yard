# Yard

Yard serves local PHP sites on macOS, as a replacement for Laravel Herd. A Rust daemon
(`yardd`, a user LaunchAgent) runs nginx, dnsmasq and a php-fpm pool per PHP version,
all from Homebrew and all as you. A small root helper (`yard-helper`, a LaunchDaemon)
does the few things that need root through a fixed set of verbs: it writes
`/etc/resolver/<tld>`, and in the privileged profile it opens ports 80 and 443 on
loopback and hands the sockets to yardd. Root never runs nginx. The `yard` CLI talks to
the daemon over a unix socket.

The design is in [`docs/superpowers/specs/2026-10-19-yard-design.md`](docs/superpowers/specs/2026-10-19-yard-design.md).

## Status

Milestone 1 (core serving, CLI only) is done and verified end to end beside a running
Herd: linked PHP sites, proxies, the port registry, `yard doctor` and the commands
below. M1.1 moved nginx from root to the user. M2 is done too: sites are explicit
(`yard link`, no parking), PHP 7.0–8.5 install from brew and run side by side, a site
can be isolated to its own version, extensions come from brew, pecl or a `.so`, and
the `php`/`composer`/`pecl` shims follow the site's version and never reach Herd's
PHP. M3 adds HTTPS: `yard secure` serves a site with a certificate from a local CA,
and `yard trust` puts that CA in your login keychain. M4a adds the `.yard` project file:
a project commits the PHP, docroot, HTTPS and extensions it needs, `yard init` writes it,
and a site whose file asks for something not installed serves a setup page naming the
fix. M4a.1 keeps that file out of the project when you want: `yard init` writes it to
Yard's data root (`sites/<name>/.yard`), `yard init --local` into the project, and a
committed `.yard` always wins. M4b adds services: MySQL, PostgreSQL, Redis and MinIO instances Yard runs from
brew's kegs (`yard service add`), a `[services]` table in `.yard`, `yard env` for the
`.env` lines, `yard info` for what one site runs on, and `yard source` for the brew
taps you trust. M4c adds Node: `node`, `npm` and `npx` shims that run the version a
project pins (`.yard`'s `node`, `yard node use`, `.nvmrc` or `.node-version`), installed with
brew's fnm into Yard's own `~/.yard/node` and never nvm's or Herd's. M4e lets service
instances sleep when idle and wake on use, if you turn it on. M4d points Yard at
[emailulator](https://github.com/webteractive/emailulator), the mail catcher, which keeps
running as its own LaunchAgent: `mail.<tld>` shows its web UI, `yard env` prints its `MAIL_*`
lines, and a `.yard` can say `emailulator = true`. Tunnels (M5) and
Yard.app (M6) come later.

Yard uses the **standard profile** by default: nginx on 8080/8443, and sites at
`http://<site>.yard:8080`. Herd keeps its `.test` domain,
ports 80/443 and `/etc/resolver/test` untouched. `yard ports profile privileged` moves
nginx to 80/443 through the helper, once nothing else holds them.

## Prerequisites

- An Apple Silicon Mac with Homebrew at `/opt/homebrew`.
- Rust 1.92 (`rust-toolchain.toml` pins it).
- nginx, dnsmasq and PHP from Homebrew; Yard runs them itself.

```sh
brew install nginx dnsmasq shivammathur/php/php@8.4
```

Yard only uses the installed binaries; do not start them with `brew services`.

A `dnsmasq` keg with no files in it needs `brew reinstall dnsmasq` instead. `yard doctor`
says which of these is missing.

## Install

```sh
cargo build --release
ln -sf "$PWD/target/release/yard" ~/.local/bin/yard   # any directory on PATH
sudo ./target/release/yard helper install   # once, and after each upgrade
yard start
yard doctor                                 # every check should say ok
```

The symlink keeps `yard` current across rebuilds; it still finds `yardd` and
`yard-helper` beside the binary it points at.

Run the helper install in a real terminal: sudo needs one to ask for the password.
Upgrading from M1, re-run it: it stops the root nginx M1's helper started, waits for it to
exit, and removes the two files that nginx left owned by root (`logs/nginx-error.log`,
`run/nginx.pid`). It names each file it removed and the next step: `yard start`, or
`yard restart nginx` when yardd is already running.
`scripts/e2e-m1.sh` then checks the whole path on your machine: it links a PHP app and
proxies another, curls both on `.yard:8080`, and confirms Herd's resolver is unchanged.
`scripts/e2e-m2.sh` checks multi-PHP and extensions, `scripts/e2e-m3.sh` HTTPS, `scripts/e2e-m4a.sh`
the `.yard` file, `scripts/e2e-m4a1.sh` the managed `.yard` (and that `git status` stays clean),
`scripts/e2e-m4b.sh` services: it adds MySQL, PostgreSQL, Redis and MinIO
(installing what is missing with brew), asks each one a question, and removes what it added,
`scripts/e2e-m4c.sh` Node: it installs Node 16.14.0 and 22 with fnm, runs them through the
shims (a native module build included), and removes only what it installed, and
`scripts/e2e-m4e.sh` sleeping services: it lets scratch Redis, MySQL 8.0 and PostgreSQL 17
instances fall asleep, times each one's cold start, wakes them from a page and from `php`,
keeps them awake with held connections, and removes them, and `scripts/e2e-m4d.sh`
emailulator: against the agent already installed (it never installs, starts or stops it), it
checks `yard doctor`, the external port entries, `mail.yard:8080` and a project's `MAIL_PORT`.

`yard start` installs `~/Library/LaunchAgents/dev.more.yard.daemon.plist` and starts
`yardd` from beside the `yard` binary. Data lives in
`~/.yard` (or `YARD_ROOT`, or `--root <path>`).

## HTTPS

`yard secure` serves a site at `https://<site>.yard:8443/` (443 in the privileged
profile) and answers its http URL with a `301` there. `secure_by_default = true` in
`yard.toml` secures every site; `yard unsecure <site>` still opts one out. yardd makes a
local CA on first use (`ca/yard-ca.pem`, key mode 0600) and issues each secured site a
certificate for `<site>.yard` and `*.<site>.yard`, renewing it 30 days before it
expires and reissuing it when the TLD or the CA changes. The CA itself is remade about
825 days before its 10 years run out (the old one is kept as `ca/yard-ca.pem.bak`), and
then needs `yard trust` again.

Run `yard trust` once so Safari and Chrome accept those certificates: it runs
`security add-trusted-cert` against your login keychain, as you, and macOS asks for your
password. `yard untrust` removes it. Until then `yard doctor` warns. Yard's `node` shim sets
`NODE_EXTRA_CA_CERTS` to Yard's CA when you have not set it; any other Node needs
`NODE_EXTRA_CA_CERTS=<path to yard-ca.pem>`, curl takes `--cacert`, and Firefox needs
`security.enterprise_roots.enabled` set in `about:config`; `yard doctor` prints the path
and the Firefox note.

## Old PHP and MySQL

PHP 7.0 to 7.3 can log in to MySQL only with `mysql_native_password`, so a MySQL 8 instance
Yard adds makes it the default, for `root` too. Every MySQL instance also defaults to
`utf8mb4_unicode_ci` and the lenient `sql_mode = NO_ENGINE_SUBSTITUTION`, so old apps' loose
`GROUP BY` and zero dates work (Laravel sets its own `sql_mode`). Put a different setting under
`[mysqld]` in the instance's `overrides.conf`. MySQL 9 has no `mysql_native_password`: use
`mysql = "8.4"` for an old project, and `yard doctor` warns when one asks for 9.

## Sleeping services

Service instances can sleep when nothing uses them, which frees their memory. It is off
until you turn it on: `service_sleep_after = "15m"` in `yard.toml` for every instance, or
`yard service sleep-after <instance> 15m` for one (`off` keeps it awake, `default` follows
`yard.toml`; at least `60s`). An instance with no client, no connection closed since the last
look (so a `schedule:run` that connects for a moment every minute counts) and no wake for that
long is stopped gracefully, once yardd has checked again that nothing connected meanwhile; its
port stays its own. It wakes when a PHP request comes in for
a site whose `.yard` lists it, when `php` or `composer` runs in that site (they print
`yard: waking the services <site> uses …` and wait up to 30 s), or with
`yard service start <instance>`. A GUI client or a `mysql` run outside a site cannot wake one
and gets "connection refused": start it, or set `off` for an instance used that way. If
another program takes a sleeping instance's port, the site's pages answer 503 and `php`
refuses to run, naming the program, rather than talk to it; `yard doctor` shows it. Once that
program lets go of the port, the next page or `php` starts the instance again. On a site that
can wake services, a server-level `error_page` in its nginx override no longer applies to PHP
requests (the wake's own `error_page` takes its place); PHP's own error pages are unaffected.
`yard services` shows `sleeping 2h` and the SLEEP setting, and every instance starts awake
with yardd. Setting it back to `off`, or removing the key, wakes an instance that sleeps.
While nothing may sleep, PHP requests never ask yardd anything.

## Commands

Every command has `--help`, accepts `--json` for scripts and the global `--root <path>`.
Exit codes: 0 success, 1 a Yard error (`error:` and `hint:` lines on stderr), 2 a usage
error, 3 `yardd` is not running.

| Command | Needs yardd | What it does |
|---|---|---|
| `yard start` | no | Installs yardd's LaunchAgent and starts it. |
| `yard stop` | no | Stops yardd and unloads its LaunchAgent. |
| `yard status` | yes | Profile, TLD, units and site count; a sleeping service instance is a row of its own (`sleeping`). |
| `yard version`, `--version`, `-V` | optional | CLI, daemon, helper and protocol versions. |
| `yard doctor` | optional | Checks the installation and prints the fix for each problem. It tells yardd which `XDG_CONFIG_HOME` and `HOMEBREW_FORBIDDEN_*` your shell sets (and nothing else), so it can warn when brew would see something else under launchd. |
| `yard sites` | yes | The sites Yard serves, with the directory each one serves and whether it is `serving` or `needs setup`. |
| `yard link [name] [--yes]` / `unlink [name]` | yes | Serves the current directory as `<name>.yard`. When its `.yard` needs a PHP, extension or service that is not installed, it asks before installing (`--yes` does not ask). A Node the project pins that is not installed is offered too, and installed last; it never stops the site serving, so without a terminal it only prints the `yard node install` hint, and a Node install that fails is a warning, not an error. Without a committed `.yard` it uses the managed one written for this directory (`Using <file>`). `unlink` never deletes a managed file. |
| `yard init [name] [--local] [--force] [--replace] [--yes]` | yes | Writes a `.yard` for the current project from its settings: the managed one in Yard's data root (`sites/<name>/.yard`, nothing added to the repo), or with `--local` `<project>/.yard`, moving a managed one there. A committed `.yard` always wins and is applied. It links the project, moves its `php`, `node`, `secure` and `docroot` out of `sites.toml`, and installs what the file needs (PHP, then extensions, then services, then Node) after asking. An existing `.yard` is kept; `--force` rewrites the keys Yard detects and keeps the rest. When the name already serves another directory, it moves that site here, keeping how it serves; it asks first (or takes `--replace`) only when this directory has its own `.yard`, the name's managed `.yard` was written for another directory (kept as `.yard-backup`), or the name is a proxy. |
| `yard proxy <name> <url>` / `unproxy <name>` | yes | Serves `<name>.yard` by proxying to a URL. |
| `yard docroot <path> [--site <name>]` | yes | Sets the directory a site serves, e.g. `public`. Refused for a site with a committed `.yard`, which owns its docroot: edit the file. For a managed `.yard` it edits that file. |
| `yard isolate <ver> [--site <name>]` / `unisolate [--site <name>]` | yes | Runs a site on its own installed PHP version, or the default again. The site defaults to the one containing the current directory. Refused for a site with a committed `.yard`: edit its `php`. For a managed `.yard` it edits that file. |
| `yard secure [site]` / `unsecure [site]` | yes | Serves a site over HTTPS with a certificate from Yard's CA, its http URL redirecting there, or plain HTTP again. The site defaults to the one containing the current directory. Refused for a site with a committed `.yard`: edit its `secure`. For a managed `.yard` it edits that file. |
| `yard trust` / `untrust` | yes | Adds Yard's CA to your login keychain as trusted, or removes it; macOS asks for your password. |
| `yard php [list]` | yes | Installed PHP versions, the default, the sites on each and its pool. |
| `yard php install <ver>` / `uninstall <ver>` | yes | Installs or removes a PHP version (7.0–8.5) with brew, showing brew's output; Ctrl-C cancels it. Homebrew 6+ must trust the formula or its tap (`brew trust`): Yard asks `brew trust --json=v1` first and refuses with `FORMULA_UNTRUSTED` instead of installing; on a terminal the CLI then asks `Trust <formula> (from the <tap> tap)? [y/N]` and, on yes, trusts that formula and retries. `--yes` never answers it. `uninstall` is refused while the version is the default, a site is isolated to it or names it in its `.yard`, or it has extensions you added. |
| `yard php default <ver>` | yes | Makes an installed version the default. |
| `yard node [list] [--available [<major>]]` | yes | Installed Node versions, the default and the sites on each; `--available` adds the newest release of each line (or one major) from nodejs.org. |
| `yard node install [<spec>]` / `uninstall <version>` | yes | Installs a Node version (`22`, `22.17`, `16.14.0`, `lts/iron`, `lts/*`; with none, the one the current directory pins) with fnm, installing fnm with brew first if it is missing; Ctrl-C cancels it. `uninstall` takes an installed version and is refused with `NODE_IN_USE` while the default or a linked site would be left with no version to run. |
| `yard node default <spec>` | yes | Sets the Node for sites and directories that pin none (`22` until you change it); it must be installed. |
| `yard node use <spec> [--site <name>]` / `unuse [--site <name>]` | yes | Pins a site's Node, in its managed `.yard` or else `sites.toml`, or removes that pin. Refused for a site with a committed `.yard`: edit its `node`. |
| `yard php config get\|set\|unset <key> [value] [--php <ver>]` | yes | Reads or changes your php.ini overrides (`99-overrides.ini`) and reloads that version's pool. |
| `yard ports` | yes | Registered ports with their mode, status and holder. |
| `yard ports check` | optional | Reads `ports.toml` as it is now and probes every port in it; exits 1 on a conflict or a file that does not parse. |
| `yard ports set <unit> <port>`, `pin`, `unpin`, `range <a-b>`, `profile <standard\|privileged>` | yes | Changes the port registry. `privileged` puts nginx on 80/443. |
| `yard ext add <name\|name-version\|path.so> [--php <ver>]` | yes | Installs a PHP extension from brew's `shivammathur/extensions` (the same trust check as `php install`, unless the tap's files do not list the formula, renames and aliases included), else pecl (`xdebug-2.9.8` pins a pecl release for legacy PHP), or copies a built `.so`, then checks it loads with `php -m`. Ctrl-C cancels it and undoes what it recorded. |
| `yard ext list` / `enable <name>` / `disable <name>` / `remove <name>` `[--php <ver>]` | yes | Lists extensions, turns one on or off without uninstalling it (PHP's bundled ones too), or uninstalls one. `brew uninstall` leaves the extension's ini in brew's `conf.d`; Yard skips it, and `yard doctor` names the `rm` that removes it. |
| `yard xdebug on\|off [--php <ver>]` | yes | Turns an added Xdebug on or off. |
| `yard services` | yes | One row per service instance: `INSTANCE ENGINE VERSION PORT STATE SLEEP ACCOUNT USED BY MANAGED BY`. STATE reads `sleeping 2h` or `waking` for a sleeping instance, and SLEEP is its `sleep_after` (`15m`, `off`), or `-` for emailulator, which Yard never puts to sleep. ACCOUNT is the login Yard set up (`root (no password)`, `— (no auth)`, `minioadmin / minioadmin`). Yard stores no other password. MANAGED BY is `yard`, or `LaunchAgent <label>` for emailulator, whose row shows while its agent is installed. Each web UI (MinIO's console, emailulator's `mail.<tld>`) is listed under the table. |
| `yard service add <engine> [--version <v>] [--port <p>] [--yes]` | yes | Adds `mysql`, `postgres`, `redis` or `minio`: installs the formula with brew if its keg is missing (MinIO comes from the `minio/stable` tap, so the trust question applies), pins a port, initializes the data, starts it, waits for it to pass its health check and prints its `.env` lines. Shows brew's output; Ctrl-C cancels and undoes it. `--yes` never grants trust. `emailulator` is different: Yard never installs it (use its installer); `add` runs `emailulator service install` (`--port` is its SMTP port) unless its LaunchAgent already catches mail, then records which binary to use (the first on your PATH); a failed install records nothing, and `--port` is refused while an agent already listens on another port. While its agent is installed, `start`, `stop`, `restart`, `port`, `sleep-after` and `remove` refuse emailulator with `SERVICE_EXTERNAL`, naming emailulator's own command; with none, `remove` forgets the recorded binary. `logs` reads its agent's log. |
| `yard service remove <instance> [--keep-data] [--yes]` | yes | Checks the instance first: unknown is `SERVICE_NOT_FOUND`, and one a linked site's `.yard` lists is refused with `SERVICE_IN_USE` (naming the sites and the file to edit) before anything is asked. Then asks `Delete <instance> and its data? [y/N]` (`--keep-data` leaves `services/<instance>/data`) and removes it. Without a terminal and without `--yes` it deletes nothing and fails with `NEEDS_CONFIRMATION`. Never runs `brew uninstall`. |
| `yard service start\|stop\|restart <instance>` | yes | `start` and `restart` wait until the instance is running, and wake a sleeping one; one that fails prints its last log lines and exits 1. `stop` keeps it stopped across yardd restarts until `start`. |
| `yard service logs <instance> [-f] [-n <lines>]` | yes | The last 100 lines (at most 10000) of the instance's log; `-f` keeps following until Ctrl-C. |
| `yard service port <instance> <port>` | yes | Moves an instance to another port; a running one restarts on it. |
| `yard service sleep-after <instance> <duration\|off\|default>` | yes | Sets how long the instance may go unused before it sleeps (`sleep_after` in `services.toml`; at least `60s`), `off` to keep it awake, or `default` to follow `service_sleep_after` in `yard.toml`. With neither set, an instance sleeps after 15 minutes unused (since 2026-11-03); a GUI client such as TablePlus cannot wake it, so `yard service start <instance>` first, or `off` for an instance you use that way. |
| `yard env [--site <name>] [--service <instance>]` | yes | Prints `KEY=value` lines under a `# <instance>` comment per service, for you to paste into `.env`; it never writes a file. In a linked project (or with `--site`) it prints the services its `.yard` lists; outside one, every instance. emailulator's block is the seven `MAIL_*` lines, with a `warning:` on stderr while its agent does not answer. While the site is shared through a tunnel, a `# tunnel` block prints its public `APP_URL`. Yard creates no database or bucket, so there is no `DB_DATABASE` or `AWS_BUCKET`. |
| `yard info [site]` | yes | What one site runs on: path, `.yard`, PHP (full version), Node (with the spec it pins), docroot, HTTPS, extensions, each service with its state, address and account, and unmet needs with the command that fixes each. Each setting says whether it comes from `.yard`, the managed `.yard` (`managed`), `sites.toml`, `.nvmrc`, `.node-version` or the default. |
| `yard sources` | yes | The brew taps Yard installs from (`shivammathur/php`, `shivammathur/extensions`, `minio/stable`): tapped, trusted, and the formulae trusted or installed. |
| `yard source trust\|untrust <formula> [--tap]` | yes | Runs `brew trust` / `brew untrust` for one formula, or with `--tap` a whole tap. Needs no terminal. Untrusting something installed does not stop it serving, but brew will no longer load it. |
| `yard install-shims` | no | Prints the PATH line that puts Yard's `php`, `composer`, `pecl`, `phpize`, `php-config`, `node`, `npm` and `npx` first. The PHP ones follow the nearest `.yard`, then the site's managed `.yard`, then the site's isolated version, then `.yard-php`, then the default; the Node ones the nearest pin, where in one directory a `.yard`'s `node` beats the site's pin, which beats `.nvmrc`, then `.node-version`; with none, the default. Warns when another `php` or `node` would still win, naming Herd's, nvm's, mise's, fnm's or brew's. |
| `yard tunnel [site] [--quick \| --host <hostname>] [--tunnel <name\|uuid>] [--overwrite-dns]` | yes | Shares a site at a public URL through cloudflared (`brew install cloudflared`). `--quick` gives a random `trycloudflare.com` URL until yardd stops; `--host` a name on a domain set up with `yard tunnels domain add`, kept in `sites.toml` until `yard untunnel`. With neither, it asks when a domain is set up (and without a terminal fails with `TUNNEL_KIND_REQUIRED`, exit 2), and makes a quick tunnel when none is. Prints the URL and the `APP_URL` to use. |
| `yard untunnel [site] [--keep-tunnel]` | yes | Stops sharing a site. For a named tunnel it deletes the Cloudflare tunnel Yard made (unless `--keep-tunnel`, and never one adopted with `--tunnel`) and names the DNS record left for you to delete in Cloudflare's dashboard. |
| `yard tunnels` | yes | One row per tunnel: `SITE URL KIND STATE TUNNEL`. |
| `yard tunnels domain add <domain>` / `remove <domain>`, `yard tunnels domains` | yes | Signs cloudflared in to Cloudflare for a domain in your browser (in a terminal only) and records it in `yard.toml`; `remove` forgets it and keeps the sign-in; `domains` lists them with `SIGNED IN`. |
| `yard restart <unit>` | yes | Restarts nginx (after checking its config), dnsmasq, `php-fpm-<version>` or `tunnel-<site>`. |
| `sudo yard helper install` / `uninstall` | no | Installs or removes the root helper. |

## Development

See [`AGENTS.md`](AGENTS.md) for the build, test and lint commands and the conventions.
