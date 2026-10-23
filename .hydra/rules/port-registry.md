---
paths:
  - "crates/yard-core/src/ports/**"
  - "crates/yard-daemon/src/probe.rs"
  - "crates/yard-daemon/src/holder.rs"
  - "crates/yard-daemon/src/portscan.rs"
triggers:
  - a unit needs a TCP or UDP port
  - adding a service or proxy that listens on or targets a port
---

# Port registry

- No unit binds a port the registry has not granted (`ports.toml`, spec §7). A new
  listener means a new registry entry with a mode. A proxy target is an `external`
  entry `proxy:<name>`.
- Checks happen at three points: allocation (`Registry::parse` rejects two non-external
  entries on one port as `CONFIG_INVALID` at the later line), pre-start
  (a bind probe of every port in `UnitSpec.ports`, which on the first held one goes straight
  to `Failed` with that port's holder and does not back off, since retrying cannot free a
  foreign port), and runtime (health probes plus
  a 30 s scan of idle registered ports that emits `port.conflict`).
- Probe without `SO_REUSEADDR` first (use `socket2`; std sets it on unix and hides wildcard
  listeners). If that bind is refused, the port still counts as free when a reuse bind
  succeeds and nothing answers on connect: that is TIME_WAIT, which nginx can bind.
  Probe both `127.0.0.1` and `::1`, and UDP as well for dnsmasq.
- A user cannot bind loopback below 1024 (`PermissionDenied`), so for 80 and 443 the
  probe is a connect: free when nothing answers on either address. In the privileged
  profile the nginx unit has no probe port; the helper's `ports.bind` is the pre-start
  check, and its `PORT_IN_USE` takes the same no-backoff path to `Failed` with the
  holder and a `port.conflict` event. A missing or M1-era helper fails nginx without
  retrying, and `ports.profile privileged` refuses up front unless the helper answers
  and hands over both sockets. A unit entering `Failed` releases its privileged ports.
- Before the first nginx start, `NginxCommand::stop_orphan` stops the master in
  `run/nginx.pid` if it is brew's nginx and launchd is its parent (a yardd that was
  killed left it running); it would otherwise hold the port the new nginx needs.
- Service units (`UnitSpec.pid_file`) are found after a killed yardd the same way, by
  `orphans::stop(pid_file, program, grace, signal).await` (async, so it never blocks a runtime
  thread): the process in `run/<instance>.pid` is stopped when it is the unit's own binary
  (`proc_pidpath`, canonicalized) and launchd is its parent, with the unit's own stop signal (an
  orphaned postgres gets SIGINT, its fast shutdown) and SIGKILL after `grace`. Anything else is left
  running, and a stale file is removed. yardd runs it for every instance in `services.toml` at
  startup, before the first reconcile (`services::stop_orphans`, 30 s grace). A unit with two ports registers both
  (`minio`, `minio-console`), and `reconcile::port_owner` maps each to its unit.
- Every `PORT_IN_USE` names the holder with `holder::label` (`nginx-arm (pid 4534, Herd)`);
  a refused profile switch also lists them in `data.holders`.
- `portscan::infos` is the one place a registered port gets a status and a holder;
  `ports.list`, `ports.check` and `doctor` all call it, so they always agree.
- Holders come from `/usr/sbin/lsof` plus `proc_pidpath`, not the `libproc` crate (its
  bindgen build needs libclang and adds BSD-3/ISC licenses).
- A unit's own running port counts as free for that unit. A busy port whose unit is
  `Starting` or `Running` (`portscan::own_units`, used by `ports.list`, `doctor` and the
  scan) has status `yard`: lsof does see nginx (it runs as the user and holds the
  listening sockets), but only as a plain `nginx` process, so whether it is Yard's
  comes from the unit, not the holder.
- Socket tests take `crate::PORT_TESTS`: macOS hands out ephemeral ports in sequence,
  so parallel tests otherwise land on each other's ports.
- Holder classification (`ports::holder::classify`) is pure and tested: Herd, DBngin
  (`/Users/Shared/DBngin/` or a `/DBngin.app/` path), Postgres.app (`/Postgres.app/`),
  brew services, Docker, Yard, or unknown. Herd is checked first, and the two new kinds
  before brew services, so a conflict reads `mysqld (pid 77034, DBngin)`. When the holder is not visible, say "unknown (not
  visible to this user)", never guess.
- `set_profile` checks every port of the new profile before changing anything, and
  reports all conflicts at once.
- A service port is allocated once, at `services.add`, and recorded `pinned`:
  `Registry::allocate_from(unit, usual, reserved, is_free)` takes the first port at or above
  the engine's usual one that no other entry holds, that is not reserved and that `is_free`;
  `allocate_exact` is `--port` (`PORT_IN_USE` if registered elsewhere or busy). Never use
  `preferred` for a service: `ensure` moves it back to its preferred port, which would change
  `DB_PORT` under a project's `.env`. Reconcile only re-reads pinned entries (an entry the user
  edited keeps its mode); one `ports.toml` lost is pinned again the same way
  (`Reconciler::settle_service_ports`, around `reserved_ports`), and an instance whose port cannot
  be had is a reconcile warning with no unit. `DaemonOptions.usual_ports` (keys `mysql`, `postgres`,
  `redis`, `minio`, `minio-console`) replaces the engine's usual ports and `app_dirs` where DBngin
  and Postgres.app are looked for; tests set the first from `free_port()` and the second to a
  directory with neither, so no test binds 3306, 5432, 6379, 9000 or 9001.
- While DBngin.app is installed, 3306, 5432 and 6379 are reserved, and while Postgres.app is
  installed, 5432 (`services::reserved::reserved_ports`, which looks for the app bundle in
  `/Applications` and `~/Applications` and reads none of their files). A probe only sees live
  listeners, so without the reservation a stopped DBngin's port would be pinned and DBngin
  could no longer start. `--port` overrides a reservation.
