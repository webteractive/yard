# Developing Yard

How to build Yard from source, run it, and check it on a real Mac. [`AGENTS.md`](AGENTS.md)
has the full build, test and lint commands and the project's conventions, and the design is in
[`docs/superpowers/specs/2026-10-19-yard-design.md`](docs/superpowers/specs/2026-10-19-yard-design.md).

## How it fits together

A Rust daemon (`yardd`, a user LaunchAgent) runs nginx, dnsmasq and a php-fpm pool per PHP
version, all from Homebrew and all as the user. A small root helper (`yard-helper`, a
LaunchDaemon) does the few things that need root through a fixed set of verbs: it writes
`/etc/resolver/<tld>`, and in the privileged profile it opens ports 80 and 443 on loopback and
hands the sockets to yardd. Root never runs nginx. The `yard` CLI and Yard.app talk to the
daemon over a unix socket.

The root workspace holds the daemon, helper, CLI, shims and their shared crates; `crates/` lists
them. Yard.app is a second workspace in `app/` (Rust and GPUI) on its own toolchain.

## Prerequisites

- An Apple Silicon Mac with Homebrew at `/opt/homebrew`.
- Rust 1.92 from rustup (`rust-toolchain.toml` pins it). Homebrew's `rust` formula ignores
  the pin, so use rustup instead of it. Yard.app needs Rust 1.98.1 (`app/rust-toolchain.toml`)
  and Xcode 26 for the macOS 26 SDK.
- nginx, dnsmasq and PHP from Homebrew; Yard runs them itself. `yard start` installs nginx
  and dnsmasq when they are missing, after asking (`--yes` does not ask), and `yard link`
  installs the PHP a project needs. `yard tunnel` also needs `brew install cloudflared`, and
  the e2e scripts need `jq` and `shellcheck`.

Yard only uses the installed binaries; do not start them with `brew services`. A `dnsmasq` keg
with no files in it needs `brew reinstall dnsmasq`. `yard doctor` says which of these is missing.

## Build and run

```sh
cargo build --release
ln -sf "$PWD/target/release/yard" ~/.local/bin/yard   # any directory on PATH
sudo ./target/release/yard helper install             # in a real terminal
yard start
yard doctor                                           # every check should say ok
```

The symlink keeps `yard` current across rebuilds; it still finds `yardd` and `yard-helper`
beside the binary it points at. `yard start` installs
`~/Library/LaunchAgents/dev.more.yard.daemon.plist` and starts `yardd` from beside the `yard`
binary. A rebuilt yardd runs after `yard stop && yard start`.

Run the helper install in a real terminal: sudo needs one to ask for the password. It is needed
again only when the helper's revision changes; `yard doctor` says so. Re-run over a helper from
before nginx ran as the user, it stops the root nginx that helper started and removes the two
files it left owned by root (`logs/nginx-error.log`, `run/nginx.pid`), naming each file and the
next step.

Yard.app: `cd app && cargo test --workspace --locked`, and `app/scripts/bundle.sh` builds
`app/target/Yard.app` (ad hoc signed, for this Mac).

## Tests

`cargo test --workspace` runs the root suite; it uses a temporary data root and free ports, and
never touches the real `~/.yard`, `/etc/resolver` or ports 53, 80, 443, 8080, 8443 and 53535.
CI also runs `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo deny check` gates dependency changes.

The `scripts/e2e-*.sh` scripts check the real daemon on a real Mac, against `~/.yard`, and clean
up what they add:

- `e2e-m1.sh` links a PHP app and proxies another, curls both on `.yard:8080`, and, where 80 and
  443 are free, switches to the privileged profile and back.
- `e2e-m2.sh` checks multi-PHP and extensions, and `e2e-m3.sh` HTTPS.
- `e2e-m4a.sh` checks the `.yard` file, and `e2e-m4a1.sh` the managed `.yard` (and that
  `git status` stays clean).
- `e2e-m4b.sh` adds MySQL, PostgreSQL, Redis and MinIO (installing what is missing with brew),
  asks each one a question, and removes what it added.
- `e2e-m4c.sh` installs Node 16.14.0 and 22 with fnm, runs them through the shims (a native
  module build included), and removes only what it installed.
- `e2e-m4d.sh` checks emailulator against the agent already installed (it never installs,
  starts or stops it): `yard doctor`, the external port entries, `mail.yard:8080` and a
  project's `MAIL_PORT`.
- `e2e-m4e.sh` lets scratch Redis, MySQL 8.0 and PostgreSQL 17 instances fall asleep, times
  each one's cold start, wakes them from a page and from `php`, and keeps them awake with held
  connections.
- `e2e-m5.sh` checks a quick tunnel with no Cloudflare account (it needs the network and brew's
  `cloudflared`): PHP behind the `trycloudflare.com` URL sees HTTPS, the public host and the
  visitor's address, and `yard untunnel` leaves nothing behind.

`YARD=<path to yard>` points a script at another build, such as an installed release
(`/Applications/Yard.app/Contents/Helpers/yard`). Yard.app's checklists are
`app/scripts/checklist.sh`, `checklist-installs.sh` and `checklist-real.sh`; AGENTS.md says what
each needs.

## A scratch daemon

`cargo run -p yard-daemon --bin yardd -- --root /tmp/yard-dev` runs a daemon on a scratch data
root, logging to stderr, and `cargo run -p yard-cli -- --root /tmp/yard-dev sites` talks to it.
Pin its nginx and dnsmasq away from the real ones in its `ports.toml`. It leaves `/etc/resolver`
and ports 80/443 alone. `open app/target/Yard.app --args --root /tmp/yard-dev` runs the app
against it.

## Releasing

`scripts/release.sh X.Y.Z` builds, signs with the keychain's Developer ID, notarizes, and
publishes: it tags the release, uploads the app to GitHub Releases, updates the cask in
`webteractive/homebrew-tap`, and installs the published release on the Mac it runs on.
`--no-publish` stops before publishing, and `--adhoc` builds an ad hoc signed copy for a Mac
without the certificate. AGENTS.md has the details.
