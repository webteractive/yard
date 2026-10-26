# emailulator as a Yard service — design spec

Date: 2026-10-23
Status: approved by the maintainer on 2026-10-26. §4's question, who runs emailulator, is decided: **option B**,
emailulator keeps its own LaunchAgent (2026-10-26, Claude's call; the maintainer asked Claude to decide).
It extends
`2026-10-19-yard-design.md` (the main spec, §2, §7 and §8.2) and
`2026-10-22-yard-project-file-design.md` (the `.yard` spec, §4), and builds on M4b
(services), whose plan is being written in parallel. §14 lists every M4b interface this
spec assumes.

Decisions marked **(maintainer)** are the maintainer's, from the main spec, the `.yard` spec or the M4b
brief of 2026-10-23. Decisions marked **(builder's call)** were made while writing this
and can be reversed on their own.

## 1. Intent

A project that sends mail in development reaches emailulator through Yard without anyone
looking up a port: `yard env` prints the `MAIL_*` lines, `mail.<tld>` opens the web UI,
`yard doctor` says who is serving mail, and a `.yard` that says `emailulator = true` gets
it on `yard init`.

emailulator is our own mail catcher (`emailulator 0.1.3`, in `~/.local/bin`, on
2026-10-23). It differs from every other service in three ways:

- It does not come from brew. Its own installer puts it in `~/.local/bin`, and it updates
  itself (`emailulator self-update`). Yard never installs or updates it **(maintainer)**.
- It keeps its own message store, shared with `emailulator app`, `emailulator mcp` and
  `emailulator inbox|key`, so agents can read what a site sent **(maintainer)**.
- It can supervise itself: `emailulator service install` writes a LaunchAgent, and while
  that agent runs, `emailulator run` refuses to start a second server. On this machine
  the agent is installed and running on 127.0.0.1:1025 (SMTP) and 127.0.0.1:8025 (HTTP).

**Milestone: M4d (maintainer, 2026-10-23).** the maintainer moved emailulator out of M4b. M4d follows M4b
(services) and M4c (Node), and needs M4b's service plumbing.

## 2. Decisions

From the existing specs and the M4b brief **(maintainer)**:

- emailulator is found on the user's machine, never installed or updated by Yard.
- When Yard runs it, it runs `emailulator run --smtp-addr 127.0.0.1:<smtp> --http-addr
  127.0.0.1:<http>`, on registry entries `emailulator-smtp` and `emailulator-http`, and
  keeps emailulator's own store, not `services/emailulator/data/`.
- While emailulator's own LaunchAgent runs, Yard runs no unit. It records the agent's
  two ports as `external` registry entries and points `mail.<tld>` and `yard env` at them.
  `yard doctor` says which of the two is serving mail.
- The web UI is proxied at `mail.<tld>`.
- `yard env` prints `MAIL_MAILER=smtp`, `MAIL_HOST=127.0.0.1`, `MAIL_PORT=<smtp>`, and
  never writes `.env`.
- `.yard` `[services]` `emailulator = true` is met by any emailulator Yard can reach, the
  LaunchAgent included. A missing one is a need whose hint is emailulator's installer.
- Services bind 127.0.0.1 only. A service the user stopped stays stopped. Instances are
  shared across projects. A conflict names the holder.

Made here **(builder's call)**, each argued where it appears:

1. Yard asks emailulator about its agent with `emailulator service status --json`, and
   never reads the plist or hard-codes the label (§5.2).
2. Yard finds the binary by the absolute path the CLI resolved on the user's PATH, then
   `~/.local/bin/emailulator` (§5.1).
3. `mail.<tld>` rewrites `Host` to emailulator's loopback address and passes the browser's
   host in `X-Forwarded-Host` (§10).
4. `mail.<tld>` is not a site: it is not in `sites.toml` or `yard sites`, and cannot be
   tunnelled (§10).
5. `yard env` prints `MAIL_USERNAME=null`, `MAIL_PASSWORD=null`, `MAIL_ENCRYPTION=null`
   and `MAIL_SCHEME=null` too (§9).
6. `.yard` takes only `emailulator = true`. A version string is `PROJECT_FILE_INVALID` (§8).

## 3. Goals, non-goals, anti-goals

### Goals

1. A Laravel project on Yard sends mail to emailulator with the lines `yard env` prints.
2. `http://mail.<tld>:8080/` (or `https://mail.<tld>:8443/`) opens the web UI that is
   serving mail, whoever runs it.
3. `yard doctor` and `yard services` say whether mail is caught, by whom, on which ports.
4. `emailulator = true` in a `.yard` is met on `yard init`, or the setup page names the fix.

### Non-goals

- Per-project inboxes and keys (`emailulator inbox create`, `MAIL_USERNAME=<inbox>`). A
  later step can mint them; printing a key means handling a secret, which `yard env` does
  not do today **(builder's call)**.
- Versions. emailulator has one installed version and updates itself; `--version` is
  refused (§7).
- emailulator's own options (`--max-messages`, `--auth-token`, `--tls-cert` …). Yard
  runs it with addresses only.

### Anti-goals (never)

- Yard never downloads, installs, updates or deletes the emailulator binary.
- Yard never writes, moves or deletes emailulator's LaunchAgent plist, and never runs
  `launchctl` on it. (Option B lets Yard run `emailulator service install` after a yes,
  which is emailulator changing its own agent; §4.)
- Yard never deletes emailulator's message store, `--keep-data` or not.
- Yard never runs a second emailulator server beside the agent.

## 4. Who runs emailulator? Decided: B

**Status: decided, option B (2026-10-26, Claude's call; the maintainer asked Claude to decide).** The
recommendation below stands; the main spec §1, §3.1 and §8.2 were reworded in the same change.
Where a table below has a column for each option, only B's is built; A's is kept as the path
to adding A later. Nothing below this section depended on the answer except §7's command
table.

### Option A — Yard runs it as a yardd unit, and defers to the agent (the main spec today)

yardd supervises `emailulator run` like any other service unit. While emailulator's
LaunchAgent is installed and loaded, Yard runs no unit and records the agent's ports as
`external`. Note that a pure "Yard always runs it" is not available: Yard never touches the
agent, and `emailulator run` refuses to start beside it, so A has to defer anyway.

- For: one place for every service (`yard services`, `yard service stop`, Yard.app's
  Services pane from M6, logs in `logs/`); ports allocated by the registry, with holders
  named on conflict; mail runs exactly while Yard does; teammates need no
  `emailulator service install`; matches the main spec's success criterion "MySQL, Redis
  and emailulator run under Yard".
- Against: two supervisors for one program, with a handover in both directions (the agent
  appearing while Yard's unit runs, and going away); `emailulator app` knows only the agent,
  so with Yard's unit it opens servers of its own on other ports, over the same store;
  more code and more tests (the unit, the handover, the probe on two ports); mail stops
  whenever yardd does, which during the Herd coexistence period also stops mail for Herd
  sites that send to 1025.

### Option B — emailulator keeps its own LaunchAgent; Yard only points at it

Yard never runs an emailulator unit. It reads the agent's state and ports, records them
as `external`, proxies `mail.<tld>` to them, prints `yard env`, and checks them in
`doctor`. When no agent is installed, `yard service add emailulator` (and the `.yard`
confirm flow) runs `emailulator service install` after a yes.

- For: emailulator already has a supervisor built for it, with the one-server rule,
  `emailulator app` opening the agent's UI, and `self-update --restart`; Yard does not fight
  any of it. Its state (the store, the agent) is outside Yard either way, so owning the
  process buys little. It works whether or not yardd runs, which matters while Herd and
  Yard run side by side (AGENTS.md goal 1, 2026-10-23). The smallest M4d: no unit, no
  handover, no two-servers-on-one-store window.
- Against: `yard service start|stop|restart|remove emailulator` are refused with a hint
  naming emailulator's command (§7); Yard.app cannot toggle it without calling emailulator;
  the ports are emailulator's (`emailulator service install -- --smtp-addr …`), so Yard
  only tracks them; the main spec's §1 success criterion and §3.1 diagram would be reworded
  ("reachable through Yard" instead of "run under Yard").

### Recommendation: B

Recommended, for these reasons:

1. **It avoids two supervisors for one process.** emailulator was given its own agent on
   2026-10-20, built around being the one server on the Mac. A, which the main spec
   describes, runs a second supervisor that has to watch the first. The handover cases
   (an agent installed on other ports while Yard's unit runs, a KeepAlive restart, an
   uninstall) are the kind of thing that works in tests and surprises later.
2. **Mail outlives yardd.** During the coexistence period Herd sites send mail too, and
   `yard stop` should not stop catching it.
3. **Yard owns nothing of emailulator's state.** The store is emailulator's under both
   options, so A's main benefit is one more row Yard can start and stop.
4. **The cost is small and reversible.** B is a subset of A: everything B needs (finding
   the binary, reading the agent, external entries, the proxy, `yard env`, doctor, `.yard`)
   is the shared part of the plan. Adding A later is the plan's Task 7 alone.

With B chosen, the same change reworded main spec §1 (success criteria), §3.1 (yardd's
supervised units) and §8.2's emailulator bullets.

## 5. Finding emailulator

### 5.1 The binary

yardd runs under launchd's PATH, not the user's shell PATH, so "on the user's PATH" has to
be resolved by the CLI **(builder's call)**:

- `yard service add emailulator` (and `yard init`/`yard link` when a `.yard` needs it)
  resolve `emailulator` on the CLI's own PATH and send the absolute path. yardd records it
  as `path` on the instance entry (§14, M4b's services config).
- yardd looks, in order: the recorded `path`; `~/.local/bin/emailulator` (where the
  installer puts it). The first one that is an executable regular file wins.
- `self-update` replaces the binary in place, so a recorded path stays valid.
- Not found: `EMAILULATOR_NOT_FOUND`, message `emailulator is not installed`, hint
  `curl -fsSL https://raw.githubusercontent.com/webteractive/emailulator/main/install.sh | sh`,
  `data.searched` listing the paths tried.

### 5.2 Its version and its agent

- `emailulator --version` prints `emailulator 0.1.3 (823e513, 2026-10-20)`. Yard needs
  0.1.3 or later, the first release with `service status --json`; an older one is a doctor
  `warn` with hint `emailulator self-update`, and Yard treats it as having no agent
  **(builder's call)**.
- Yard reads the agent with `emailulator service status --json` **(builder's call)**. It
  exits 0 (answering), 3 (installed, not answering) or 4 (not installed), and prints
  `label`, `installed`, `answering`, `loaded`, `running`, `pid`, `smtp_addr`, `http_addr`,
  `tls`, `log`. Yard reads these fields by name and ignores the rest. Why not read the
  plist: emailulator's next release renames the label from `com.webteractive.emailulator`
  to `co.webteractive.emailulator` and still finds the old one; asking emailulator keeps
  Yard right across that and any later change.
- **The agent owns mail when it is `installed` and `loaded`** **(builder's call)**, not
  only when it is `running`: launchd restarts a loaded KeepAlive agent, and a Yard unit
  started in that gap would hold its ports and leave the agent crash-looping.
- The command runs with the `cloexec_inherited` hook and a 5 s timeout, on every reconcile
  and in the 30 s port scan. A timeout, a non-JSON answer or an unknown exit code keeps
  the last known state and is a doctor `warn` **(builder's call)**.
- `0.0.0.0` in `smtp_addr`/`http_addr` means 127.0.0.1 for connecting, as emailulator
  itself treats it.

## 6. Ports

- **Yard's unit (option A):** `emailulator-smtp` asks for 1025 and `emailulator-http` for
  8025, emailulator's own defaults, through M4b's allocation (the usual port, else the next
  free one above it, recorded once) **(builder's call)**. 8025 rather than an `auto` port
  from the range keeps bookmarks and an MCP client configured for
  `http://127.0.0.1:8025/mcp` working when the agent is replaced by Yard's unit.
- **The agent (both options):** the two entries are `external` with the agent's ports,
  rewritten when the agent's ports change (seen at the next reconcile or scan).
- External entries are never bound by Yard and never allocated to anything else (main
  spec §7). When neither Yard's unit nor an agent exists, the entries are removed, like a
  stale `proxy:` entry.
- `yard service port emailulator <port>` (A) moves the SMTP port; the HTTP port moves with
  `yard ports set emailulator-http <port>` **(builder's call: no second flag on M4b's
  command)**. Under B both are refused with `SERVICE_EXTERNAL` (§7).

## 7. CLI

| Command | Option A (Yard runs it) | Option B (agent only) |
|---|---|---|
| `yard service add emailulator` | Records the instance (binary path, ports) and starts the unit. With an agent loaded: records it, starts nothing, and says the agent serves mail. | No agent: lists `emailulator service install`, asks `[y/N]` (`--yes` skips), runs it as a job. Agent loaded: nothing to do, says so. |
| `--version <v>` | `INVALID_ARGUMENT`, `emailulator has no versions; it updates itself`, hint `emailulator self-update`. | same |
| `--port <p>` | The SMTP port to ask for. | `emailulator service install -- --smtp-addr 127.0.0.1:<p>` |
| `yard service start\|stop\|restart emailulator` | As M4b, on the unit. With an agent loaded: `SERVICE_EXTERNAL`. | `SERVICE_EXTERNAL`. |
| `yard service remove emailulator [--keep-data]` | Stops and removes the unit and its entries. The store is never removed, so `--keep-data` changes nothing and says so. | `SERVICE_EXTERNAL`, hint `emailulator service uninstall`. |
| `yard service logs emailulator` | `logs/emailulator.log`, as M4b. With an agent: the agent's `log` file. | The agent's `log` file (read-only). |
| `yard service port emailulator <p>` | Moves SMTP (§6). | `SERVICE_EXTERNAL`. |
| `yard services` | One row, `managed by` = `yard` or `LaunchAgent <label>`, ports, state, `http://mail.<tld>:8080/`. | same, always `LaunchAgent` |

- `SERVICE_EXTERNAL` (new): `emailulator is run by its LaunchAgent <label>; Yard does not
  start or stop it`, hint `emailulator service uninstall` under A (which hands mail to Yard)
  and `emailulator service status` under B, `data { label, pid }`.
- Every human list ends `For agents and scripts: <command> --json` (cli-output rule).
- After `emailulator self-update`, under A `yard service restart emailulator` runs the new
  binary; `emailulator self-update --restart` also works, since it SIGTERMs every
  `emailulator run` of the user and Yard's supervisor restarts the unit on the new binary
  (one restart against the backoff budget).

## 8. `.yard`

```toml
[services]
emailulator = true
```

- Only `true` is accepted. A string (`emailulator = "0.1"`) is `PROJECT_FILE_INVALID` at
  its line, `emailulator takes true: Yard does not choose its version` **(builder's
  call)**. `false` means the same as leaving it out.
- It is met when Yard can reach an emailulator: Yard's instance exists (A, running or
  stopped by the user) or an agent is installed and loaded (both). A stopped or
  not-answering one is met, and a doctor finding, not a need: a crashed mail catcher must
  not take the site down **(builder's call)**.
- Otherwise it is a need, and the site serves the setup page (the maintainer's rule, `.yard` spec §5):
  - Binary not found: `Need::Manual { what: "emailulator", command: <installer> }`. Yard
    cannot run it. The confirm flow lists it, installs everything else after the yes, and
    exits 1 with `NEEDS_INSTALL`, hint the installer line, while it remains
    **(builder's call)**.
  - Binary found: under A, `Need::Service { service: "emailulator" }` (command
    `yard service add emailulator`); under B, the same need whose job runs
    `emailulator service install`.
- `yard init` writes `# emailulator = true` in its commented `[services]` block, like the
  other services (`.yard` spec §6) **(builder's call)**.

## 9. `yard env`

For a project whose `.yard` lists `emailulator`, or any project when an emailulator is
reachable and `yard env` prints every service (M4b decides which) **(builder's call: follow M4b)**:

```dotenv
MAIL_MAILER=smtp
MAIL_HOST=127.0.0.1
MAIL_PORT=1025
MAIL_USERNAME=null
MAIL_PASSWORD=null
MAIL_ENCRYPTION=null
MAIL_SCHEME=null
```

- `MAIL_PORT` is the actual SMTP port: the agent's or the unit's (main spec §7).
- `MAIL_HOST` is 127.0.0.1 (the agent's host when it listens on another loopback address).
- `MAIL_USERNAME`/`MAIL_PASSWORD=null` override credentials copied from another
  environment: emailulator accepts mail on loopback without them, but a wrong key would be
  refused **(builder's call)**. `MAIL_ENCRYPTION=null` (Laravel 10 and older) and
  `MAIL_SCHEME=null` (Laravel 11 and later) stop a copied `tls`/`smtps` from trying TLS
  against a server that offers none **(builder's call)**. `MAIL_FROM_*` is the app's own
  business and is not printed.
- When emailulator is not answering, the lines are still printed (the ports are known) and
  stderr says `warning: emailulator is not answering; see yard doctor`.

## 10. `mail.<tld>`

- nginx serves `mail.<tld>` (and `*.mail.<tld>`) whenever an emailulator is reachable, on
  the http port, and on https too when `secure_by_default` is on, with a certificate issued
  like a site's **(builder's call)**. It is rendered to `nginx/services/emailulator.conf`
  (generated, header naming `nginx/overrides/mail.conf`), with its certificate at
  `certs/services/emailulator.pem`, so neither collides with a site named `emailulator`.
- It proxies to `http://<emailulator's http addr>` (`https://`, with `proxy_ssl_verify off`,
  for an agent started with `--tls-cert`; it is loopback). **Host is rewritten**:

  ```nginx
  proxy_set_header Host 127.0.0.1:8025;
  proxy_set_header X-Forwarded-Host $http_host;
  proxy_set_header X-Forwarded-Proto $scheme;
  proxy_buffering off;          # the live event stream
  proxy_read_timeout 1h;
  client_max_body_size 25m;     # emailulator's largest message
  ```

  Why: with no auth token on a loopback listener, emailulator answers only to a `Host`
  that means this machine (its DNS-rebinding guard). Checked on 2026-10-23: the running
  agent answers `Host: mail.yard:8080` with 403. nginx's `server_name mail.<tld>` does the
  guard's job here: a rebinding page's own host name never reaches this server block. A
  state-changing request's `Origin` is compared with `Host` and `X-Forwarded-Host`, so the
  latter carries `$http_host` (with the port), not `$host`. Yard's existing proxy block
  (`yard proxy`) passes `Host $host` and would get the 403, so `mail.<tld>` is not a
  `yard proxy`.
- **Not a site** **(builder's call)**: it is not in `sites.toml` or `yard sites`, and
  `yard tunnel mail` finds no site, so captured mail cannot be put on a public URL. A
  linked or proxied site named `mail` wins: Yard does not render `mail.<tld>`, and doctor
  warns (§11).

## 11. Doctor

| Check | Status |
|---|---|
| `emailulator` | `ok`: who serves mail (`Yard's unit` or `LaunchAgent <label> (pid N)`), the SMTP address and the web UI URL. `fail` when a `.yard` or an instance needs it and it is unreachable or not answering, hint `yard service start emailulator` (Yard's, stopped), `emailulator service status` (the agent). `fail` when the binary is gone, with `EMAILULATOR_NOT_FOUND`'s hint. Absent when nothing uses emailulator. |
| `emailulator-version` | `warn` below 0.1.3, hint `emailulator self-update`. |
| `emailulator-status` | `warn` when `service status --json` timed out or did not parse. |
| `mail-host` | `warn` when a site named `mail` shadows `mail.<tld>`, hint `yard unlink mail` or rename it. |

Ports go through `portscan::infos` like every registered port, so `yard ports` and doctor's
port checks show `emailulator-smtp`/`-http` with their holder (`emailulator (pid N)`).

## 12. Failure modes

| Situation | What happens |
|---|---|
| emailulator not installed | `yard service add` and the `.yard` need fail with `EMAILULATOR_NOT_FOUND` and the installer hint. A `.yard` that asks for it serves the setup page naming the installer. Nothing else changes. |
| Binary removed after it was set up | A: the unit's start fails with `EMAILULATOR_NOT_FOUND` and goes to `failed` without backoff (retrying cannot bring a file back); doctor fails. B: the agent's own problem; doctor shows `emailulator service status` failing. |
| LaunchAgent installed and loaded | No Yard unit. Entries `external` on the agent's ports; `mail.<tld>` and `yard env` follow them. `yard service start` etc. are `SERVICE_EXTERNAL`. |
| Agent installed while Yard's unit runs (A) | On another port pair: at the next reconcile or scan (≤ 30 s) Yard stops its unit and switches to `external`; both servers write the same store meanwhile, which emailulator's store tolerates. On the same ports: `emailulator service install` itself refuses, naming the listener. |
| Agent uninstalled (A) | At the next scan Yard allocates its ports again (1025/8025 first) and starts its unit, unless the user stopped it. |
| Agent loaded but crash-looping | Yard starts nothing (§5.2). Doctor fails with hint `emailulator service status`. |
| SMTP or HTTP port held by something else (A) | M4b's allocation takes the next free port above and records it; at start, the probe fails with `PORT_IN_USE` naming the holder and the unit goes to `failed`. |
| `service status` hangs or emailulator is older than 0.1.3 | Times out after 5 s; last known state kept; doctor warns. Never blocks reconcile for other sites. |
| A site named `mail` | The site wins; `mail.<tld>` is not rendered; doctor warns. |

## 13. RPC and errors

- No new methods: emailulator rides M4b's `services.*` and `env.print` **(builder's call)**.
- `ServiceInfo` (M4b) gains `managed_by: ServiceManager` (`yard` | `launch_agent { label,
  pid }`) and `web_url: Option<String>`. `Need` gains `Manual { what, command }`. Both
  additive: protocol minor bump over M4b's.
- New codes: `EMAILULATOR_NOT_FOUND` (`data.searched`) and `SERVICE_EXTERNAL`
  (`data { label, pid }`).

## 14. M4b interfaces this spec assumes

Re-check each against the M4b plan before building; names are placeholders until then.

1. Services config: a user TOML table per instance (`[services.<instance>]`, edited with
   `TomlFile`) with `engine`, `version`, a user-stopped flag, and room for an
   engine-specific `path`.
2. An engine abstraction in `yard-core` (`services::Engine`): name, versions, the registry
   ports it needs with their usual numbers, its unit command, its `yard env` lines, its
   doctor checks. emailulator is one more engine.
3. `Registry::ensure_usual(unit, usual, is_free)`: the usual port, else the next free one
   above, recorded once.
4. Reconcile builds service units from the config and leaves user-stopped ones stopped;
   `UnitSpec` can probe more than one port before a start (emailulator has two).
5. Methods `services.list|add|remove|start|stop|restart|logs|port` with `ServiceInfo`, and
   `env.print { path? }` returning `KEY=value` lines grouped by service.
6. `.yard` `[services]` parsed into `ProjectFile.services` (name → version or default),
   `Need::Service { service, version }`, and `settle` (the CLI's confirm flow) running
   `services.add` jobs after the yes.
7. Doctor checks per instance, `service:<instance>`.
8. If M4b builds a generic "service web host" for `minio.<tld>`, `mail.<tld>` uses it with
   a Host-rewrite option; otherwise M4d builds `nginx/services/` and MinIO can reuse it.

## 15. When built

The same change updates: the main spec (§1 and §3.1 if the maintainer picks B, §3.3 `nginx/services/`
and `certs/services/`, §6.3 the two codes, §7 the emailulator example entries, §8.2,
§10 `e2e-m4d.sh`, §12), the `.yard` spec §4, README, `--help`, AGENTS.md, and the
port-registry and cli-output rules.

## 16. Inconsistencies found in the existing docs

Reported, not changed here:

- Main spec §7's example registers `emailulator-http` as `auto` 33001; M4b's usual-port
  allocation would give 8025, and §6 here argues for 8025.
- Main spec §7's `preferred` mode falls back into the range only with `auto_fallback`; the
  M4b decision (next free port above the usual one) changes that for services.
- Main spec §8.2 names the label `com.webteractive.emailulator`; emailulator's source (not
  yet released) moves to `co.webteractive.emailulator` with `com.` kept as legacy (§5.2).
- Main spec §8.2 says Yard finds emailulator "on the user's PATH"; yardd does not have the
  user's PATH (§5.1).
- The tunnels spec §11 still says `mailpit-http`.
- Yard's proxy block used to send `X-Forwarded-Host $host`, which drops the port, so an app
  that compares `Origin` with it, as emailulator does, refused state-changing requests through
  any `yard proxy` on 8080/8443. Fixed in M4b (2026-10-24): `proxy_location` sends
  `X-Forwarded-Host $http_host` for every proxy block, sites and services alike.
