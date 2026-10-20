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
and `yard trust` puts that CA in your login keychain. Yard.app (M4), and Node and
services (M5) come later.

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
`scripts/e2e-m2.sh` checks multi-PHP and extensions, and `scripts/e2e-m3.sh` HTTPS.

`yard start` installs `~/Library/LaunchAgents/dev.more.yard.daemon.plist` and starts
`yardd` from beside the `yard` binary. Data lives in
`~/Library/Application Support/Yard` (or `YARD_ROOT`, or `--root <path>`).

## HTTPS

`yard secure` serves a site at `https://<site>.yard:8443/` (443 in the privileged
profile) and answers its http URL with a `301` there. `secure_by_default = true` in
`yard.toml` secures every site; `yard unsecure <site>` still opts one out. yardd makes a
local CA on first use (`ca/yard-ca.pem`, key mode 0600) and issues each secured site a
certificate for `<site>.yard` and `*.<site>.yard`, renewing it 30 days before it
expires and reissuing it when the TLD or the CA changes.

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
| `yard doctor` | optional | Checks the installation and prints the fix for each problem. |
| `yard sites` | yes | The sites Yard serves, with the directory each one serves. |
| `yard link [name]` / `unlink [name]` | yes | Serves the current directory as `<name>.yard`. |
| `yard proxy <name> <url>` / `unproxy <name>` | yes | Serves `<name>.yard` by proxying to a URL. |
| `yard docroot <path> [--site <name>]` | yes | Sets the directory a site serves, e.g. `public`. |
| `yard isolate <ver> [--site <name>]` / `unisolate [--site <name>]` | yes | Runs a site on its own installed PHP version, or the default again. The site defaults to the one containing the current directory. |
| `yard secure [site]` / `unsecure [site]` | yes | Serves a site over HTTPS with a certificate from Yard's CA, its http URL redirecting there, or plain HTTP again. The site defaults to the one containing the current directory. |
| `yard trust` / `untrust` | yes | Adds Yard's CA to your login keychain as trusted, or removes it; macOS asks for your password. |
| `yard php [list]` | yes | Installed PHP versions, the default, the sites on each and its pool. |
| `yard php install <ver>` / `uninstall <ver>` | yes | Installs or removes a PHP version (7.0–8.5) with brew, showing brew's output; Ctrl-C cancels it. `uninstall` is refused while the version is the default, a site is isolated to it, or it has extensions you added. |
| `yard php default <ver>` | yes | Makes an installed version the default. |
| `yard php config get\|set\|unset <key> [value] [--php <ver>]` | yes | Reads or changes your php.ini overrides (`99-overrides.ini`) and reloads that version's pool. |
| `yard ports` | yes | Registered ports with their mode, status and holder. |
| `yard ports check` | optional | Reads `ports.toml` as it is now and probes every port in it; exits 1 on a conflict or a file that does not parse. |
| `yard ports set <unit> <port>`, `pin`, `unpin`, `range <a-b>`, `profile <standard\|privileged>` | yes | Changes the port registry. `privileged` puts nginx on 80/443. |
| `yard ext add <name\|name-version\|path.so> [--php <ver>]` | yes | Installs a PHP extension from brew's `shivammathur/extensions`, else pecl (`xdebug-2.9.8` pins a pecl release for legacy PHP), or copies a built `.so`, then checks it loads with `php -m`. Ctrl-C cancels it and undoes what it recorded. |
| `yard ext list` / `enable <name>` / `disable <name>` / `remove <name>` `[--php <ver>]` | yes | Lists extensions, turns one on or off without uninstalling it (PHP's bundled ones too), or uninstalls one. |
| `yard xdebug on\|off [--php <ver>]` | yes | Turns an added Xdebug on or off. |
| `yard install-shims` | no | Prints the PATH line that puts Yard's `php`, `composer`, `pecl`, `phpize` and `php-config` first; they follow the site's isolated version, then `.yard-php`, then the default. Warns when Herd's `php` would still win. |
| `yard restart <unit>` | yes | Restarts nginx (after checking its config), dnsmasq or `php-fpm-<version>`. |
| `sudo yard helper install` / `uninstall` | no | Installs or removes the root helper. |

## Development

See [`AGENTS.md`](AGENTS.md) for the build, test and lint commands and the conventions.
