# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

# Yard — a Rust replacement for Laravel Herd (macOS)

Yard serves local PHP sites on macOS. A Rust daemon (`yardd`, a user
LaunchAgent) supervises php-fpm pools, dnsmasq and services. A tiny root helper
(`yard-helper`, a LaunchDaemon) manages `/etc/resolver` and CA trust through a
fixed verb set. nginx runs as the user, as a yardd unit; root only opens 80/443
(`ports.bind`) when the user opts in to the privileged profile. The `yard` CLI and,
from M6, `Yard.app` (Rust, GPUI) are pure JSON-RPC clients of the daemon.

## Status

"This machine" below is the maintainer's main Mac (see Machines).

- Design approved and committed: `docs/superpowers/specs/2026-10-19-yard-design.md`
  is the source of truth. Read it before changing behaviour, and update it in
  the same change when a decision moves.
- M1 (core serving, CLI only) is done: `docs/superpowers/plans/2026-10-19-yard-m1-core-serving.md`,
  verified by `scripts/e2e-m1.sh` on this machine on 2026-10-20.
- M1.1 (nginx as the user, the privileged profile) is done:
  `docs/superpowers/plans/2026-10-20-yard-m1.1-nginx-as-user.md`, verified on this
  machine on 2026-10-20 (helper reinstalled, `scripts/e2e-m1.sh` passing, nginx running
  as the maintainer under yardd).
- M2 (multi-PHP, extensions, shims, removing parking) is done:
  `docs/superpowers/plans/2026-10-20-yard-m2-multi-php.md`, verified on this machine on
  2026-10-20 (`scripts/e2e-m1.sh` and `scripts/e2e-m2.sh` passing).
- M3 (HTTPS: a local CA, `yard secure`/`unsecure`, `yard trust`/`untrust`, renewal) is
  done: `docs/superpowers/plans/2026-10-20-yard-m3-https.md`, verified on this machine on
  2026-10-21 (`scripts/e2e-m1.sh`, `e2e-m2.sh` and `e2e-m3.sh` passing). The maintainer ran
  `yard trust` on 2026-10-21, so the login keychain trusts this machine's CA. There is no `yard takeover`
  (dropped 2026-10-20). M4 is split into five plans: M4a, the `.yard` project file
  and `yard init` (`docs/superpowers/plans/2026-10-22-yard-m4a-project-file.md`,
  approved 2026-10-22; built on branch `m4a-project-file` and verified on this machine
  on 2026-10-22 with `scripts/e2e-m1.sh` and `e2e-m4a.sh` passing), then M4b services,
  `yard env`, `yard info` and brew sources (`yard source trust`), designed in
  `docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md` (approved 2026-10-23),
  planned in `docs/superpowers/plans/2026-10-23-yard-m4b-services.md` (approved 2026-10-24).
  M4b is built on branch `m4b-services` and verified on this machine on 2026-10-24 with
  `scripts/e2e-m1.sh` and `e2e-m4b.sh` passing (maintainer approved the run; its first pass found that
  postgres needs `LC_ALL` under launchd, fixed in 4420fb5), and merged into main on 2026-10-25.
  M4a.1, the managed `.yard` kept in Yard's data root (`sites/<name>/.yard`), is designed in
  `docs/superpowers/specs/2026-10-24-yard-managed-project-file-design.md` (approved
  2026-10-25) and planned in `docs/superpowers/plans/2026-10-25-yard-m4a1-managed-project-file.md`;
  it is built, merged into main and verified on this machine on 2026-10-25 with
  `scripts/e2e-m4a1.sh` passing (maintainer approved the run; its first pass found that
  `yard unisolate` dropped the comment above a last key, fixed in 05c77dd). Until the first
  release, work happens on main, with no feature branches (maintainer, 2026-10-25). The data root
  moved from `~/Library/Application Support/Yard` to `~/.yard` on 2026-10-25 (maintainer), so no
  path Yard writes or runs from has a space; this machine was moved by hand (no migration code). M4c Node is
  designed in `docs/superpowers/specs/2026-10-25-yard-m4c-node-design.md` (approved
  2026-10-26) and planned in `docs/superpowers/plans/2026-10-26-yard-m4c-node.md` (approved 2026-10-26); it is built on main and verified on this machine on 2026-10-26 with `scripts/e2e-m4c.sh`
  passing on its first run (maintainer approved the run and `brew install fnm`). The maintainer moved emailulator out of M4b into
  M4d on 2026-10-23: designed in `docs/superpowers/specs/2026-10-23-yard-emailulator-design.md`
  and planned in `docs/superpowers/plans/2026-10-23-yard-m4d-emailulator.md`, both
  approved 2026-10-26; emailulator keeps its own LaunchAgent and Yard only points at it (option B,
  decided 2026-10-26: the maintainer asked Claude to decide). M4d is built on main (2026-11-01, Tasks 1–11
  committed one per task with the maintainer's approval) and verified on this machine on 2026-11-01 with
  `scripts/e2e-m1.sh` and `e2e-m4d.sh` passing on their first run against yardd built from 09c4b81
  (maintainer approved the run); the agent kept its pid throughout. The final review's four Important
  findings were fixed in a3413ce (emailulator's last known state is kept when it cannot be read,
  it is asked outside the reconcile lock, the scan follows a crashed or recovered agent, and a
  recorded `[emailulator]` alone no longer makes doctor speak); both scripts passed again against
  yardd built from a3413ce the same day, so M4d is done. The review's deferred minors, the two
  flaky tests and brew's missing disable date were fixed in 7580881, which also adds the e2e's
  Origin check through `mail.yard`; both scripts passed again against yardd built from 7580881
  the same day (maintainer approved the run).
  M4e, on-demand services (instances sleep when idle and wake on use), is designed in
  `docs/superpowers/specs/2026-10-26-yard-m4e-on-demand-services-design.md` (2026-10-26,
  written in another session and checked against the code; the maintainer answered its §13 questions
  and approved it on 2026-10-26: sleeping is opt-in; on 2026-11-03 the maintainer made it the default, 15 min
  unused, `off` per instance or in `yard.toml` to opt out) and planned in
  `docs/superpowers/plans/2026-10-26-yard-m4e-on-demand-services.md` (approved 2026-10-26). It
  is built on main (2026-10-26, before M4d) and verified on this machine on 2026-10-26 with
  `scripts/e2e-m1.sh` and `e2e-m4e.sh` passing on their first run (maintainer approved the run); a wake
  ask while awake took 0.16 ms at p50. Both passed again on 12babe5, after the final review's
  fixes (the snapshot published outside the reconcile lock, sleep undone by the files, the hook
  only where a service may sleep). The review's deferred minors were fixed on 2026-10-27: sites
  are listed in `run/sleeping` before the stop begins, a failed sleep reconcile undoes the
  sleep, the idle check and the marking share one lock, a connection between two of an
  engine's own sockets is not a client, the wake ask forwards no client headers, and an
  unknown `ServiceState` with data reads as `Other` (2fb79d6). `scripts/e2e-m1.sh` and `e2e-m4e.sh`
  passed again against yardd built from 2fb79d6 on 2026-10-27 (a wake ask took 0.18 ms at p50),
  so M4e is done. On 2026-11-02 the maintainer asked for it to be made ready for production: closed
  connections (`TIME_WAIT`) now count as use, a sleep drains first and rechecks, asks during a
  stop wait for the new process, a held port refuses every ask (503, shim exit 1) until it is
  free, a broken TOML no longer stops a wake, and asks are capped at 64. The e2e now runs real
  Redis, MySQL 8.0 and PostgreSQL 17. `scripts/e2e-m1.sh` and `e2e-m4e.sh` passed on their first
  run against yardd built with those changes the same day (maintainer approved the run): cold starts
  0.46–0.90 s, a page waking all three in 1.51 s, a wake ask 0.10 ms at p50.
- Approved on 2026-10-22: the `.yard` project file
  (`docs/superpowers/specs/2026-10-22-yard-project-file-design.md`; M4a builds `php`,
  `docroot`, `secure` and `extensions`, M4b `[services]`, M4c `node`) and tunnels
  (`docs/superpowers/specs/2026-10-21-yard-tunnels-design.md`, M5), planned in
  `docs/superpowers/plans/2026-11-02-yard-m5-tunnels.md` (approved 2026-11-03). M5 is built
  inline on main (2026-11-03, one commit per task with the maintainer's approval); a quick tunnel is
  verified on this machine on 2026-11-03 with `scripts/e2e-m1.sh` and `e2e-m5.sh` passing
  against yardd built from 0df393e (maintainer approved the run; its first pass found that this Mac's
  resolver cached the NXDOMAIN of a URL asked for before Cloudflare published it, so the script
  now resolves through 1.1.1.1); a named tunnel is the maintainer's manual check on his own zone (`yard
  tunnels domain add <domain>`, `yard tunnel --host <name>.<domain>`, curl it, `yard untunnel`,
  delete the CNAME, `yard doctor` clears once the record is gone).
- M6 (Yard.app in Rust and GPUI, the maintainer 2026-11-06) is built: design
  `docs/superpowers/specs/2026-11-05-yard-m6-app-design.md`, plan
  `docs/superpowers/plans/2026-11-05-yard-m6-app.md` (approved and built 2026-11-06, inline on
  main, one commit per task). Protocol 2.11 (`reconciled`, `php.list { available }`) and
  `yard-client` came with it. Checked here: the app against a scratch yardd follows a stop and a
  restart, runs as a menu bar item with a Dock icon only while a window is open, and onboarding
  judged this Mac set up. Most of its checklist (plan Task 17) is `app/scripts/checklist.sh`,
  which drives the app through macOS accessibility against a scratch yardd; it passed 71/71 on the
  Mac mini on 2026-11-06. Its runs found and fixed: a site row and Settings… each opening another
  window; VoiceOver reading no text (then no log lines), greyed-out controls reading as enabled, and
  controls below the fold doing nothing when pressed; the menu missing sites and services after a
  launch; a committed `.yard` locking only the PHP picker; the site detail with no Node; Copy .env
  dropping a note; a failed service shown twice in the menu and without its holder on the screen;
  the line picker never offered (`data.available`); a `--root` helper hint offering only Copy;
  onboarding hiding its own errors; and two scratch-root hazards: a scratch yardd repointing this
  Mac's resolver, and `yard --root <scratch> start|stop` taking the real LaunchAgent. The final
  whole-branch review (2026-11-07, root and app reviewed apart) found, and this fixed: clicks
  falling through sheets and picker popups, `reconnect_now` dropping a live session (its jobs
  turned lost), action errors no screen showed, onboarding that could not be reopened, the Xdebug
  submenu missing after a launch, the PHP screen asking for a failed `ext.list` every frame, a
  `cargo test` that ran a real yardd on 8080/8443/53535, `yard start` refused for good by an agent
  left for a deleted scratch root, and the features the design names that were missing (the
  Overview's CA, Remove keeping data, Save settings' location and replace, Not now, a `.so` from an
  open panel, Range…, per-formula trust, a failed job's output, named shares, Copy URL, the DNS
  records a stop leaves). `checklist.sh` then passed 72/72, `checklist-installs.sh` (installs,
  tunnels and Terminal against a fake brew) 14/14, and `checklist-real.sh` (plan Step 4) 11/11
  against the real yardd here on 2026-11-07, which now runs that build. System Events (ax.js) gives
  a repeated structure's later items the first item's text and frame (Activity's job cards); the
  AX API itself, which VoiceOver reads, is right, so such checks read `app/scripts/axdump.swift`.
  A1 and A2 were checked with VoiceOver on (maintainer approved) on 2026-11-07, reading its caption panel
  after each VO key; it found and this fixed two bugs: every table read "empty" (AccessKit gives a
  macOS table no rows; tables are now lists of groups) and VO-Space could not open the status
  menu (tray-icon gives the button no action; `platform::status_press`). R3 (the app's privileged
  switch refused, naming Herd) was dropped from M6 (maintainer, 2026-11-07): the Mac mini has no Herd,
  and `e2e-m1.sh` checks yardd's refusal on his main Mac. M6 is done. The review's deferred minors
  were fixed on 2026-11-09 (the client's write deadline and socket limit, a dropped `Session`
  ending its task, `php list --available` against an older yardd, the trust questions queued and a
  settle resumed after one, sheets taking Enter and Escape, a picker closing on a click outside, log
  tails reconnecting, bad port input shown, and the rest in the M6 spec's status); `checklist.sh`
  passed 72/72 and `checklist-installs.sh` 14/14 again. `ax.js menu-click` no longer presses Escape
  after an enabled item, which would cancel the sheet the item opened. The visual design
  (`docs/superpowers/specs/2026-11-09-yard-app-visual-design.md`, the D3 logo in its §6) was built on
  2026-11-10; its Status says what is left; `checklist.sh` passed 72/72 and `checklist-installs.sh` 14/14 on it
on 2026-11-11.
- Milestones M1–M7 (spec §12; reordered 2026-10-21: M4 Node and services, M5 tunnels,
  M6 Yard.app, M7 distribution) each get their own plan in `docs/superpowers/plans/`.
  Write no product code for a milestone before its plan is approved.

## Goals (decided — do not re-litigate)

1. Replace Herd on the maintainer's machine (~30 sites in `~/Herd`), then uninstall Herd. Herd is
   retired only once Yard is ready for production (maintainer, 2026-10-23); until then both run
   side by side and Yard never disturbs Herd.
2. Fix Herd's pain points: PHP versions for legacy projects (7.0–7.3),
   arbitrary extensions, config the user owns and Yard never overwrites, and no
   paywall on services or Node.
3. Ship to the team, later the public: signed, notarized, auto-updating.
4. Learn Rust. Code quality and clear boundaries matter, not just "it works". Yard.app is Rust
   with GPUI too, so the whole project is one language (maintainer, 2026-11-06; until then the goal
   was Rust and Swift, with a SwiftUI app).

## Key decisions

- **PHP comes from Homebrew** (`shivammathur/php` and `shivammathur/extensions`),
  not our own builds. Homebrew is a hard dependency. Supported PHP: 7.0–8.5.
- **Architecture A** (spec §3): `yardd` owns all state and logic; the root
  helper only takes verbs; nginx and dnsmasq come from brew. Replacing nginx
  with Rust (option C) is deferred and would sit behind the daemon API.
- **Port registry** (spec §7, the maintainer asked for it explicitly): `ports.toml` with
  `pinned`/`preferred`/`auto`/`external` modes, checked at allocation, before
  start (bind probe + holder via lsof) and at runtime. `standard` (8080/8443)
  and opt-in `privileged` (80/443) profiles; M1's names `trial` and `default`
  are still read as aliases.
- **8080/8443 is the default, with TLD `.yard`**, so Herd keeps working alongside.
  There is no `yard takeover` (dropped 2026-10-20): Yard never stops or reconfigures
  Herd, and moving to `.test` is the user's own steps once Herd is gone (spec §5.5). 80/443 is opt-in, and even then root never runs
  nginx: the helper hands yardd the two sockets (spec §3.1).
- **Sites are explicit** (decided 2026-10-20): a project is served only after
  `yard link`; nothing is written into the project, except `.yard` by
  `yard init --local` (2026-10-22, `--local` since 2026-10-24), which never overwrites one
  without `--force`. Plain `yard init` writes a managed `.yard` in Yard's data root
  (`sites/<name>/.yard`, carrying `project = "<path>"`), so the repo stays clean; a
  committed `.yard` always wins over it (maintainer, 2026-10-24). A linked project's committed
  `.yard` owns its `php`, `docroot`, `secure` and `extensions`, and `yard isolate`,
  `secure` and `docroot` are refused for it; for a managed one they edit that file. Herd-style parking, which M1
  shipped, was removed in M2. There is no `yard migrate herd` (dropped 2026-10-20):
  each project moves from Herd by `yard link` run in it.
- **Services** (M4b, spec `2026-10-23-yard-m4b-services-design.md`): MySQL, PostgreSQL, Redis
  and MinIO run from brew's kegs as supervised units listening on 127.0.0.1 only, listed in
  `services.toml` and named `<engine>-<line>` (`mysql-8.4`, `redis`). Their ports are
  allocated once at `yard service add`, recorded `pinned` and never moved by Yard, and skip
  the ports DBngin and Postgres.app reserve while installed. Yard never runs `brew services`
  or `brew uninstall` for one, never re-initializes or deletes data except `service remove`,
  and creates no databases, roles or buckets. A MySQL 8 instance makes `mysql_native_password`
  the default (root included) and every MySQL instance uses `utf8mb4_unicode_ci` and
  `sql_mode = NO_ENGINE_SUBSTITUTION`, so PHP 7.0–7.3 projects work (M4b spec §16, the maintainer
  2026-10-28: old projects must install and work). A formula from a third-party tap installs only
  once brew itself trusts it (`brew trust`, read with `brew trust --json=v1`); `--yes`, `init`
  and `link` never grant trust, and Yard sets no `HOMEBREW_NO_*` variable.
- **Never Herd's PHP** (maintainer, 2026-10-20): every PHP command Yard runs or a shim
  `exec`s is an absolute brew keg path, and its PATH comes from `shim::child_path`
  (the keg's `bin` first, every `Herd` directory removed).
- **Never nvm's or Herd's Node** (M4c spec, 2026-10-26): Node comes only from brew's `fnm`,
  run by absolute path with `FNM_DIR=~/.yard/node`. Yard never reads, runs or changes
  `~/.nvm`, Herd's `config/nvm` or fnm's own directory, and the `node`/`npm`/`npx` shims
  never download: a missing version is `NODE_MISSING` with the `yard node install` hint.
- **Names:** `yard` / `yardd` / `Yard.app`. launchd labels `dev.more.yard.daemon`
  and `dev.more.yard.helper`.
- **v1 scope:** core serving, multi-PHP + isolate, HTTPS, Node + services.
  Linux, Windows, Intel Macs and PHP < 7.0 are non-goals. On 2026-11-01 the maintainer tabled PHP 5.6
  and Node older than 16 (his oldest project is PHP 7.3 and Node 16) to make 7.0–8.5 solid
  instead, and left MariaDB as a later engine.

## Machines

Yard is developed on more than one Mac. What Yard needs is the same on each; what is
installed, which ports are held and whether Herd runs are not. Read the machine you are on
before relying on any of that (`scutil --get ComputerName`, `yard doctor`, `brew list
--versions`, `lsof -nP -iTCP -sTCP:LISTEN`), and never carry Herd, DBngin, an installed PHP or
a port's holder over from another machine's notes below. "This machine" in Status, in the
spec's milestone notes and in older rules means the maintainer's main Mac, where M1–M5 were verified.

### Setting up a Mac

1. An Apple Silicon Mac with Homebrew at `/opt/homebrew` and the `shivammathur/php` and
   `shivammathur/extensions` taps (`brew tap shivammathur/php`, `brew tap
   shivammathur/extensions`). Xcode 26 (the macOS 26 SDK and its Metal toolchain) only matters
   from M6, for Yard.app's GPUI build, with Rust 1.98.1 (`rustup toolchain install 1.98.1
   --profile minimal --component clippy,rustfmt`; `app/rust-toolchain.toml` picks it in `app/`).
2. Rust from rustup, never brew's `rust` formula, which ignores `rust-toolchain.toml` (1.92;
   CI's clippy runs on it, and a newer rustc lints differently). Install rustup with the
   official installer (`sh.rustup.rs`, `--no-modify-path` when the shell already sources
   `~/.cargo/env`), then `rustup toolchain install 1.92 --component clippy,rustfmt`, and
   `brew uninstall rust` if it is there. `rustc --version` in the repo must print 1.92.
3. `brew install shivammathur/php/php@8.4` (`yard start` installs nginx and dnsmasq when they
   are missing, after asking), plus `fnm` (Node), `cloudflared`
   (tunnels), and `jq` and `shellcheck` for the e2e scripts. Ask the maintainer before each install
   (homebrew rule). Never start them with `brew services`.
4. `cargo build --release`. The maintainer runs `sudo ./target/release/yard helper install` in a real
   terminal: an agent's shell, `!` included, has no tty for sudo's password.
5. Put `yard` on PATH as a symlink, `ln -sf "$PWD/target/release/yard" ~/.local/bin/yard` (any
   directory on PATH): `yard` follows the link to find `yardd` beside the real binary, so a
   rebuild needs no relink. Then `yard start` and `yard doctor`, which must say every check is ok.
6. Check serving end to end: in a scratch dir with an `index.php`, `yard link <name>`, then
   `curl http://<name>.yard:8080/`; afterwards `yard unlink <name>` and delete
   `~/.yard/nginx/overrides/<name>.conf`.
7. Add a subsection for the machine below with what you found.

Every `scripts/e2e-*.sh` checksums Herd's `/etc/resolver/test` before and after; on a Mac
without Herd the file must stay absent. `e2e-m1.sh` curls a Herd site and expects the
privileged profile to be refused naming Herd only where Herd is installed (`Herd.app` or its
resolver); without it, it skips the Herd site and tries the profile only while something
holds 80 or 443, since with both free the switch would go through.

### Findings that hold on any Mac

- In the `shivammathur/php` tap, PHP 8.5 is the plain `php` formula (with alias
  `php@8.5`, keg `opt/php`); every other version is `php@<ver>`. The tap also
  carries 5.6 and pre-release 8.6/8.7, which Yard does not support.
- Brew's extension formulas keep the `.so` in their own keg (`redis@7.1` installs keg
  `phpredis@7.1`) and `brew uninstall` leaves their ini in `etc/php/<v>/conf.d`.
  `e2e-m2.sh` removes the ones its adds leave, and `yard doctor` warns `brew-ini:<v>/<file>`.
  An old ini naming `redis@<v>`, which brew renamed `phpredis@<v>`, stops redis loading in
  that PHP until brew's current `20-redis.ini.default` replaces it (doctor's own hint).
- The extensions tap's `formula_renames.json` maps `redis@8.4` to `phpredis@8.4`, which
  `brew search` cannot see (it matches real formula names only) but `brew info` resolves, so
  the trust gate looks in the tap's files instead of asking `brew search`.
- Since Homebrew 6.0 brew loads a formula from a non-official tap only when the formula or
  its tap is trusted; brew's own store holds that, `brew trust --json=v1` reads it and
  `brew trust`/`brew untrust` change it (Yard never touches `trust.json`). `yard service add
  minio` is refused with `FORMULA_UNTRUSTED` until `minio/stable` is trusted.
- `brew search --formula "/^<base>(@|$)/"` found (2026-10-24) `mysql`, `mysql@8.0`, `mysql@8.4`,
  `mysql@9.7`, `postgresql@12` to `@18`, `redis`, `redis@6.2`, `redis@8.2` and `minio`.
  `postgresql@12` and `@13` are disabled, so they are not offered as lines; `@14` is deprecated
  with a disable date of 2026-11-12, so it is a line until then, with a job line naming the date.
- Under launchd yardd has no locale, and postgres on macOS then exits at startup
  ("postmaster became multithreaded during startup"), so Yard runs `postgres` and `initdb`
  with `LC_ALL=en_US.UTF-8`.
- A `~/.nvmrc` applies to every linked site with a `package.json` in its root; one whose
  `package.json` sits deeper has no Node. Brew's `python3` (3.12 and later) has no
  `distutils`, so Node 16's node-gyp needs `npm_config_python=/usr/bin/python3` (Apple's 3.9)
  to build a native module.
- A MySQL 8.0 instance lets PHP 7.0 log in only once `default_authentication_plugin` is set
  (M4b spec §16), which Yard does.
- Ports held by DBngin (3306, 6379) or Postgres.app (5432) push new instances to the next free
  port; read them from `yard services`, never assume them.
- Brew's dnsmasq caveat says `/etc/resolver/<domain>` overrides fail with `127.0.0.1` on a port
  other than 53. It does not hold: `.yard` names resolve through dnsmasq on 53535 on both Macs.

### Each dev Mac

What is installed on each of the maintainer's Macs, which ports their other software holds, and
what was verified where are in `AGENTS.local.md` beside this file (git-ignored, so a clone has
none). On a Mac without one, follow "Setting up a Mac" above and write down what you find there.

## Build & test

- Yard.app, in `app/` (its `rust-toolchain.toml` picks Rust 1.98.1): `cargo fmt --check`, `cargo
  clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` and,
  after an `app/Cargo.lock` change, `cargo deny check`. `scripts/bundle.sh` builds
  `app/target/Yard.app` (ad hoc, this Mac only); `open app/target/Yard.app --args --root
  /tmp/yard-dev` runs it against a scratch yardd (`cargo run -p yard-daemon --bin yardd -- --root
  /tmp/yard-dev` at the root). Without `--root` it finds the real LaunchAgent and talks to the real
  yardd. A GPUI build fills `app/target` with several GB: remove it when done.
- `scripts/release.sh X.Y.Z [--no-publish | --adhoc]` — a release (M7 spec §8.1): builds the
  root binaries and `app/scripts/bundle.sh --release` (the app with `yard`, `yardd`, `yard-helper`
  and `yard-shim` in `Contents/Helpers/`), signs inside out with the keychain's Developer ID by
  SHA-1 (`YARD_SIGN_IDENTITY` overrides), notarizes through the `notary` keychain profile, staples,
  and writes `dist/X.Y.Z/` (zip, DMG, `SHA256SUMS`, `latest.json`). `--no-publish` and `--adhoc`
  put the version bump back; `--adhoc` seals ad hoc, skips Apple, and names every file `-adhoc`.
  Submitting to Apple or publishing needs the maintainer's approval. `packaging/yard.rb.tmpl` is
  the cask it renders for `webteractive/homebrew-tap`. Remove `dist/` afterwards.
- `install.sh` — the curl installer (M7 spec §5.2); `scripts/test-install.sh` checks its refusals
  against a fake release over `file://` and installs nothing.
- `app/scripts/checklist.sh [--keep]` — the M6 checklist a script can see: a scratch yardd on
  `/tmp/yard-check` (nginx and dnsmasq pinned to 28080/28443/28053, its instances to 28379-28381
  and 28900, auto ports 29000-29099), three linked sites (one with a committed `.yard` that needs
  things) and a redis instance, Yard.app driven through `app/scripts/ax.js` (menus, tabs, buttons,
  switches, pickers, text fields, rows, prompts, the open panel, other apps' windows, the icon's
  shape), each screen compared with the `yard` CLI and screenshotted, timings read by a 50 ms
  watcher, then a second launch on an empty root for onboarding. With `YARD_CHECK_OLD_YARDD`
  naming a yardd built from 2c1ed9b (before protocol 2.11) it also checks the stale banner (O4);
  that yardd predates the helper guard, so build it with yard-core's `HELPER_SOCKET` pointed at a
  path that does not exist (the script refuses one that can reach the helper: the first O4 run
  repointed this Mac's resolver for a few minutes on 2026-11-06).
  It needs `target/release/{yard,yardd}`, `app/target/Yard.app`, jq, brew's redis keg (and adds
  and removes MinIO when brew has it), Accessibility and Screen Recording for the terminal app,
  and the network for N3. It refuses `~/.yard`, a running Yard.app or a used port; fails if
  `/etc/resolver` or yardd's LaunchAgent changed; puts Yard.app's settings back; and removes what
  it made unless `--keep`. Other apps are driven only through accessibility (Apple Events to
  TextEdit or Finder would ask for Automation permission). A full run takes about 12 minutes; do
  not edit the script while it runs (bash reads it as it goes). `shellcheck` stays clean.
- `app/scripts/checklist-installs.sh [--keep]` — the M6 checklist items that install, share or need
  a password, against a scratch yardd on `/tmp/yard-check-installs` whose brew (`YARD_BREW_PREFIX`,
  `/tmp/yard-check-installs-brew`) is the test suite's fake: `yard-fake-unit` as brew, nginx,
  dnsmasq, php-fpm, fnm and cloudflared, so nothing is installed, no tunnel is public and no
  password is typed. B3 (onboarding's helper step opens Terminal; the `sudo` it starts is stopped
  at its password), Set up Yard… from the menu and the Overview, P2 (install progress, a second
  install waiting and cancelled), P3's add and remove, M5 (the Xdebug submenu follows xdebug), the
  trust question (MinIO from the untrusted `minio/stable`), N2 and T1/T2. Same ports as
  `checklist.sh` minus the instances' (MinIO on 28900), so run them one after the other. It needs
  `target/release/{yard,yardd,yard-fake-unit}`, `app/target/Yard.app` and `swift` (P2 reads the job
  cards through `app/scripts/axdump.swift`, the AX API itself); about 3 minutes.
- `app/scripts/checklist-real.sh` — plan Task 17 Step 4, only with the maintainer's approval: the checklist
  items that change nothing, against the real yardd on `~/.yard` (M3, O1, A1, R1, D1, S4 on a
  scratch site `chk-real` it links, secures and unlinks, R3 where Herd is installed, and M1 last,
  which runs `yard stop` and `yard start`, so every site is down for a few seconds and the
  LaunchAgent's yardd comes back as the build in `target/release`). It backs up and restores
  `yard.toml`, `sites.toml`, `ports.toml` and `services.toml`, and refuses a running Yard.app.
- `cargo test --workspace` — full suite. Tests use a temp `YARD_ROOT` and free ports in
  20000-29999 (below macOS's ephemeral range), and never touch Herd, `/etc/resolver` or ports 53/80/443/8080/8443/53535.
  If test daemons time out at startup, `target/debug/deps` has grown too big for FSEvents:
  run `cargo clean --profile dev` (never plain `cargo clean`: the LaunchAgent runs `target/release/yardd`).
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` — CI enforces both.
- `cargo deny check` — after any `Cargo.lock` change (supply-chain gates; `deny.toml` follows the reference in `~/AI/skillset`).
- `cargo run -p yard-daemon --bin yardd -- --root /tmp/yard-dev` — a daemon on a scratch data
  root (logs to stderr). `yardd --version` prints the build; the socket is `<root>/yardd.sock`.
  Pin its nginx and dnsmasq away from the real ones in its `ports.toml`, as `checklist.sh` does. It
  leaves `/etc/resolver` and 80/443 alone (the helper serves `~/.yard`; `doctor` warns so), so its
  sites answer at `http://<site>.yard:<its port>/` through the real resolver only because every
  `.yard` name resolves to 127.0.0.1. A yardd built before 2026-11-06 lacks that guard and
  repoints the real resolver at its own dnsmasq. Run it directly: `yard --root <scratch>
  start|stop` refuses while the LaunchAgent serves another root that still exists, since it has
  one label (before 2026-11-06 they rewrote the agent for the scratch root and stopped the real
  yardd; an agent such a yard left for a root since deleted is taken over by the next `yard start`). Give a
  scratch service instance a `--port` of its own: a sleeping real instance's port is free, and
  a scratch one that takes it fails the real one when it wakes.
- `cargo run -p yard-cli -- --root /tmp/yard-dev sites` — the CLI against that daemon.
  `doctor` and `ports check` also work with no daemon running.
- `scripts/e2e-m1.sh` — real-machine check in the standard profile. It needs the
  brew prerequisites and `sudo yard helper install`, so ask the maintainer first.
- `scripts/e2e-m2.sh` — real-machine M2 check after `e2e-m1.sh`: brew-installs php@7.1
  and two extension formulas, and pecl-builds apcu into brew's PHP 8.4, so ask the maintainer first. Its
  cleanup removes the inis in brew's `conf.d` that its adds left for a removed extension, and it
  fails if one is left. Run PHP only as
  `"$HOME/.yard/bin/php"`; a bare `php` can be Herd's.
- `scripts/e2e-m3.sh` — real-machine M3 check after `e2e-m1.sh`: secures a scratch site and
  curls it over HTTPS with `--cacert`, so it needs no keychain trust; ask the maintainer first. Tests
  never run the real `security` or touch a keychain: `DaemonOptions.security`, the
  fixture's fake `security` and the CLI's `YARD_SECURITY` stand in for it.
- `scripts/e2e-m4a.sh` — real-machine M4a check after `e2e-m1.sh`: a scratch project's
  `.yard` picks PHP 7.1, then 8.4 after an edit, then the first supported PHP that is not
  installed serves the setup page, and `yard isolate` is refused. It needs php@7.1 (from
  `e2e-m2.sh`) and one supported PHP left uninstalled, and installs nothing; ask the maintainer first.
- `scripts/e2e-m4a1.sh` — real-machine M4a.1 check after `e2e-m1.sh`: `yard init` in a
  scratch git repo leaves `git status` clean and serves from the managed `.yard`,
  `yard isolate 7.1` edits that file and keeps its comment, `yard unisolate` keeps the
  header and the comment above `php`, a committed `.yard` added
  afterwards wins within 5 s and `yard doctor` warns `managed-file-ignored`, and
  `yard init --local` moves the file into the project. It needs php@7.1 (from `e2e-m2.sh`)
  and installs nothing; ask the maintainer first. `shellcheck scripts/e2e-m4a1.sh` stays clean.
- `scripts/e2e-m4b.sh` — real-machine M4b check after `e2e-m1.sh`: adds `mysql-8.4`,
  `postgres-17`, `redis` and `minio`, asks each one a question with its own client by
  absolute keg path, checks `yard env` against `yard services`, checks that brew's installed
  `minio` is the formula `services.toml` records, links a scratch project whose `.yard`
  lists MySQL 8.4 and checks `yard info` shows it met, then removes what it added. It needs
  yardd running, jq and `minio/stable` trusted; it brew-installs `mysql@8.4`, `postgresql@17`,
  `redis` and `minio/stable/minio` when missing, so ask the maintainer first. It stops before changing
  anything if one of the four instances already exists, backs up and restores `services.toml`,
  `ports.toml` and `sites.toml`, and fails if the listeners on 3306, 5432 and 6379 or Herd's
  resolver changed. `shellcheck scripts/e2e-m4b.sh` stays clean.
- `scripts/e2e-m4c.sh` — real-machine M4c check after `e2e-m1.sh`: in a scratch project under
  `$TMPDIR` whose `.nvmrc` pins 16.14.0, `yard link --yes` installs it and `node -v` and an
  `npm run` script both report it; `npm install --build-from-source bufferutil` builds;
  `yard node install 22` leaves `jod = 22` in `lts.toml`; after `yard init`, `yard node use 22`
  beats the `.nvmrc`; a pin that is not installed fails with
  the install hint; and `yard node uninstall 16.14.0` is refused while the site pins it. It
  needs yardd running and `fnm` from brew, and downloads Node, so ask the maintainer first. It backs up
  and restores `yard.toml` and `sites.toml`, uninstalls only the versions it installed, and
  fails if a listing of `~/.nvm/versions/node` or Herd's `config/nvm` changed.
  `shellcheck scripts/e2e-m4c.sh` stays clean.
- `scripts/e2e-m4d.sh` — real-machine M4d check after `e2e-m1.sh`, against the emailulator
  LaunchAgent already on the machine (`com.webteractive.emailulator`); it never installs,
  uninstalls, starts or stops it, and stops before changing anything when no agent answers.
  `yard doctor --json` names the agent, `yard ports --json` has `emailulator-smtp` `external`
  on its SMTP port, `mail.yard:8080` answers 200 while the agent's own port answers 403 to
  `Host: mail.yard:8080`, a `DELETE /api/v1/session` (a sign-out that changes nothing) with
  `Origin: http://mail.yard:8080` answers 204 through `mail.yard` and 403 sent to the agent
  directly, and a scratch project under `$TMPDIR` whose `.yard` says
  `emailulator = true` serves (no setup page) and prints the agent's `MAIL_PORT` in `yard env`.
  It needs yardd running and jq; it backs up and restores `sites.toml` and removes its
  scratch override, and fails if Herd's resolver changed; ask the maintainer first.
  `shellcheck scripts/e2e-m4d.sh` stays clean.
- `scripts/e2e-m4e.sh` — real-machine M4e check after `e2e-m1.sh`, with the real engines: adds
  scratch `redis`, `mysql-8.0` and `postgres-17` instances with `sleep_after = "60s"`, links a
  scratch project under `$TMPDIR` whose `.yard` lists them, waits for them to sleep (nothing
  listens on their ports), times each engine's cold start through `yard service start`, then
  checks a page wakes all three within nginx's 10 s wait (PHP gets `PONG` from Redis and runs
  `SELECT 1` on MySQL and PostgreSQL through PDO), that `php` through the shim wakes them and
  says so, and that a held client connection keeps each awake (PostgreSQL's held by a forked
  backend). It prints the p50 of a wake ask while awake. It needs yardd running and brew's
  `redis`, `mysql@8.0` and `postgresql@17` kegs, and installs nothing; ask the maintainer first. It never
  touches another instance (the maintainer's `mysql-8.4` included), stops before changing anything if one
  of the three is already in `services.toml`, backs up and restores `yard.toml`,
  `services.toml`, `ports.toml` and `sites.toml`, and fails if the listeners on 3306, 5432 and
  6379 or Herd's resolver changed. It runs on macOS's bash 3.2. `shellcheck scripts/e2e-m4e.sh`
  stays clean.
- `scripts/e2e-m5.sh` — real-machine M5 check after `e2e-m1.sh`, with the network and no
  Cloudflare account: links a scratch PHP project under `$TMPDIR`, runs `yard tunnel --quick
  --json` in it, resolves the `trycloudflare.com` name through 1.1.1.1 with `dig` (this Mac's resolver,
  asked before the name is published, caches the NXDOMAIN past the run) and curls it pinned to that
  address with `--resolve` (each wait up to 60 s), and checks PHP sees
  `HTTPS=on`, the public host and a `REMOTE_ADDR` that is not loopback, checks the tunnel's
  origin port serves only that site whatever Host it gets, then `yard untunnel` leaves no
  `tunnel-<site>` unit and no port entry. It needs yardd running, jq and brew's `cloudflared`
  (installs nothing), backs up and restores `sites.toml` and `ports.toml`, and fails if Herd's
  resolver changed or `~/.cloudflared` was created; ask the maintainer first.
  `shellcheck scripts/e2e-m5.sh` stays clean.

## Verifying a change on a real machine

The helper is bound to the real data root (`~/.yard`), and a yardd on any other root leaves the
resolver and 80/443 alone, so a change to serving, the helper or reconcile is only observable
there, through `yard`. On a
Mac that is not set up yet, follow Machines → Setting up a Mac first.

- `cargo build --release`, then `./target/release/yard stop && ./target/release/yard start`;
  `yard --version` must show the new commit for yardd. A changed helper needs the maintainer to run
  `sudo ./target/release/yard helper install` in a real terminal first.
- Back up what you will touch (`yard.toml`, `sites.toml`, `ports.toml`, `services.toml`, an
  `nginx/overrides/<site>.conf`) into the scratchpad, and restore it with `cp -p` and a
  `cmp` at the end. Restore permissions with a `trap`.
- Drive it with `yard status`, `yard doctor`, `yard ports [check]`, `yard sites`, and
  `curl http://<site>.yard:8080/` for a site you linked. yardd applies a TOML or override
  edit within about 2 s, and the docroot scan every 30 s.
- For a scratch site, `yard link <name>` run inside a scratch dir and `yard unlink <name>`
  afterwards, then delete the `nginx/overrides/<name>.conf` Yard created for it.
- Never create directories in `~/Herd` (Herd parks it where it runs), and never touch Herd's
  files.
- Never touch DBngin or Postgres.app, wherever they are installed, and use a service's own client by absolute keg path
  (`$(brew --prefix)/opt/mysql@8.4/bin/mysql`): a bare `mysql` or `psql` can be theirs.
  Remove only the service instances you added (`yard service remove <i> --yes`).

## Layout (planned, spec §3.2)

- `crates/yard-proto` — wire types, error codes, JSON-RPC framing (shared by daemon, helper, CLI, and from M6 Yard.app, which uses the types directly).
- `crates/yard-core` — pure logic: paths, config, port registry, site resolution, rendering, write planning, restart policy. Most unit tests live here.
- `crates/yard-daemon` — every side effect; the `yardd` binary and the `yard-fake-unit` test fixture.
- `crates/yard-helper` — the root helper.
- `crates/yard-client` — the connection to yardd that the CLI and Yard.app share (M6).
- `crates/yard-cli` — the `yard` binary.
- `crates/yard-shim` — the `php`/`composer`/`pecl`/`phpize`/`php-config` and `node`/`npm`/`npx` shims, one binary
  dispatching on `argv[0]`; it reads the TOMLs itself and runs without a daemon. `php` and
  `composer` ask yardd's `run/wake.sock` to wake a site's sleeping services first, when
  `run/sleeping` lists the site (M4e).
- `app/` — Yard.app (M6, Rust and GPUI): a second Cargo workspace with its own toolchain (1.98.1),
  lockfile and `deny.toml`. `yard-app-model` holds the decisions (no GPUI, tokio, files or clock),
  `yard-app` the GPUI entities, platform glue, widgets and screens.

## Conventions

- **`AGENTS.md` is the only agent instruction file.** Do not add a `CLAUDE.md`
  (or any other harness-specific copy); project guidance goes here or in `.hydra/rules/`.
- **Never commit or push without asking the maintainer.** The remote is `origin`,
  `git@github.com:webteractive/yard.git` (private, added 2026-11-03; `gh` acts as
  `maintainer` for the webteractive org). Commit messages are lean prose:
  a lowercase subject with no type prefix, prose paragraphs, no trailers.
- **Docs move with the code**: the spec, the active plan, README, `--help`
  text, this file and the hydra rules.
- **Herd must keep working.** Yard only reads Herd's files.
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
| changing a Yard.app screen, widget or menu item · adding an action Yard.app runs against yardd, the yard CLI or Terminal · bumping GPUI or adding a dependency to the app workspace | `app/**` | app.md |
| adding or changing a yard CLI command or its output | `crates/yard-cli/**` | cli-output.md |
| adding a generated config file · writing to a user's toml or override file · Yard overwrote something the user edited · reading or writing a project's .yard · reading or writing a managed .yard in Yard's data root | `crates/yard-core/src/render/**` · `crates/yard-core/src/config/**` · `crates/yard-core/src/generated.rs` · `crates/yard-daemon/src/reconcile.rs` · `crates/yard-daemon/src/doctor.rs` · `crates/yard-core/src/project.rs` · `crates/yard-daemon/src/project.rs` · `crates/yard-daemon/src/rpc/project.rs` · `crates/yard-daemon/src/managed.rs` · `crates/yard-daemon/src/node.rs` · `crates/yard-daemon/src/rpc/node.rs` | config-ownership.md |
| running yard against the real data root on a real machine · binding ports or touching /etc/resolver · switching port profiles or the TLD · reading Herd's configuration | `scripts/e2e*` · `crates/yard-core/src/ports/**` | herd-coexistence.md |
| installing or locating a PHP version, extension, nginx or dnsmasq · launching a brew-installed binary · gating an install on brew's trust, or trusting a tap or formula · installing Node or running fnm · running cloudflared | `crates/yard-core/src/brew.rs` · `crates/yard-core/src/php.rs` · `crates/yard-core/src/render/php.rs` · `crates/yard-core/src/sources.rs` · `crates/yard-daemon/src/sources.rs` · `crates/yard-daemon/src/rpc/sources.rs` · `crates/yard-daemon/src/node.rs` · `crates/yard-daemon/src/rpc/node.rs` · `crates/yard-daemon/src/tunnel/**` · `brew ` | homebrew.md |
| a unit needs a TCP or UDP port · adding a service or proxy that listens on or targets a port | `crates/yard-core/src/ports/**` · `crates/yard-daemon/src/probe.rs` · `crates/yard-daemon/src/holder.rs` · `crates/yard-daemon/src/portscan.rs` | port-registry.md |
| adding or changing a privileged operation · installing or uninstalling the root helper | `crates/yard-helper/**` · `crates/yard-daemon/src/helper_client.rs` · `sudo ` · `launchctl` | root-helper.md |
| adding or changing a daemon API method, event or error code | `crates/yard-proto/**` · `crates/yard-daemon/src/rpc/**` · `crates/yard-cli/src/client.rs` | rpc-protocol.md |
| deciding which crate new code belongs in · writing or running tests | `Cargo.toml` · `crates/**` · `cargo test` · `cargo add` | rust-workspace.md |
<!-- hydra:rules:end -->
