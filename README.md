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
fix. M4b adds services: MySQL, PostgreSQL, Redis and MinIO instances Yard runs from
brew's kegs (`yard service add`), a `[services]` table in `.yard`, `yard env` for the
`.env` lines, `yard info` for what one site runs on, and `yard source` for the brew
taps you trust. Node (M4c), tunnels (M5) and Yard.app (M6) come later.

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
the `.yard` file, and `scripts/e2e-m4b.sh` services: it adds MySQL, PostgreSQL, Redis and MinIO
(installing what is missing with brew), asks each one a question, and removes what it added.

`yard start` installs `~/Library/LaunchAgents/dev.more.yard.daemon.plist` and starts
`yardd` from beside the `yard` binary. Data lives in
`~/Library/Application Support/Yard` (or `YARD_ROOT`, or `--root <path>`).

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
password. `yard untrust` removes it. Until then `yard doctor` warns. Node needs
`NODE_EXTRA_CA_CERTS=<path to yard-ca.pem>`, curl takes `--cacert`, and Firefox needs
`security.enterprise_roots.enabled` set in `about:config`; `yard doctor` prints the path
and the Firefox note.

## Commands

Every command has `--help`, accepts `--json` for scripts and the global `--root <path>`.
Exit codes: 0 success, 1 a Yard error (`error:` and `hint:` lines on stderr), 2 a usage
error, 3 `yardd` is not running.

| Command | Needs yardd | What it does |
|---|---|---|
| `yard start` | no | Installs yardd's LaunchAgent and starts it. |
| `yard stop` | no | Stops yardd and unloads its LaunchAgent. |
| `yard status` | yes | Profile, TLD, units and site count. |
| `yard version`, `--version`, `-V` | optional | CLI, daemon, helper and protocol versions. |
| `yard doctor` | optional | Checks the installation and prints the fix for each problem. It tells yardd which `XDG_CONFIG_HOME` and `HOMEBREW_FORBIDDEN_*` your shell sets (and nothing else), so it can warn when brew would see something else under launchd. |
| `yard sites` | yes | The sites Yard serves, with the directory each one serves and whether it is `serving` or `needs setup`. |
| `yard link [name] [--yes]` / `unlink [name]` | yes | Serves the current directory as `<name>.yard`. When its `.yard` needs a PHP, extension or service that is not installed, it asks before installing (`--yes` does not ask). |
| `yard init [name] [--force] [--replace] [--yes]` | yes | Writes `.yard` for the current project from its settings, links it, moves its `php`, `secure` and `docroot` out of `sites.toml`, and installs what the file needs (PHP, then extensions, then services) after asking. An existing `.yard` is kept; `--force` rewrites the keys Yard detects and keeps the rest. When the name already serves another directory, it moves that site here, keeping how it serves; it asks first (or takes `--replace`) only when this directory has its own `.yard` or the name is a proxy. |
| `yard proxy <name> <url>` / `unproxy <name>` | yes | Serves `<name>.yard` by proxying to a URL. |
| `yard docroot <path> [--site <name>]` | yes | Sets the directory a site serves, e.g. `public`. Refused for a site with a `.yard`, which owns its docroot: edit the file. |
| `yard isolate <ver> [--site <name>]` / `unisolate [--site <name>]` | yes | Runs a site on its own installed PHP version, or the default again. The site defaults to the one containing the current directory. Refused for a site with a `.yard`: edit its `php`. |
| `yard secure [site]` / `unsecure [site]` | yes | Serves a site over HTTPS with a certificate from Yard's CA, its http URL redirecting there, or plain HTTP again. The site defaults to the one containing the current directory. Refused for a site with a `.yard`: edit its `secure`. |
| `yard trust` / `untrust` | yes | Adds Yard's CA to your login keychain as trusted, or removes it; macOS asks for your password. |
| `yard php [list]` | yes | Installed PHP versions, the default, the sites on each and its pool. |
| `yard php install <ver>` / `uninstall <ver>` | yes | Installs or removes a PHP version (7.0–8.5) with brew, showing brew's output; Ctrl-C cancels it. Homebrew 6+ must trust the formula or its tap (`brew trust`): Yard asks `brew trust --json=v1` first and refuses with `FORMULA_UNTRUSTED` instead of installing; on a terminal the CLI then asks `Trust <formula> (from the <tap> tap)? [y/N]` and, on yes, trusts that formula and retries. `--yes` never answers it. `uninstall` is refused while the version is the default, a site is isolated to it or names it in its `.yard`, or it has extensions you added. |
| `yard php default <ver>` | yes | Makes an installed version the default. |
| `yard php config get\|set\|unset <key> [value] [--php <ver>]` | yes | Reads or changes your php.ini overrides (`99-overrides.ini`) and reloads that version's pool. |
| `yard ports` | yes | Registered ports with their mode, status and holder. |
| `yard ports check` | optional | Reads `ports.toml` as it is now and probes every port in it; exits 1 on a conflict or a file that does not parse. |
| `yard ports set <unit> <port>`, `pin`, `unpin`, `range <a-b>`, `profile <standard\|privileged>` | yes | Changes the port registry. `privileged` puts nginx on 80/443. |
| `yard ext add <name\|name-version\|path.so> [--php <ver>]` | yes | Installs a PHP extension from brew's `shivammathur/extensions` (the same trust check as `php install`, unless the tap's files do not list the formula, renames and aliases included), else pecl (`xdebug-2.9.8` pins a pecl release for legacy PHP), or copies a built `.so`, then checks it loads with `php -m`. Ctrl-C cancels it and undoes what it recorded. |
| `yard ext list` / `enable <name>` / `disable <name>` / `remove <name>` `[--php <ver>]` | yes | Lists extensions, turns one on or off without uninstalling it (PHP's bundled ones too), or uninstalls one. |
| `yard xdebug on\|off [--php <ver>]` | yes | Turns an added Xdebug on or off. |
| `yard services` | yes | One row per service instance: `INSTANCE ENGINE VERSION PORT STATE ACCOUNT USED BY`. ACCOUNT is the login Yard set up (`root (no password)`, `— (no auth)`, `minioadmin / minioadmin`). Yard stores no other password. |
| `yard service add <engine> [--version <v>] [--port <p>] [--yes]` | yes | Adds `mysql`, `postgres`, `redis` or `minio`: installs the formula with brew if its keg is missing (MinIO comes from the `minio/stable` tap, so the trust question applies), pins a port, initializes the data, starts it, waits for it to pass its health check and prints its `.env` lines. Shows brew's output; Ctrl-C cancels and undoes it. `--yes` never grants trust. |
| `yard service remove <instance> [--keep-data] [--yes]` | yes | Checks the instance first: unknown is `SERVICE_NOT_FOUND`, and one a linked site's `.yard` lists is refused with `SERVICE_IN_USE` (naming the sites and the file to edit) before anything is asked. Then asks `Delete <instance> and its data? [y/N]` (`--keep-data` leaves `services/<instance>/data`) and removes it. Without a terminal and without `--yes` it deletes nothing and fails with `NEEDS_CONFIRMATION`. Never runs `brew uninstall`. |
| `yard service start\|stop\|restart <instance>` | yes | `start` and `restart` wait until the instance is running; one that fails prints its last log lines and exits 1. `stop` keeps it stopped across yardd restarts until `start`. |
| `yard service logs <instance> [-f] [-n <lines>]` | yes | The last 100 lines (at most 10000) of the instance's log; `-f` keeps following until Ctrl-C. |
| `yard service port <instance> <port>` | yes | Moves an instance to another port; a running one restarts on it. |
| `yard env [--site <name>] [--service <instance>]` | yes | Prints `KEY=value` lines under a `# <instance>` comment per service, for you to paste into `.env`; it never writes a file. In a linked project (or with `--site`) it prints the services its `.yard` lists; outside one, every instance. Yard creates no database or bucket, so there is no `DB_DATABASE` or `AWS_BUCKET`. |
| `yard info [site]` | yes | What one site runs on: path, `.yard`, PHP (full version), docroot, HTTPS, extensions, each service with its state, address and account, and unmet needs with the command that fixes each. Each setting says whether it comes from `.yard`, `sites.toml` or the default. |
| `yard sources` | yes | The brew taps Yard installs from (`shivammathur/php`, `shivammathur/extensions`, `minio/stable`): tapped, trusted, and the formulae trusted or installed. |
| `yard source trust\|untrust <formula> [--tap]` | yes | Runs `brew trust` / `brew untrust` for one formula, or with `--tap` a whole tap. Needs no terminal. Untrusting something installed does not stop it serving, but brew will no longer load it. |
| `yard install-shims` | no | Prints the PATH line that puts Yard's `php`, `composer`, `pecl`, `phpize` and `php-config` first; they follow the nearest `.yard`, then the site's isolated version, then `.yard-php`, then the default. Warns when Herd's `php` would still win. |
| `yard restart <unit>` | yes | Restarts nginx (after checking its config), dnsmasq or `php-fpm-<version>`. |
| `sudo yard helper install` / `uninstall` | no | Installs or removes the root helper. |

## Development

See [`AGENTS.md`](AGENTS.md) for the build, test and lint commands and the conventions.
