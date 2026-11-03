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
- Protocol 2.9 (M4d): `ServiceInfo.managed_by` (`{"kind":"yard"}` or `{"kind":"launch_agent",
  "label", "pid"}`, `yard` when absent) says who runs an instance; emailulator's web UI URL rides
  `console`. `Need::Manual { what, command }` is a need Yard cannot install (`installable()` is
  false): the confirm flow lists it and leaves it to the user. `ServiceAddParams.path` is the
  emailulator binary the CLI found on its PATH; `services.*` for the name `emailulator` are
  handled in `rpc/emailulator.rs` (add a job, the rest `SERVICE_EXTERNAL` while its agent is
  installed and `SERVICE_NOT_FOUND` without one, `remove` then forgetting `[emailulator]`, logs
  from the agent's log), and `env.print` adds an `emailulator` block. `Need` and `ServiceManager` are tagged by
  `kind`, and each has an `Other` fallback the daemon never sends: `#[serde(other)]` on an
  internally tagged enum reads an unknown kind with fields too. An `Other` need is not
  installable, and `ServiceManager::Other` shows as `-`.
- Protocol 2.10 (M5, tunnels): `job_finished` carries an optional `result` (a job's answer, sent
  only on success); `tunnels.list` returns every tunnel the last reconcile ran as `TunnelInfo`,
  sorted by site, and `tunnel_changed` (topic `tunnels`) goes out when a tunnel's state, url or
  last error changes. `tunnel::monitor` asks each running tunnel's metrics server `/ready` (and
  a quick one's `/quicktunnel` until it has a hostname) every `DaemonOptions.tunnel_poll` (2 s);
  losing the edge is `connecting`, never a restart. `tunnels.add` (a `tunnel-add` job) checks
  the site, provider, an existing tunnel and cloudflared before it starts, then waits up to
  `DaemonOptions.tunnel_ready_wait` (30 s) for the tunnel to be ready with a URL and sends its
  `TunnelInfo` as the result; a quick tunnel that does not connect is removed again
  (`TUNNEL_NOT_READY`, `data.tail`). `tunnels.remove` (`tunnel-remove`) answers
  `TunnelRemoved`. Quick tunnels live only in yardd's memory, end with it, and end when their
  site is unlinked or unproxied (P8). `tunnels.domains` lists `[tunnels.cloudflared] domains`
  with `logged_in`; `tunnels.domain_add` (a `tunnel-login` job) runs `cloudflared tunnel login`
  only when the domain has no certificate yet and records the domain only once it has one;
  `tunnels.domain_remove` forgets a domain and keeps its certificate. `TunnelProvider`, `TunnelKind` and
  `TunnelState` each have an `Other` fallback the daemon never sends.
- `run/wake.sock` (M4e) is not JSON-RPC and has no `yard-proto` types: minimal HTTP/1.1, one
  route (`GET /wake/<site>?wait=<s>`, capped at 30), for nginx's `auth_request` and the shims.
  It answers `204`, except `403` with one line per instance (`redis: port 6380 is held by …`)
  when an instance the site needs failed because another program holds its port: nginx then
  serves a 503 and the shims exit 1 without running PHP (M4e spec §5.1). Every ask is refused
  until the port is free, and the next one then restarts the instance. At most
  `wake::WAKE_SLOTS` (64) asks wait at once; the rest are answered straight away. It never
  adds, changes or removes anything; keep it that narrow.
- Adding a method touches, in one change: the `yard-proto` const and params, the daemon
  handler, the CLI command, the CLI tests' `MockDaemon`, and the method list in spec §6.2.
- Mutations go through `TomlFile` and then `reconcile`. Handlers never write generated
  files directly.
- A method that gained params after clients shipped accepts `null` (what a client with none sends) as
  the defaults: `doctor` reads `DoctorParams { client_env }`, empty for `null`, `{}` or an older CLI.
