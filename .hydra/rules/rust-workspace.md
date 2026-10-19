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

- `yard-core` is pure: parse, render, decide. No spawning, binding, sockets or clock
  reads. Inject `exists` / `is_free` / `read_link` closures and `Instant`s instead.
  Side effects live in `yard-daemon`. The helper and CLI depend on core, never the reverse.
- The CLI links `yard-daemon` only for the offline commands (`doctor`, `ports check`).
  Keep business logic out of `yard-cli`; it formats and dispatches.
- Every child the daemon or helper spawns gets the `cloexec_inherited` `pre_exec` hook
  (`yard-daemon` `fds.rs`, a copy in the helper's `nginx.rs`). macOS has no `SOCK_CLOEXEC`,
  so without it a unit spawned while another thread opens a socket keeps that socket:
  a port probe's socket held this way made ports look taken.
- No `.unwrap()` / `.expect()` on daemon or helper runtime paths. Errors become a
  `YardError` with a hint. Tests may unwrap.
- Tests use a temp `YARD_ROOT`, OS-assigned free ports and `yard-fake-unit` (via
  `argv[0]` it stands in for nginx, dnsmasq and php-fpm and reads the configs Yard
  rendered). `tests/support` copies it to the nginx path rather than symlinking:
  the helper signals only a pid whose executable is the canonical nginx binary.
- Keep unix socket paths under 104 bytes: put test sockets under a short
  `/tmp/yard-test-*` dir.
- A test that reconciles, probes or runs doctor writes a `ports.toml` first
  (`support::ports_toml(free_port(), …)`): with no file the registry seeds 8080, 8443
  and 53535, which tests never bind.
- Timing tests use scaled-down `SupervisorConfig` / `PolicyConfig` (`support::fast()`),
  never real 5 s intervals. A test that passes on one run and fails on the next is a bug to fix, not
  to retry. Run timing suites several times before calling them done.
- Rendering is pinned with `insta` snapshots. Review each new snapshot against the spec
  before accepting it.
- New crates follow the global rust-dependencies rule: vet them, then run `cargo deny check`.
