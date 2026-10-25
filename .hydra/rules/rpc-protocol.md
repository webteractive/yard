---
paths:
  - "crates/yard-proto/**"
  - "crates/yard-daemon/src/rpc/**"
  - "crates/yard-cli/src/client.rs"
triggers:
  - adding or changing a daemon API method, event or error code
---

# Daemon protocol

- JSON-RPC 2.0, one JSON object per line (NDJSON), over `yardd.sock` (mode 0600). The
  helper uses the same framing on its own socket.
- The first request on a connection must be `hello { protocol, client }`. A major
  version mismatch is `PROTOCOL_MISMATCH` and the connection closes.
  `PROTOCOL_VERSION` lives in `yard-proto`. Bump the minor for additive changes and the
  major for anything breaking.
- Every Yard error is JSON-RPC code `-32000` with `data = { code, message, hint, data? }`.
  Codes are stable SCREAMING_SNAKE strings and are never renamed. A new failure mode
  gets a new code, and its `hint` is the exact command that fixes it.
- Types are defined once in `yard-proto` and used unchanged by the CLI's `--json`
  output. From M6 the Swift models are generated from their JSON Schema, so a field
  rename is a breaking change for the app too.
- An enum a newer daemon may extend in a minor version carries a `#[serde(other)]` variant
  the daemon never sends (`SettingSource::Other`, shown as `elsewhere`), so an older client
  still reads the answer instead of failing on an unknown variant.
- Adding a method touches, in one change: the `yard-proto` const and params, the daemon
  handler, the CLI command, the CLI tests' `MockDaemon`, and the method list in spec §6.2.
- Mutations go through `TomlFile` and then `reconcile`. Handlers never write generated
  files directly.
- A method that gained params after clients shipped accepts `null` (what a client with none sends) as
  the defaults: `doctor` reads `DoctorParams { client_env }`, empty for `null`, `{}` or an older CLI.
