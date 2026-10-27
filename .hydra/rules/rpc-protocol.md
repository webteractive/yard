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
  still reads the answer instead of failing on an unknown variant. `#[serde(other)]` only
  catches a unit variant: an enum with data variants that may gain more with data
  (`ServiceState`) uses `#[serde(remote = "Self")]` and a hand-written `Deserialize` that reads
  an unknown `{"<variant>": …}` as `Other` too, and still fails a known variant that does not parse.
- The `node.*` methods (protocol 2.7, M4c): `node.list` (`NodeListParams`, `null` is the
  defaults), `node.install` and `node.uninstall` (jobs), `node.default`, `node.use` and
  `node.unuse`; `SiteInfo.node` is filled per request, not in reconcile's snapshot.
- Protocol 2.8 (M4e): `ServiceInfo.sleeping_since` (RFC 3339) marks a sleeping instance
  (`stopped` with it) and a waking one (`starting` with it), so an older client still sees a true
  state; `sleep_after_secs` and `sleep_after_source` give the effective setting, and
  `StatusInfo.sleeping` lists the sleeping instances. `ServiceState` has an `Other` fallback
  from 2.8, so a later state can be a variant, with or without data. `services.sleep.set { name, after }` takes a
  duration, `off` or `default`.
- `run/wake.sock` (M4e) is not JSON-RPC and has no `yard-proto` types: minimal HTTP/1.1, one
  route (`GET /wake/<site>?wait=<s>`, capped at 30), always `204`, for nginx's `auth_request`
  and the shims. It never adds, changes or removes anything; keep it that narrow.
- Adding a method touches, in one change: the `yard-proto` const and params, the daemon
  handler, the CLI command, the CLI tests' `MockDaemon`, and the method list in spec §6.2.
- Mutations go through `TomlFile` and then `reconcile`. Handlers never write generated
  files directly.
- A method that gained params after clients shipped accepts `null` (what a client with none sends) as
  the defaults: `doctor` reads `DoctorParams { client_env }`, empty for `null`, `{}` or an older CLI.
