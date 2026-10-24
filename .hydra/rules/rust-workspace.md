---
paths:
  - "Cargo.toml"
  - "crates/**"
commands:
  - "cargo test"
  - "cargo add"
triggers:
  - deciding which crate new code belongs in
  - writing or running tests
---

# Rust workspace boundaries and tests

- Crates: `yard-proto` (wire types), `yard-core` (pure logic), `yard-daemon` (side
  effects, `yardd`), `yard-helper` (root), `yard-cli` (`yard`), `yard-shim` (the PHP
  shims; depends on core and proto only, and reads the TOMLs without a daemon).
- `yard-core` is pure: parse, render, decide. No spawning, binding, sockets or clock
  reads. Inject `exists` / `is_free` / `read_link` closures and `Instant`s instead.
  Side effects live in `yard-daemon`. The helper and CLI depend on core, never the reverse.
- The CLI links `yard-daemon` only for the offline commands (`doctor`, `ports check`).
  Keep business logic out of `yard-cli`; it formats and dispatches.
- Every child the daemon spawns gets the `cloexec_inherited` `pre_exec` hook
  (`yard-daemon` `fds.rs`). macOS has no `SOCK_CLOEXEC`, so without it a unit spawned
  while another thread opens a socket keeps that socket: a port probe's socket held
  this way made ports look taken. A unit that inherits sockets (`UnitSpec.inherit`,
  nginx in the privileged profile) clears `FD_CLOEXEC` on exactly those, after the hook.
- No `.unwrap()` / `.expect()` on daemon or helper runtime paths. Errors become a
  `YardError` with a hint. Tests may unwrap.
- Tests use a temp `YARD_ROOT`, OS-assigned free ports and `yard-fake-unit` (via
  `argv[0]` it stands in for nginx, dnsmasq and php-fpm, and for the service engines
  (`mysqld`, `postgres`, `initdb`, `redis-server`, `minio`), and reads the configs Yard
  rendered). Its nginx `-t` fails on `yard-test-broken` in any `.conf` under its prefix or in a single file
  an `include` there names (as real nginx follows includes). Its nginx runs in the foreground; with `NGINX` set it serves on every
  inherited socket and logs real nginx's `using inherited sockets from` line, which
  `support::inherited_starts` counts. A bare connect cannot prove inheritance, since
  yardd's own copy of the listener accepts it into the backlog. Like real nginx it
  serves every inherited socket and reads a request before answering. Like nginx, the fake exits 0 on SIGQUIT: dying of the signal makes
  macOS write a crash report, and ReportCrash holds the process (and its sockets) while
  it does, which stalls stops and flakes tests. Its `signals <port>` mode logs which of
  SIGQUIT and SIGTERM stopped it, and `stubborn <port>` ignores both, so only SIGKILL (a stop
  timeout) or SIGINT ends it.
- The fake `brew` (`<prefix>/bin/brew`) logs each call to `fake-brew.log` and the `HOMEBREW_*`
  keys it saw to `fake-brew-env.log`. `Fixture::new` seeds `fake-trust.json` with the
  `shivammathur/php` and `shivammathur/extensions` taps (what `brew trust --json=v1`
  prints; `brew trust|untrust` edit it); `fake-brew-version` sets `--version` (default
  7.0.10). Service formulae come from `fake-formulae.json` (`brew info --json=v2` objects)
  for `search`, `info --json=v2` (one or several names: it fails when any is unknown, as brew does) and `install`.
- The fake engines (`redis-server` exits 1 with `Can't chdir to` when its conf's `dir` is missing, as
  real redis does): `mysqld --defaults-file=F` listens on the `port` in `F` and exits 1 with
  `unknown variable` when the overrides file `F` includes contains `bad`; `postgres -c port=N`,
  `redis-server F` and `minio server … --address A --console-address C` listen where they are
  told. `mysqld --no-defaults --initialize-insecure --datadir=D` and `initdb -D D` create `D/ibdata1` and
  `D/PG_VERSION`, wait `<prefix>/fake-init-delay` ms and fail while `<prefix>/fake-init-fail`
  exists; a running `mysqld` waits `<prefix>/fake-listen-delay` ms before it listens, as one in
  crash recovery does. `Fixture::add_formula(name)` makes the keg `opt/<name>/bin/<binaries>` as the fake
  brew's `install` does, and `Fixture::add_service(instance, formula, stopped)` appends an entry
  to `services.toml`. The fake brew reads `tap`, `versions.stable`, `deprecated`, `disabled` and
  `disable_date` from a `fake-formulae.json` entry, and `fake-brew-delay` (ms) holds every install, which
  keeps an add job busy while a second one is queued behind it.
- Keep unix socket paths under 104 bytes: put test sockets under a short
  `/tmp/yard-test-*` dir.
- A test that reconciles, probes or runs doctor writes a `ports.toml` first
  (`support::ports_toml(free_port(), …)`): with no file the registry seeds 8080, 8443
  and 53535, which tests never bind.
- Timing tests use scaled-down `SupervisorConfig` / `PolicyConfig` (`support::fast()`),
  never real 5 s intervals. A test that passes on one run and fails on the next is a bug to fix, not
  to retry. Run timing suites several times before calling them done.
- yardd binds its socket before its first reconcile and answers only once its FSEvents
  watch has started. `support::daemon::Daemon::start` therefore waits for an answered
  `hello` (60 s), so a slow start never eats a call's 10 s budget.
- Starting an FSEvents stream makes fseventsd list the directory of the executable, one
  stream at a time across the machine. On macOS `target/debug/deps` keeps every build's
  object files (`split-debuginfo = "unpacked"`), and at about 300,000 files each test
  daemon took 6 s to start, so the last daemons in a test file timed out (2026-10-20).
  `Daemon::start` names that in its timeout. The fix is `cargo clean --profile dev`, never a
  longer timeout: `target/release/yardd` is what the LaunchAgent runs, so leave it alone.
  `split-debuginfo = "packed"` stops the growth but doubles a test relink, so it is not set.
- Rendering is pinned with `insta` snapshots. Review each new snapshot against the spec
  before accepting it.
- New crates follow the global rust-dependencies rule: vet them, then run `cargo deny check`.
