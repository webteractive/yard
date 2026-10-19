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
  (a bind probe, which on failure goes straight to `Failed` with the holder and does not
  back off, since retrying cannot free a foreign port), and runtime (health probes plus
  a 30 s scan of idle registered ports that emits `port.conflict`).
- Probe without `SO_REUSEADDR` first (use `socket2`; std sets it on unix and hides wildcard
  listeners). If that bind is refused, the port still counts as free when a reuse bind
  succeeds and nothing answers on connect: that is TIME_WAIT, which nginx can bind.
  Probe both `127.0.0.1` and `::1`, and UDP as well for dnsmasq.
- Every `PORT_IN_USE` names the holder with `holder::label` (`nginx-arm (pid 4534, Herd)`);
  a refused profile switch also lists them in `data.holders`.
- `portscan::infos` is the one place a registered port gets a status and a holder;
  `ports.list`, `ports.check` and `doctor` all call it, so they always agree.
- Holders come from `/usr/sbin/lsof` plus `proc_pidpath`, not the `libproc` crate (its
  bindgen build needs libclang and adds BSD-3/ISC licenses).
- A unit's own running port counts as free for that unit. A busy port whose unit is
  `Running` has status `yard`: lsof does see nginx (its workers run as the user and
  hold the listening sockets), but only as a plain `nginx` process, so whether it is
  Yard's comes from the unit, not the holder.
- Socket tests take `crate::PORT_TESTS`: macOS hands out ephemeral ports in sequence,
  so parallel tests otherwise land on each other's ports.
- Holder classification (`ports::holder::classify`) is pure and tested: Herd, brew
  services, Docker, Yard, or unknown. When the holder is not visible, say "unknown (not
  visible to this user)", never guess.
- `set_profile` checks every port of the new profile before changing anything, and
  reports all conflicts at once.
