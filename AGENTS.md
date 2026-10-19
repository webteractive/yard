# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

# Yard — a Rust + Swift replacement for Laravel Herd (macOS)

Yard serves local PHP sites on macOS. A Rust daemon (`yardd`, a user
LaunchAgent) supervises php-fpm pools, dnsmasq and services. A tiny root helper
(`yard-helper`, a LaunchDaemon) runs nginx on 80/443 and manages
`/etc/resolver` and CA trust through a fixed verb set. The `yard` CLI and,
from M4, a SwiftUI `Yard.app` are pure JSON-RPC clients of the daemon.

## Status

- Design approved and committed: `docs/superpowers/specs/2026-10-19-yard-design.md`
  is the source of truth. Read it before changing behaviour, and update it in
  the same change when a decision moves.
- M1 (core serving, CLI only) plan: `docs/superpowers/plans/2026-10-19-yard-m1-core-serving.md`,
  awaiting review. No product code exists yet.
- Milestones M1–M6 (spec §12) each get their own plan in `docs/superpowers/plans/`.
  Write no product code for a milestone before its plan is approved.

## Goals (decided — do not re-litigate)

1. Replace Herd on the maintainer's machine (~30 sites in `~/Herd`), then uninstall Herd.
2. Fix Herd's pain points: PHP versions for legacy projects (7.0–7.3),
   arbitrary extensions, config the user owns and Yard never overwrites, and no
   paywall on services or Node.
3. Ship to the team, later the public: signed, notarized, auto-updating.
4. Learn Rust and Swift. Code quality and clear boundaries matter, not just "it works".

## Key decisions

- **PHP comes from Homebrew** (`shivammathur/php` and `shivammathur/extensions`),
  not our own builds. Homebrew is a hard dependency. Supported PHP: 7.0–8.5.
- **Architecture A** (spec §3): `yardd` owns all state and logic; the root
  helper only takes verbs; nginx and dnsmasq come from brew. Replacing nginx
  with Rust (option C) is deferred and would sit behind the daemon API.
- **Port registry** (spec §7, the maintainer asked for it explicitly): `ports.toml` with
  `pinned`/`preferred`/`auto`/`external` modes, checked at allocation, before
  start (bind probe + holder via lsof) and at runtime. `trial` and `default`
  profiles.
- **Trial profile is the default until `yard takeover`** (M3): TLD `.yard`,
  nginx on 8080/8443, so Herd keeps working alongside.
- **Names:** `yard` / `yardd` / `Yard.app`. launchd labels `dev.more.yard.daemon`
  and `dev.more.yard.helper`.
- **v1 scope:** core serving, multi-PHP + isolate, HTTPS, Node + services.
  Linux, Windows, Intel Macs and PHP < 7.0 are non-goals.

## Environment findings (checked 2026-10-19 on the maintainer's arm64 Mac)

- Toolchain: rustc/cargo 1.92.0 in `~/.cargo/bin`, Swift 6.3.3, Homebrew at `/opt/homebrew`.
- Brew has the `shivammathur/php` and `shivammathur/extensions` taps, plus
  `mysql-client` and `phpredis@7.4`. **No brew PHP is installed. `nginx` is not
  installed. The `dnsmasq` keg is empty** (`Cellar/dnsmasq` exists with nothing
  in it, so it needs `brew reinstall dnsmasq`).
- In the `shivammathur/php` tap, PHP 8.5 is the plain `php` formula (with alias
  `php@8.5`, keg `opt/php`); every other version is `php@<ver>`. The tap also
  carries 5.6 and pre-release 8.6/8.7, which Yard does not support.
- Herd is installed and running. Its nginx (`nginx-arm`) listens on
  127.0.0.1:80 and :443, and `/etc/resolver/test` (Herd's) points at
  127.0.0.1. Herd's Valet config, the input for `yard migrate herd` (M2), is
  `~/Library/Application Support/Herd/config/valet/`. Its `config.json` has
  `tld = "test"` and parked paths `…/valet/Sites`, `~/Herd/` and `~/AI`, and
  `Nginx/` holds per-site configs for secured sites.
- Herd's PHP-FPM sockets are `herd74` to `herd85`. About 30 sites live in `~/Herd`.

## Build & test

Not available until M1 Task 1 lands. Planned:

- `cargo test --workspace` — full suite. Tests use a temp `YARD_ROOT` and ports
  from the OS, and never touch Herd, `/etc/resolver` or ports 53/80/443/8080/8443/53535.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` — CI enforces both.
- `cargo deny check` — after any `Cargo.lock` change (supply-chain gates; `deny.toml` from `~/AI/cooper`).
- `scripts/e2e-m1.sh` — real-machine check in the trial profile. It needs the
  brew prerequisites and `sudo yard helper install`, so ask the maintainer first.

## Layout (planned, spec §3.2)

- `crates/yard-proto` — wire types, error codes, JSON-RPC framing (shared by daemon, helper, CLI, and the Swift app via schema from M4).
- `crates/yard-core` — pure logic: paths, config, port registry, site resolution, rendering, write planning, restart policy. Most unit tests live here.
- `crates/yard-daemon` — every side effect; the `yardd` binary and the `yard-fake-unit` test fixture.
- `crates/yard-helper` — the root helper.
- `crates/yard-cli` — the `yard` binary.
- `app/` — Yard.app (M4).

## Conventions

- **`AGENTS.md` is the only agent instruction file.** Do not add a `CLAUDE.md`
  (or any other harness-specific copy); project guidance goes here or in `.hydra/rules/`.
- **Never commit or push without asking the maintainer.** Commit messages are lean prose:
  a lowercase subject with no type prefix, prose paragraphs, no trailers.
- **Docs move with the code**: the spec, the active plan, README, `--help`
  text, this file and the hydra rules.
- **Herd must keep working** until `yard takeover`. Yard only reads Herd's files.
- Area-specific conventions are lazy-loaded hydra rules in `.hydra/rules/` (index below).

<!-- hydra:rules:start -->
## Rules

**Before you enter plan mode, run a command, or create/edit a file, you MUST
first:** find the index rows whose trigger, glob, or command covers what you are
about to do and read those rule files, then run
`grep -rin '<keyword>' .hydra/rules` to catch what the index alone misses. Do not act
until you are following every matching rule.

Project rules override global rules on conflict.

### Rules index

Rule files are in `.hydra/rules`.

| Applies when | Matches (path glob / command) | Rule |
| --- | --- | --- |
| adding or changing a yard CLI command or its output | `crates/yard-cli/**` | cli-output.md |
| adding a generated config file · writing to a user's toml or override file · Yard overwrote something the user edited | `crates/yard-core/src/render/**` · `crates/yard-core/src/config/**` · `crates/yard-core/src/generated.rs` · `crates/yard-daemon/src/reconcile.rs` | config-ownership.md |
| running yard against the real data root on this machine · binding ports or touching /etc/resolver · switching port profiles or the TLD · reading Herd's configuration | `scripts/e2e*` · `crates/yard-core/src/ports/**` | herd-coexistence.md |
| installing or locating a PHP version, extension, nginx or dnsmasq · launching a brew-installed binary | `crates/yard-core/src/brew.rs` · `crates/yard-core/src/php.rs` · `crates/yard-core/src/render/php.rs` · `brew ` | homebrew.md |
| a unit needs a TCP or UDP port · adding a service or proxy that listens on or targets a port | `crates/yard-core/src/ports/**` · `crates/yard-daemon/src/probe.rs` · `crates/yard-daemon/src/holder.rs` · `crates/yard-daemon/src/portscan.rs` | port-registry.md |
| adding or changing a privileged operation · installing or uninstalling the root helper | `crates/yard-helper/**` · `crates/yard-daemon/src/helper_client.rs` · `sudo ` · `launchctl` | root-helper.md |
| adding or changing a daemon API method, event or error code | `crates/yard-proto/**` · `crates/yard-daemon/src/rpc/**` · `crates/yard-cli/src/client.rs` | rpc-protocol.md |
| deciding which crate new code belongs in · writing or running tests | `Cargo.toml` · `crates/**` · `cargo test` · `cargo add` | rust-workspace.md |
<!-- hydra:rules:end -->
