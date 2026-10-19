# Yard

Yard serves local PHP sites on macOS, as a replacement for Laravel Herd. A Rust daemon
(`yardd`, a user LaunchAgent) supervises nginx, dnsmasq and a php-fpm pool per PHP
version, all from Homebrew. A small root helper (`yard-helper`, a LaunchDaemon) does the
few things that need root through a fixed set of verbs. The `yard` CLI talks to the
daemon over a unix socket.

The design is in [`docs/superpowers/specs/2026-10-19-yard-design.md`](docs/superpowers/specs/2026-10-19-yard-design.md).

## Status

Milestone 1 (core serving, CLI only) is in progress: parked and linked PHP sites,
proxies, the port registry, `yard doctor` and the commands below. HTTPS (M3), PHP
installs and extensions (M2), Node and services, and Yard.app (M4) come later.

Yard runs in the **trial profile** until `yard takeover` (M3): sites are
`http://<site>.yard:8080`, and Herd keeps its `.test` domain, ports 80/443 and
`/etc/resolver/test` untouched.

## Prerequisites

- An Apple Silicon Mac with Homebrew at `/opt/homebrew`.
- Rust 1.92 (`rust-toolchain.toml` pins it).
- nginx, dnsmasq and PHP from Homebrew. Do not start them with `brew services`; Yard
  runs them itself.

```sh
brew install nginx dnsmasq shivammathur/php/php@8.4
```

A `dnsmasq` keg with no files in it needs `brew reinstall dnsmasq` instead. `yard doctor`
says which of these is missing.

## Install

```sh
cargo build --release
sudo ./target/release/yard helper install   # once, and after each upgrade
./target/release/yard start
./target/release/yard doctor
```

`yard start` installs `~/Library/LaunchAgents/dev.more.yard.daemon.plist` and starts
`yardd` from beside the `yard` binary. Data lives in
`~/Library/Application Support/Yard` (or `YARD_ROOT`, or `--root <path>`).

## Commands

Every command has `--help`, accepts `--json` for scripts and the global `--root <path>`.
Exit codes: 0 success, 1 a Yard error (`error:` and `hint:` lines on stderr), 2 a usage
error, 3 `yardd` is not running.

| Command | Needs yardd | What it does |
|---|---|---|
| `yard start` | no | Installs yardd's LaunchAgent and starts it. |
| `yard stop` | no | Stops yardd and unloads its LaunchAgent. |
| `yard status` | yes | Profile, TLD, parked paths, units and site count. |
| `yard version`, `--version`, `-V` | optional | CLI, daemon, helper and protocol versions. |
| `yard doctor` | optional | Checks the installation and prints the fix for each problem. |
| `yard sites` | yes | The sites Yard serves. |
| `yard park [path]` / `unpark [path]` | yes | Serves every directory inside a path (default: the current one). |
| `yard link [name]` / `unlink [name]` | yes | Serves the current directory as `<name>.yard`. |
| `yard proxy <name> <url>` / `unproxy <name>` | yes | Serves `<name>.yard` by proxying to a URL. |
| `yard docroot <path> [--site <name>]` | yes | Sets the directory a site serves, e.g. `public`. |
| `yard ports` | yes | Registered ports with their mode, status and holder. |
| `yard ports check` | optional | Probes every registered port now; exits 1 on a conflict. |
| `yard ports set <unit> <port>`, `pin`, `unpin`, `range <a-b>`, `profile <trial\|default>` | yes | Changes the port registry. |
| `yard restart <unit>` | yes | Restarts nginx, dnsmasq or `php-fpm-<version>`. |
| `sudo yard helper install` / `uninstall` | no | Installs or removes the root helper. |

## Development

See [`AGENTS.md`](AGENTS.md) for the build, test and lint commands and the conventions.
