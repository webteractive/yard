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
- No `.unwrap()` / `.expect()` on daemon or helper runtime paths. Errors become a
  `YardError` with a hint. Tests may unwrap.
- Tests use a temp `YARD_ROOT`, OS-assigned free ports and `yard-fake-unit` (via
  `argv[0]` it stands in for nginx, dnsmasq and php-fpm and reads the configs Yard
  rendered). `tests/support` copies it to the nginx path rather than symlinking:
  the helper signals only a pid whose executable is the canonical nginx binary.
- Keep unix socket paths under 104 bytes: put test sockets under a short
  `/tmp/yard-test-*` dir.
- Timing tests use scaled-down `SupervisorConfig` / `PolicyConfig`, never real 5 s
  intervals. A test that passes on one run and fails on the next is a bug to fix, not
  to retry. Run timing suites several times before calling them done.
- Rendering is pinned with `insta` snapshots. Review each new snapshot against the spec
  before accepting it.
- New crates follow the global rust-dependencies rule: vet them, then run `cargo deny check`.
