# Yard M6: Yard.app — design

Date: 2026-11-05
Status: draft, not approved. Written by Claude for the maintainer's review; nothing here is decided until
the maintainer approves it. It extends `2026-10-19-yard-design.md` (the main spec: §3.1, §6.2, §9, §10, §11,
§12) and is planned in `docs/superpowers/plans/2026-11-05-yard-m6-app.md`.

Decisions marked **(builder's call)** were made while writing this, not by the maintainer. Each can be
reversed without changing anything else here unless it says otherwise. The questions only the maintainer can
answer are in §18, and the places where the main spec and the code disagree with building M6 as the
spec describes it are in §19.

## 1. Intent

Yard.app is a SwiftUI client of `yardd`, beside the `yard` CLI: a menu bar item for the
day-to-day (is everything up, open a site, start a service, flip Xdebug), a main window for
everything the daemon API offers, and onboarding for a Mac where Yard is not set up yet.

It holds no business logic (main spec §3.1): every change it makes is a JSON-RPC call to yardd or
a run of the `yard` CLI, so everything it can do, the CLI can do. What it adds is presentation:
live state, progress for long jobs, errors with a button that fixes them, and confirmations.

Why now: M1–M5 are built and verified, so the API the app speaks is complete for v1 (main spec
§12). Goal 4 (learning Swift) makes the app's boundaries part of the deliverable: a pure Swift
package with the wire types, the client and the models, tested with `swift test`, and a thin
SwiftUI layer on top.

## 2. Goals, non-goals, anti-goals

### Goals

1. A menu bar item that shows Yard's health at a glance and offers the common actions.
2. A main window covering every daemon method a person would use: sites, services, PHP and
   extensions, Node, tunnels, ports, doctor, sources, CA trust, units, and live logs.
3. Long operations (brew installs, tunnels) shown with streaming output and a Cancel button.
4. Every daemon error shown where the action was taken, with its hint, and a button when the
   hint maps to something the app can do.
5. Onboarding that takes a Mac from "Homebrew only" to "yardd running, helper installed, first
   site linked", reflecting what is already done.
6. Live updates from events, with no timer polling (main spec §9).
7. A Swift codebase with clear modules: wire types and client (`YardKit`), observable models and
   pure logic (`YardModel`), views (the app target). Most logic is tested with `swift test`.

### Non-goals (M6)

- Signing, notarization, a DMG, auto-update (appdater), a brew tap: M7 (main spec §11).
- Registering the helper or yardd with `SMAppService`, launch at login, and carrying `yard`,
  `yardd`, `yard-helper` and `yard-shim` inside the bundle: M7, pending §18 Q1 and Q3.
- System notifications for finished jobs (an in-app record is kept; see §9).
- A history of nginx, dnsmasq and php-fpm logs: the API streams live lines only (§19.5).
- Editing user-owned files inside the app. The app opens them in the user's editor.
- Linux, Windows, Intel Macs (main spec §1).

### Anti-goals (never)

- The app never writes a TOML, a `.yard`, an override file or anything else in Yard's data root
  or a project. Changes go through RPC or the CLI.
- The app never runs `brew`, `launchctl`, `security`, `sudo` or a shell itself. Offline steps run
  the `yard` CLI with an argument array; anything needing a password in a terminal is handed to
  Terminal for the user to run (§10.3).
- The app never executes a hint string through a shell. Hints are parsed into a fixed set of
  typed actions; anything else is offered as text to copy (§10.2).
- The app never touches Herd, its files, or ports 80/443 except through `ports.profile`, which
  yardd refuses while Herd holds them (herd-coexistence rule).

## 3. Constraints

- macOS 14+ (Sonoma), Apple Silicon, SwiftUI, Swift 6 language mode with strict concurrency
  (main spec §9). Toolchain here: Swift 6.3.3 with Xcode at `/Applications/Xcode.app`.
- The protocol is NDJSON JSON-RPC 2.0 over `<root>/yardd.sock`, `hello { protocol, client }`
  first, every Yard error JSON-RPC code `-32000` with `data = YardError` (rpc-protocol rule).
- Types are defined once in `yard-proto`. The Swift models mirror them field for field; a field
  rename is a breaking change for the app (rpc-protocol rule).
- An enum a newer daemon may extend is read with a fallback, never a decode failure (rpc-protocol
  rule). The Swift side applies this to every enum, including the ones Rust has no fallback for
  (§6.3).
- Herd keeps working (herd-coexistence rule). The default profile stays `standard`.
- Never commit without asking the maintainer; lean prose commit messages (AGENTS.md).
- Disk is tight on this Mac: builds clean their output at the end of a task
  (build-artifact-cleanup rule). The LaunchAgent runs `target/release/yardd`; never clean it.

## 4. Layout

### 4.1 Tuist project plus a local Swift package **(builder's call; §18 Q6)**

```
app/
  Package.swift          # SwiftPM: YardKit, YardModel, YardTestSupport, and their tests
  Project.swift          # Tuist: the Yard.app target, depending on the local package
  Tuist.swift
  App/Sources/…          # SwiftUI views and the AppKit glue; no logic worth a unit test
  Sources/YardKit/…      # wire types, framing, socket, client, CLI runner, locating yardd
  Sources/YardModel/…    # @Observable models, jobs, actions, hints, flows, onboarding facts
  Sources/YardTestSupport/…  # FakeDaemon (a real unix-socket server), FakeYardService, fixtures
  Tests/YardKitTests/…
  Tests/YardModelTests/…
  scripts/build.sh       # tuist generate, xcodebuild, print the .app path
```

This is the layout zetty already uses on this Mac (`~/AI/zetty`: `Project.swift` with an app
target over a local `Package.swift` whose pure core is tested with `swift test`). The reasons:

- `swift test --package-path app` runs every model and client test with no Xcode project, in
  the terminal and in CI. That is where almost all the tests live.
- An app needs an `Info.plist` (`LSUIElement`), a bundle, an icon and later entitlements and
  signing. Tuist describes those in Swift, generates `Yard.xcodeproj` (gitignored, as zetty
  does), and M7 can follow zetty's `scripts/package.sh`, which the macOS release-signing rule
  names as the reference.
- A hand-kept `.xcodeproj` is a large generated file that is hard to review; plain SwiftPM cannot
  produce an app bundle without a hand-written bundling script that M7 would then have to grow
  into signing and notarization.

The cost is a tool: Tuist 4.200.5 is installed here through mise. CI needs no Tuist, since it
runs only `swift test` (§12.4).

### 4.2 Modules and their boundaries

| Module | Imports | Holds | Never holds |
|---|---|---|---|
| `YardKit` | Foundation, Darwin, Dispatch | wire types, `JSONValue`, framing, `UnixSocket`, `YardConnection`, `YardClient`, `YardService` protocol, `YardPaths`, `DaemonLocator`, `CLIRunner` | Observation, SwiftUI, AppKit |
| `YardModel` | YardKit, Foundation, Observation | `AppModel` and its resources, `JobsStore`, `LogsStore`, `ActionRunner`, `Prompt`, `HintAction`, presentation helpers, settle and link flows, onboarding facts and steps, the `Opener` and `TerminalHandoff` protocols | SwiftUI, AppKit, file writes, process launches (it calls protocols the app implements) |
| `Yard` (app target) | SwiftUI, AppKit, YardModel, YardKit | scenes, views, `NSWorkspace` openers, the Terminal hand-off, activation policy | decisions: anything a test would want to check goes in `YardModel` |
| `YardTestSupport` | YardKit, YardModel | `FakeDaemon`, `FakeYardService`, `FakeCLI`, `Fixtures` | production code |

The dependency direction is one way: app → model → kit. `swift-tools-version: 6.0`, platform
`.macOS(.v14)`, language mode 6, no third-party packages. Tests use Swift Testing
(`import Testing`).

## 5. Talking to yardd

### 5.1 Socket and framing

- The socket is `<root>/yardd.sock`; `<root>` is resolved as in §5.8.
- `UnixSocket` (YardKit) is a thin POSIX wrapper, the same approach zetty's control socket uses:
  `socket(AF_UNIX, SOCK_STREAM, 0)`, `SO_NOSIGPIPE`, `connect`, then the fd set non-blocking and
  read by a `DispatchSourceRead` on a private serial queue **(builder's call)**. A missing socket
  file or `ECONNREFUSED` is "not running"; any other errno is reported with its text. The path
  must fit `sockaddr_un.sun_path` (104 bytes); a longer one is an error naming the path, never a
  truncation.
- `LineSplitter` (pure) turns byte chunks into lines: it buffers a partial line across reads,
  yields every complete line in a chunk, and drops empty lines. A line over 16 MiB with no
  newline closes the connection as a protocol error (a `doctor` report or a site list is far
  smaller).
- Writes are one `encode + "\n"` per message, written on the socket's queue until done.

### 5.2 Connections **(builder's call)**

yardd answers the requests on one connection in order, awaiting each handler before reading the
next (`rpc/server.rs`), and pushes events on the same connection once it subscribes. So the
client keeps separate connections, each greeted with `hello`:

| Connection | Opened | Used for |
|---|---|---|
| `events` | at connect, kept | `subscribe` to `units`, `sites`, `ports`, `jobs`, `tunnels` and, from protocol 2.11, `state` (§13); then only read |
| `calls` | at connect, kept | every quick call, pipelined: several requests may be in flight, answers matched by id |
| one-shot | per slow call, closed after its answer | `doctor`, `node.list` with `available`, `services.logs`, `ports.check`; these can take seconds and would hold up a click queued behind them |
| `logs` | while a log view is visible | `subscribe` to `logs` only |

Logs get their own connection because `log_line` is the busiest event and only a visible log view
needs it, and because re-subscribing on `events` to add or drop a topic replaces the subscription,
which could lose a job event in between. Whether a method is slow is a property of its entry in
the method catalogue (`RPCMethod.slow`).

### 5.3 `hello` and protocol versions

- The app sends `hello { protocol: YardProtocol.version, client: "Yard.app <version> (<commit>)" }`.
  `YardProtocol.version` is `"2.11"` once §13 lands. A Swift test checks it against
  `crates/yard-proto/fixtures/protocol.json`, which the Rust fixture test writes from
  `PROTOCOL_VERSION`, so the two cannot drift.
- `PROTOCOL_MISMATCH` (another major) closes the connection; the app shows the daemon's message
  and hint (`upgrade yard and yardd together`) and does not retry until the user asks or yardd
  restarts (§5.6).
- A daemon with the same major and an older minor than the app (yardd not restarted after a
  build) is connected but **stale**: the app subscribes only to the topics that minor knows (an
  unknown topic would fail the whole `subscribe`), shows a banner `yardd speaks protocol <p>; this
  app needs <m>` with a **Restart yardd** button (`yard stop`, then `yard start`, §5.7), and keeps
  working with what the older daemon offers. `METHOD_NOT_FOUND` from a stale daemon is rewritten
  the way the CLI's `Client::stale` does: `yardd speaks protocol <p> and has no <method>`, hint
  `yard stop && yard start`.
- A newer minor is fine: everything it adds is optional, and the app ignores what it does not know
  (§6.3).

### 5.4 Calls

- Ids are per connection, from 1. A `Response` with an id nobody waits for is dropped; a line that
  is neither a response nor an event notification is ignored, as the CLI does.
- Timeout: 60 s per call, the CLI's `CALL_TIMEOUT` (a mutation reconciles, which may wait on the
  helper). On timeout the app reports the CLI's text, `yardd did not answer <method> within 60 s`,
  hint `yard doctor checks what is stuck`, and closes that connection, since a late answer would
  otherwise arrive for a caller that gave up. `services.remove` waits for the unit to stop (30 s
  at most) inside the call and fits.
- Every failure a call can have is a `YardError` (§10.1): the daemon's own, or one the client
  makes with the CLI's wording: `DAEMON_NOT_RUNNING` (`yardd is not running`, hint `yard start`),
  `INTERNAL` `lost the connection to yardd: …` and `yardd closed the connection` (hint
  `yard doctor checks why`), and `PROTOCOL_MISMATCH` `yardd answered <method> in an unexpected
  shape: …` (hint `upgrade yard and yardd together`). Public calls are `throws(YardError)`.
- Calls are never retried automatically. A mutation whose connection dropped may or may not have
  applied; after the reconnect every list is fetched again (§7), which shows what happened.

### 5.5 Events

- An event is a notification `{ "jsonrpc": "2.0", "method": "event", "params": <Event> }`, tagged
  by `type`. An unknown `type` decodes as `.unknown(type:)` and is ignored, so a newer daemon's
  events never break the stream.
- Subscribing happens before any call that starts a job (main spec §6.2: "a client subscribes to
  `jobs` before it calls"). The `events` connection is opened and subscribed before `calls` is
  used, and a job-starting action is refused while `events` is down (`INTERNAL`, `cannot follow
  the job's progress without yardd's events`, hint `yard start`).
- Job events and the answer that started the job travel on different connections, so either can
  arrive first. `JobsStore` handles both orders (§9.2).

### 5.6 Reconnecting

- When `events` or `calls` closes, the client closes the other, fails every pending call with the
  lost-connection error, bumps its **epoch**, and reports `.disconnected`. It then tries to
  connect again after 0.25 s, 0.5 s, 1 s, 2 s, then every 5 s, for as long as the app runs. A
  connect attempt to a missing socket costs one syscall; this is reconnecting, not polling state.
- It also tries at once when the app becomes active, when the menu bar item opens, and after the
  app itself runs `yard start` (§5.7).
- After a successful `hello` and `subscribe`, the client reports `.connected(Session)` with the
  new epoch, and the model fetches every resource it holds (§7.2). Events that arrive while those
  fetches run are applied after them; every event's effect is idempotent (§7.3), so order does not
  matter.
- Job ids restart at 1 when yardd restarts (`<kind>-<n>`), so the app keys jobs by
  `(epoch, job_id)`. A job still running when the connection dropped becomes **lost** (§9.1).

### 5.7 When yardd is not running or not installed

`DaemonLocator` (§5.8) reads the LaunchAgent's plist, so the app can tell three cases apart:

| Facts | State shown | Offered |
|---|---|---|
| no `~/Library/LaunchAgents/dev.more.yard.daemon.plist` and no answer | **Yard is not set up** | the onboarding window (§8.4) |
| plist present, socket does not answer | **yardd is not running** | **Start yardd**: runs `yard start` |
| answers | connected (or stale, §5.3) | everything |

The offline commands are the CLI's (main spec §3.1, cli-output rule): `start`, `stop`, `version`,
`doctor`, `ports check`, `install-shims`, `trust`/`untrust` (which run `security` as the user),
and `helper install|uninstall` (sudo). The app runs them through `CLIRunner`: `Process` with the
located `yard`, an argument array, `--json`, and `--root <root>` when the root is not the default.
Exit 0 decodes the yard-proto type; exit 1 decodes `{"error": YardError}`; exit 3 is
`DAEMON_NOT_RUNNING`; anything else is `INTERNAL` with stderr's last line. The app's PATH is
launchd's (`/usr/bin:/bin:/usr/sbin:/sbin`), which is what `yard start` (`launchctl`) and
`yard trust` (`/usr/bin/security`) need.

While yardd is down the menu bar shows the offline glyph, the window shows its last fetched data
greyed with the banner above, and every control that calls yardd is disabled.

### 5.8 Finding `yard`, `yardd` and the data root **(builder's call; §18 Q3)**

In M6 the app carries no binaries. It follows the yardd the LaunchAgent runs:

1. **Data root:** the `-YardDataRoot <path>` launch argument (a `UserDefaults` argument domain,
   for development against a scratch daemon), else the LaunchAgent plist's
   `EnvironmentVariables.YARD_ROOT` (written by `yard start --root …`), else `~/.yard`.
2. **`yard`:** the `YardCLIPath` setting when set, else the `yard` beside the plist's
   `ProgramArguments[0]` (`yard start` writes the `yardd` beside the `yard` that ran it, so on
   this Mac both are `target/release/`), else `Contents/Helpers/yard` inside the app when present
   (the M7 layout, main spec §9), else none: CLI actions are disabled with `yard was not found`,
   hint `set its path in Settings`.

The socket, override files and log files are paths under the root that the main spec §3.3 fixes
(`yardd.sock`, `nginx/overrides/<site>.conf`, `php/<v>/conf.d/99-overrides.ini`,
`services/<i>/overrides.conf`, `logs/<unit>.log`). `YardPaths` builds them; the app only opens
them.

## 6. Wire models

### 6.1 Hand-written Swift models, checked against Rust-written fixtures **(builder's call; §18 Q2)**

The main spec (§6.2, §10) and the rpc-protocol rule say the Swift models are generated from a JSON
Schema that `cargo xtask schema` emits. Neither exists today (no `schemars`, no `xtask`), and the
wire shapes that matter most are the ones a schema-to-Swift generator handles worst:

- `ServiceState` is externally tagged with a hand-written `Deserialize` that reads an unknown
  `{"<state>": …}` as `Other`;
- `SiteInfo` flattens an internally tagged `SiteKind` (`kind`), and `UnitInfo` flattens
  `UnitState` (`state`);
- `Need` and `ServiceManager` are internally tagged with `#[serde(other)]` fallbacks;
- `YardError.data` and `JobFinished.result` are free JSON.

A generator would drop the fallbacks, which is exactly the forward-compatibility the rule asks
for, and emit names a person learning Swift would not write. So the models are written by hand,
and drift is caught by data instead of by generation:

- A test in `yard-proto` (`src/fixtures.rs`, `#[cfg(test)]`) serializes a sample of every type the
  app reads, every variant of every tagged enum, and every params type the app sends, into
  `crates/yard-proto/fixtures/<name>.json`, plus `protocol.json`. Without
  `YARD_UPDATE_FIXTURES=1` it compares and fails on a difference, naming the file; with it, it
  rewrites them. No new crate is needed (serde_json is already a dependency).
- Swift tests decode every fixture, re-encode it, and compare the two as JSON values, so a field
  the Swift model lacks fails the test, not just a field it gets wrong. Params fixtures check that
  Swift encodes what Rust expects.
- Forward-compatibility cases are written by hand in the Swift tests (an unknown event type, an
  unknown error code, `{"hibernating": {}}` as a service state, an unknown `Need` kind with
  fields), since Rust never sends them.

So a change to a yard-proto type fails `cargo test` until the fixtures are regenerated, and the
regenerated fixtures fail `swift test` until the Swift models follow. Both run in CI (§12.4).

### 6.2 Decoding rules

- Every model is a `struct` or `enum` that is `Codable`, `Sendable` and `Equatable`, with explicit
  `CodingKeys` in snake_case. **No `keyDecodingStrategy`** **(builder's call)**:
  `.convertFromSnakeCase` also rewrites dictionary keys, which would mangle
  `DoctorParams.client_env` (`XDG_CONFIG_HOME`) and `SiteDetail.sources`.
- Rust `Option<T>` is a Swift optional, decoded with `decodeIfPresent` (an absent key and `null`
  mean the same); params encode `nil` by leaving the key out, which every params type accepts
  (`#[serde(default)]`).
- Integers are `Int`, except where the wire range is the point (`port: UInt16`, `pid: UInt32`).
  Timestamps (`sleeping_since`, `since`) stay `String` on the wire type; presentation parses them
  as RFC 3339.
- `SiteInfo`: decode `kind` from the same container and then `path`, `docroot`, `php` (for
  `php`) or `target` (for `proxy`), into `SiteKind`; encode back flat. `UnitInfo` does the same
  with `state` and `retry_in_ms`/`last_lines`.
- `ServiceState`: a string (`"running"`) or a one-key object (`{"failed": {"last_lines": […],
  "reason": {…}}}`); anything else known-but-malformed is a decode error, anything unknown is
  `.other(String)`.
- `Profile` reads `trial` and `default` as `standard` and `privileged`, and writes only the new
  names.
- `ServiceInfo.managed_by` absent means `.yard`; `sleep_after_source` absent means `.default`.

### 6.3 Unknown values

Every wire enum has an unknown case that keeps the raw value: `.other(String)` for string enums,
`.other(kind:)` for tagged ones, `.unknown(type:)` for events. This holds for the enums Rust has
no fallback for too (`UnitState`, `PortMode`, `PortStatus`, `HolderKind`, `CheckStatus`,
`SiteSource`, `ProjectFileKind`, `IgnoredBecause`, `Topic`). `ErrorCode` is a `RawRepresentable`
struct over `String` with a static constant per known code, so a newer daemon's code is shown with
its message and hint like any other.

### 6.4 Methods

`RPCMethod<Params, Result>` carries a method name and whether it is slow (§5.2). A `Methods`
namespace lists one constant per daemon method with its exact params and result types, e.g.
`Methods.sitesList: RPCMethod<NoParams, [SiteInfo]>`, `Methods.phpInstall: RPCMethod<PhpParams,
JobStarted>`. `NoParams` encodes as `null`, which every parameterless method accepts. The
catalogue is the Swift twin of `yard-proto/src/methods.rs`, and a test checks its names against
a `methods.json` fixture the Rust test writes from the consts.

## 7. State in the app

### 7.1 `AppModel`

`AppModel` is a `@MainActor @Observable` class that owns the client's update stream and one
`Resource<Value>` per list: `status`, `sites`, `services`, `php`, `node`, `tunnels`,
`tunnelDomains`, `ports`, `sources`, `ca`, plus `extensions[phpVersion]` and `siteDetail[site]`
loaded on demand, and `doctor`, fetched only when asked (it is slow and runs brew). A `Resource`
holds the last value, the last error, whether a fetch is in flight, and when it last succeeded.
Views read resources; they never call the client.

### 7.2 Refresh policy: events invalidate, lists come from their method **(builder's call)**

Most events say that something changed rather than what it is now (`site_added` lacks the
per-request `node`; a sleeping service is a `unit_state_changed` with no `sleeping_since`), so an
event marks resources stale and the model fetches them again, coalesced over 150 ms, with at most
one fetch in flight per resource and one more queued if it was invalidated meanwhile.

| Trigger | Effect |
|---|---|
| connected (new epoch) | fetch every resource that has been loaded once, and `status` |
| `reconciled` (§13) | `status`, `sites`, `services`, `php`, `node`, `tunnels`, `ports`, loaded `extensions`, loaded `siteDetail` |
| `job_finished` | the same set, plus `sources`, `tunnelDomains` and `ca` |
| `unit_state_changed` | patch `status.units` in place at once; then `services` when the unit is a service instance, `php` when it is a `php-fpm` pool, `tunnels` when it is `tunnel-<site>` |
| `site_added`, `site_removed` | `sites`, `status` |
| `port_conflict` | `ports` |
| `tunnel_changed` | upsert into `tunnels` from the event |
| the window becoming key, the menu opening | `status`, plus whatever the visible screen shows |

The last row covers a daemon older than 2.11 (no `reconciled`), and costs one local call per
visible list. Nothing runs on a timer.

### 7.3 Idempotence

Applying an event twice, or after the fetch that already reflects it, changes nothing: upserts are
keyed by name, `unit_state_changed` sets a value, and invalidations only cause a fetch.

## 8. Screens

### 8.1 Menu bar

`MenuBarExtra` with the `.window` style, so it can hold toggles and progress. Its label is an SF
Symbol chosen from `Health` (§10.4): normal, busy (a job runs), warning, error, offline. Its
content, top to bottom:

- A status line: `Yard <daemon version> · <profile> · .<tld>`, or the offline / not set up /
  stale message with its button (§5.3, §5.7).
- Problems, each one line with its action: a failed unit, a service that failed (its
  `ServiceState` failure text, §10.5), a port held by another program (holder name and kind), a
  `config_error`, a tunnel in `failed`.
- Running jobs: title, last output line, a progress spinner, Cancel.
- Sites: the five most recently opened from the app (kept in `UserDefaults`), else the first five
  by name; each opens its URL. A search field filters all sites.
- Services: one row each with a start/stop toggle and its state text (`running`, `sleeping 12m`,
  `waking`, `stopped`, `failed`); a row managed by a LaunchAgent (emailulator) has no toggle.
- Xdebug: one toggle per installed PHP version that has xdebug in its `ext.list`
  (`xdebug.set`); hidden when none has it.
- **Open Yard** (the main window), **Settings…**, **Quit Yard.app**. Quitting the app leaves
  yardd running; **Stop yardd** is in the main window's Overview, behind a confirmation, since it
  stops every site.

### 8.2 Main window

`Window("Yard", id: "main")` with a `NavigationSplitView`. The sidebar has a badge per section
with a problem. Each section is a list or table with a detail pane; every action shows its error
inline beside the control that started it (§10).

- **Overview:** yardd's version, protocol and commit (from `hello` and `status.version`), the
  app's own version and commit, profile and TLD, helper version (or **not installed**, with the
  onboarding step), CA trusted or not (**Trust**, `yard trust`; **Untrust**), `config_error` with
  **Open file** at its line, the units table (state, pid, restarts, last error; **Restart**,
  `units.restart`), and **Start / Stop / Restart yardd** (CLI).
- **Sites:** a searchable list (name, URL, PHP or proxy target, secure, a needs or error badge).
  **+** links a folder (open panel; the name defaults to the folder's; `sites.link` with the
  folder's resolved absolute path) or adds a proxy (`sites.proxy`). The detail pane (`sites.info`)
  shows each setting with where it comes from (`SiteDetail.sources`: from `.yard`, from the managed
  file, from `sites.toml`, default) and the controls for it:
  - PHP picker (`sites.isolate`, with **Default** to unisolate). When a committed `.yard` owns
    the setting the picker is disabled and says `set in <file>` with **Open file**; when a managed
    `.yard` owns it the change goes through and the inline note says `set in <file>`.
  - Secure toggle (`sites.secure` / `sites.unsecure`), docroot (`sites.docroot`, with **Detect**
    to clear), Node (`node.use` / `node.unuse`).
  - Services the `.yard` asks for (`SiteDetail.services`) with met or missing; needs and a
    missing Node with **Install…** (the settle flow, §11.1); `project_error` and
    `ignored_managed_file` as notes with their hint.
  - Tunnel: the URL and state, or **Share…** (§8.2 Tunnels).
  - Actions: **Open in browser**, **Open in editor**, **Reveal in Finder**, **Open in Terminal**,
    **Edit nginx override** (opens `nginx/overrides/<site>.conf`), **Copy .env**
    (`env.print { site }`, the lines joined as `yard env` prints them), **Save settings to .yard**
    (`sites.init`, managed by default, with a **Write it into the project** option for `local`;
    `SITE_NAME_TAKEN` asks to replace, §11.3), **Unlink** / **Remove proxy** after a confirmation.
- **Services:** a table (instance, engine and version, state, ports, sleep setting, managed by,
  used by). Row actions: start, stop, restart (`services.restart`), **Logs** (the last 100 lines
  from `services.logs`, then live `log_line`s for that unit), **Copy .env** (`env.print { service }`),
  **Open console** (`console`, MinIO and emailulator), **Port…** (`services.port`), **Sleep after…**
  (`services.sleep.set` with a duration, `off` or `default`), **Remove…** (§11.3). **Add service**:
  engine (MySQL, PostgreSQL, Redis, MinIO, emailulator), an optional line and an optional port, as
  the job `services.add`; `SERVICE_VERSION_UNSUPPORTED` turns the line field into a picker of
  `data.available`.
- **PHP:** each version from `php.list { available: true }` (§13): installed or not, default,
  sites on it, pool state. **Install** and **Uninstall** are jobs; **Make default**
  (`php.default`). For an installed version: its extensions (`ext.list { php }`) with
  enable/disable toggles, **Remove** (job), **Add extension…** (a name, `name-version` for a pecl
  release, or a `.so` chosen in an open panel, sent absolute; job), the Xdebug toggle, and
  php.ini overrides: **Open 99-overrides.ini** in the editor, plus a key/value form for
  `php.config.get|set|unset`.
- **Node:** installed versions (`node.list`), default, sites on each; **Make default**,
  **Uninstall** (job); **Install…** takes a spec (`22`, `lts/iron`); **Show available** runs
  `node.list { available: true }` once, on request (it asks nodejs.org).
- **Tunnels:** each tunnel (site, kind, URL, state, last error) with **Copy URL**, **Open**, and
  **Stop sharing** (`tunnels.remove`, with **Keep the Cloudflare tunnel** for a named one; the
  result's `dns_left` is shown as records to delete in Cloudflare). **Share…**: site, quick or
  named; a named one takes a host under a set-up domain (`tunnels.domains`) and **Overwrite an
  existing DNS record** for `TUNNEL_DNS_EXISTS`. **Domains**: list, **Add…** (the
  `tunnels.domain_add` job; its output lines show the sign-in URL as a link, and the sheet says
  `In the browser, pick the zone <domain>.`), **Remove**.
- **Ports:** `ports.list` as a table (unit, port, mode, preferred, status, holder and its kind).
  **Set port…**, **Pin** / **Unpin**, **Range…**, **Check** (`ports.check`), and the profile
  (`standard` or `privileged`, with what each means). A port held by another program shows the
  holder's name, pid and kind (Herd, DBngin, Postgres.app, Docker, brew services) and **Set
  port…**.
- **Logs:** a live view of every unit's `log_line`s while the section is visible (the `logs`
  connection), with a unit filter, a text filter, pause, and **Open log file** for a unit (opens
  `logs/<unit>.log` in Console). It holds the last 5,000 lines.
- **Doctor:** **Run doctor** (`doctor`), checks grouped as fail, warn, ok, each with its message,
  hint and action button (§10.2).
- **Sources:** `sources.list` (tap, tapped, trusted, formulae) with **Trust** / **Untrust** for a
  formula or a whole tap (jobs).
- **Activity** (a toolbar button with the count of running jobs): the job list (§9).

### 8.3 Settings

A `Settings` scene: the `yard` path (shows the located one, with **Choose…**), the editor (an app
chosen in an open panel; until one is chosen, **Open in editor** asks), the terminal app (default
Terminal), and the data root in use (read-only; it is set with `-YardDataRoot`).

### 8.4 Onboarding

A window shown on first launch and whenever Yard is not set up (§5.7), also reachable from the
Overview. It is a checklist computed from facts each time it is shown and whenever the app becomes
active, so a step already done shows done, and the user can do a step in Terminal and come back
**(builder's call)**. No timer: **Check again** re-reads the facts.

| Step | Done when | Action |
|---|---|---|
| 1. Homebrew | `/opt/homebrew/bin/brew` exists | none in the app; the brew.sh install command to copy, and the step blocks the rest |
| 2. The `yard` command | `yard` located (§5.8) | **Choose…**; M6 does not install a symlink (§18 Q3) |
| 3. yardd | the socket answers `hello` | **Start yardd** (`yard start`), then waits up to 5 s for the socket, as `yard start` does |
| 4. Root helper (DNS for `.<tld>`, and 80/443 if wanted) | `status.version.helper` is set | **Open in Terminal**: `sudo <yard> helper install` (§10.3), with the reason it needs a password |
| 5. HTTPS (optional) | `ca.info.trusted` | **Trust Yard's certificate** (`yard trust`; macOS asks for the password) |
| 6. Shims on PATH (optional) | never checked (the app has no shell PATH) | shows `yard install-shims`'s line with **Copy** |
| 7. Ports 80/443 (optional) | profile is `privileged` | **Use 80 and 443** (`ports.profile privileged`); yardd refuses while another program holds them and names it (Herd on this Mac) |
| 8. First site (optional) | `sites` is not empty | **Link a folder…** (§8.2 Sites) |

The helper is step 4, not optional: without `/etc/resolver/<tld>` no `.yard` name resolves.

## 9. Jobs and progress

### 9.1 `JobsStore`

A job is a `JobRecord` keyed by `(epoch, job_id)`:

- `title`: set by the action that started it (`Install PHP 8.3`, `Add the redis extension to PHP
  7.1`, `Add MySQL 8.4`, `Install Node 22`, `Share blog`, `Sign in to Cloudflare for example.com`,
  `Trust minio`); a job started elsewhere (the CLI) gets one from its id's kind (`php-install-3` →
  `Install PHP (started elsewhere)`), else its id.
- `phase`: `waiting(behind:)` while its only line is `waiting for <job_id>` (the daemon's first
  line for a queued job), `running`, `succeeded(result)`, `failed(YardError)`, `cancelled`
  (`JOB_CANCELLED`), or `lost` (the connection dropped while it ran: `yardd's connection was lost
  while this ran; its result is unknown`, hint `yard doctor`).
- `lines`: the last 2,000 output lines; `startedAt`, `finishedAt`.

The store keeps every running job and the last 50 finished ones.

### 9.2 Starting a job

`ActionRunner.runJob(method, params, title)` checks `events` is up (§5.5), calls the method, and
waits for the job's end:

- `job_progress` or `job_finished` for an id the store has not seen creates the record (origin
  "elsewhere" until claimed). When the call answers `{ job_id }`, the record is claimed: given its
  title and marked as the app's. Either order gives one record.
- `runJob` returns `.succeeded(result)`, `.failed(YardError)`, `.cancelled` or `.lost` when the
  record ends, including when it had already ended before the answer arrived.
- **Cancel** calls `jobs.cancel { job_id }`; `JOB_NOT_FOUND` means it already ended and is
  ignored.

### 9.3 Showing it

The control that started a job shows a spinner and the last line; the Activity list shows every
job with a scrolling, monospaced, selectable log, Cancel, and on failure the error (§10) with
`data.tail` (`JOB_FAILED`) or `data.output` (`EXT_VERIFY_FAILED`, `TUNNEL_DNS_FAILED`) shown as a
log. Lines that contain an `http(s)` URL show it as a link (cloudflared's sign-in URL). A finished
job's `result` is decoded by the action that started it (`TunnelInfo` for `tunnels.add`,
`TunnelRemoved` for `tunnels.remove`).

## 10. Errors and hints

### 10.1 One error type

`YardError { code: ErrorCode, message: String, hint: String?, data: JSONValue? }` is the only error
YardKit and YardModel produce. The model keeps the last error per action
(`ActionID`, e.g. `site:blog:secure`), cleared when the action next succeeds, so the view shows it
beside the control. Connection-level errors show as the window banner and the menu's status line.

### 10.2 Hints become actions **(builder's call)**

`HintAction.parse(_ hint: String) -> HintAction` splits the hint with shell-word rules (single and
double quotes, so `yard node install 'lts/*'` parses) and matches a fixed table:

| Hint | Action |
|---|---|
| `yard start`, `yard stop && yard start` | run the CLI (restart yardd) |
| `yard trust` | run the CLI |
| `yard php install <v>`, `yard ext add <x> --php <v>`, `yard ext enable|disable <x> [--php <v>]`, `yard ext remove <x> --php <v>`, `yard node install <spec>`, `yard service add <engine> [--version <l>]`, `yard service start <i>`, `yard source trust <formula>` | the matching RPC call or job |
| `yard service logs <i>`, `yard doctor`, `yard services`, `yard info …` | go to that screen |
| `yard service port <i> <port>`, `yard ports set <unit> <port>` with a `<…>` placeholder | open the port field for that instance or unit |
| `yard init`, `yard init --yes` | go to the site's settle flow |
| `edit <file>` (a `.yard` edit) | **Open file** |
| `sudo yard helper install`, `brew install|reinstall <formula>` | **Open in Terminal** (§10.3) and **Copy** |
| anything else (`rm …`, `mv …`, `tail …`, `emailulator …`, `sudo security …`) | **Copy** only |

A hint that matches a row but carries an argument that does not look like the API's (a PHP version
outside `\d+\.\d+`, an instance name with a space) falls through to **Copy**. The table is data the
tests walk row by row, with hints taken from the daemon's code.

### 10.3 Terminal hand-off

For the two allowlisted kinds above, the app writes a `0700` file `yard-<uuid>.command` in its temp
directory containing `#!/bin/zsh`, the command with each argument single-quoted, and
`read -k1 '?Press a key to close.'`, then opens it with the chosen terminal app, which runs it. The
user types the sudo password there. The file is deleted when the app next starts. The app never
passes a hint's text to this; it rebuilds the command from the parsed action's arguments.

### 10.4 Health

`Health.evaluate(...)` is a pure function of the connection state, `status` (`config_error`, units
`failed`), `ports` (a status `in_use`), `services` (any `failed`), `tunnels` (any `failed`) and
running jobs. It returns `.offline`, `.error([Problem])`, `.warning([Problem])`, `.busy` or `.ok`,
with each problem's text and action. A unit in `backoff` or a service `waking` is not a problem.

### 10.5 Service state text

Swift ports of two yard-proto helpers, tested against the same cases: `ServiceState.failure(name:)`
(the error a failed instance reports: its reason's code, message and hint, else `JOB_FAILED` with
the log's last line and hint `yard service logs <i>`), and the CLI's `state_text` (`sleeping
<age>` in the largest whole unit, `45s`, `12m`, `2h`, `3d`; `waking`).

## 11. Confirmations and installs

The CLI asks on a terminal; the app asks in a sheet. These mirror the cli-output rule.

### 11.1 Needs (`yard link`, `yard init`, a site's **Install…**)

`SettlePlan.make(site:)` turns `SiteInfo.needs` and a missing Node (`SiteInfo.node` with a spec
and no installed version) into ordered steps, as the CLI's `settle` does: PHP first; then fetch
the site again (a new PHP may have some extensions compiled in) and take its extension needs; then
services; Node last. A `Need::Manual` is listed with its command and never run; an `Other` is
listed as `something a newer yardd asks for`. The sheet lists `<what>  (<command>)` and asks
**Install these?**; each step runs as a job with its own progress. A missing Node alone never
blocks: the site serves, and the sheet's **Not now** leaves the note `<site> runs Node <spec>,
which is not installed`.

### 11.2 Trust (`FORMULA_UNTRUSTED`)

When a call or job fails with `FORMULA_UNTRUSTED` and `data` names `formula` and `tap`, the app
asks **Trust <formula> (from the <tap> tap)?**; yes runs the `sources.trust` job and then repeats
the original call once. A skipped settle never answers trust for the user, as `--yes` never does.

### 11.3 Deleting and replacing

- **Remove service:** first refuse what yardd would refuse, from the data at hand (unknown
  instance; `used_by` not empty → `SERVICE_IN_USE` naming the sites, hint the `.yard` to edit),
  then ask **Delete <i> and its data?** with a **Keep its data** checkbox (`keep_data`).
- **Unlink / remove proxy:** **Stop serving <site>?** A named tunnel's `SITE_TUNNELLED` is shown
  with its hint.
- **Save settings to .yard** over another directory's site: `SITE_NAME_TAKEN` asks **Replace it
  with this directory?** and repeats with `replace`.
- **Stop yardd:** **Stop yardd? Every site and service stops until it starts again.**

## 12. Testing

### 12.1 YardKit (`swift test`)

- Fixtures: every file in `crates/yard-proto/fixtures/` decodes, and re-encodes to an equal JSON
  value; the hand-written forward-compatibility cases decode to their unknown cases;
  `YardProtocol.version` equals `protocol.json`; `Methods` names equal `methods.json`.
- `LineSplitter`: partial lines across chunks, several lines in one chunk, an empty line, a line at
  the size limit.
- `UnixSocket` and `YardConnection` against `FakeDaemon`: `hello` is the first request; a
  `PROTOCOL_MISMATCH` answer ends the connection; answers are matched by id when the fake answers
  out of order; an event that arrives before an answer is delivered as an event; a missing socket
  is `DAEMON_NOT_RUNNING`; a call the fake never answers times out (the timeout is a parameter,
  50 ms in tests); a path over 104 bytes is refused.
- `YardClient`: subscribes before the first call; a stale minor subscribes without `state`;
  stopping the fake fails pending calls with the lost-connection error and reports
  `.disconnected`; starting it again reconnects, re-subscribes and reports a new epoch; a slow
  method uses its own connection while a quick one answers.
- `DaemonLocator` against a temporary LaunchAgent plist; `CLIRunner` against a stub `yard` shell
  script that prints JSON and exits 0, 1 or 3.

`FakeDaemon` is a real unix-socket server under a short `/tmp/yard-app-test-<random>` directory. A
test scripts it: the answer per method (a value or a `YardError`), events to push, a delay, an
out-of-order answer, closing the connection, and stopping and starting the listener.

### 12.2 YardModel (`swift test`)

Driven by `FakeYardService` (a scripted `YardService` with no socket) and `FakeCLI`:
`AppModel` refreshes per §7.2 (each row of the table is a test); `JobsStore` handles both arrival
orders, `waiting for`, cancel, `JOB_NOT_FOUND`, lost-on-reconnect and id reuse in a new epoch;
`ActionRunner` keeps and clears errors per action and runs the trust flow; `HintAction` walks the
§10.2 table; `Health` and the service state text; `SettlePlan`; onboarding steps from facts.

### 12.3 The app target

No UI tests in M6 (they need a UI test target and a signed, running app). Every view has a SwiftUI
preview fed by `PreviewData` built from the Rust fixtures, so a broken view is caught by building.
The plan ends with a manual checklist run on this Mac against a scratch daemon and then the real
one.

### 12.4 CI

`.github/workflows/ci.yml` gains a job on `macos-15`: `swift test --package-path app`. The Rust job
already runs the fixture check. Tuist and `xcodebuild` are not run in CI in M6 (M7 adds the app
build with signing). The package uses tools version 6.0 and no language feature newer than the
runner's Swift (§16).

## 13. Rust-side changes (protocol 2.11)

M6 needs three small changes, each additive:

1. **Wire fixtures** (§6.1): `crates/yard-proto/src/fixtures.rs` (test-only) and
   `crates/yard-proto/fixtures/*.json`. No protocol change.
2. **`reconciled` event:** `Topic::State` (`"state"`) and `Event::Reconciled { config_error:
   Option<YardError> }`, emitted by `Reconciler::reconcile` after every attempt, success or failure.
   Without it a client cannot see a change made from the CLI or a hand edit (a site secured, a
   service stopped, a TOML that stopped parsing) without polling, which main spec §9 rules out
   (§19.4). The CLI subscribes to no new topic, and an older CLI skips the unknown event.
3. **`php.list { available }`:** `PhpListParams { available: bool }` (`null` is the defaults, as
   for `node.list`); with `available`, every supported version that is not installed is listed
   too, with `installed: false`. The CLI gets `yard php list --available`, so the app still does
   nothing the CLI cannot. Without it the app would copy yard-core's `SUPPORTED` list.

`PROTOCOL_VERSION` becomes `"2.11"`. Each change touches, per the rpc-protocol rule, the proto
const and types, the daemon, the CLI's `MockDaemon`-based tests, and main spec §6.2.

Not in M6, noted for later: a `jobs.list` method (jobs that started before the app connected
are seen from their next line on), structured hint actions in `YardError` (§10.2 parses text), and
an `Other` fallback for `ErrorCode` in Rust (§19.6).

## 14. What M7 must be able to do, and how M6 keeps it open

- **Bundle id** `dev.more.yard.app` **(builder's call)**, beside the launchd labels
  `dev.more.yard.daemon` and `dev.more.yard.helper`; M7's `SMAppService` plists name it in
  `AssociatedBundleIdentifiers`.
- **No App Sandbox:** the app connects to a socket in `~/.yard`, runs `yard`, and opens arbitrary
  project paths. The hardened runtime needs no exception for any of that, so M7 can turn it on.
- **Binaries in `Contents/Helpers/`:** §5.8 already looks there last, so M7 only changes the
  order. Moving yardd into the bundle changes the LaunchAgent's `ProgramArguments` and the shims'
  link target (reconcile links `bin/<shim>` to the `yard-shim` beside yardd), which M7 must plan.
- **Version stamping:** the app's `Info.plist` gets `CFBundleShortVersionString` from the
  workspace version (`0.1.0`), and a build phase stamps `YardBuildCommit` and a monotonic
  `CFBundleVersion` (git commit count), as zetty's `Project.swift` does, so the app can say which
  build it is.

## 15. Failure modes

| Situation | What the app does |
|---|---|
| yardd restarts while a job runs | the job shows **lost**; lists are fetched again; a new job may reuse the id, under the new epoch |
| The user runs `yard secure blog` in a terminal | `reconciled` arrives; `sites` and the open detail are fetched again |
| A TOML stops parsing | `reconciled { config_error }`; banner with the file and line and **Open file**; the last good state keeps showing |
| yardd is older than the app (not restarted after a build) | connected and stale; banner with **Restart yardd** (§5.3) |
| A brew install takes 30 minutes and the window is closed | the menu bar shows it running; reopening the window shows its whole log |
| The helper is not installed | Overview and onboarding say so; DNS for `.<tld>` does not work; **Open in Terminal** for the install |
| `yard` cannot be found | CLI actions disabled with the hint to set its path; RPC still works |
| Another program holds a service's port | the service shows failed with `PORT_IN_USE` naming the holder, and **Port…** |
| A hint the app does not know | shown with **Copy** |
| The socket path is too long (a deep `-YardDataRoot`) | an error naming the path and the 104-byte limit |

## 16. Builder check

Questions a Swift engineer new to this repo would ask, with the answers folded into the sections
above.

- **How do I run a daemon to build against?** `cargo run -p yard-daemon --bin yardd -- --root
  /tmp/yard-dev` in one terminal (AGENTS.md), then launch the app with `-YardDataRoot /tmp/yard-dev`
  (set it in the Tuist scheme's run arguments). The CLI against it is `cargo run -p yard-cli --
  --root /tmp/yard-dev …`. Never point a development build at `~/.yard` while changing behaviour
  (AGENTS.md, "Verifying a change on this machine").
- **Where is the wire format?** `crates/yard-proto/src/{types,methods,events,error,rpc}.rs` are the
  definition; `crates/yard-proto/fixtures/` are examples of every shape; §6.2 lists the tricky ones.
- **Xcode or `swift test`?** Both: `swift test --package-path app` for everything in the package;
  `app/scripts/build.sh` (Tuist and `xcodebuild`) to build and run the app.
- **Which Swift features can I use?** Swift 6 language mode, `@Observable`, typed throws, Swift
  Testing. Not features newer than CI's Swift (the `macos-15` runner's Xcode): no
  `@concurrent`, no module-wide default `MainActor` isolation. The first task records the
  runner's Swift version.
- **Who owns which thread?** The socket's read source runs on its private serial queue and feeds an
  `AsyncStream`; `YardConnection` and `YardClient` are actors; models and views are `@MainActor`.
  Nothing blocks the main actor on I/O; `CLIRunner` waits for its process off the main actor.
- **What about SIGPIPE?** `SO_NOSIGPIPE` on every socket, so a write to a closed socket is an
  `EPIPE` error, not a signal that kills the app.
- **Why does an event arrive before the call that caused it answers?** Different connections
  (§5.2); `JobsStore` and the refresh policy are built for either order.
- **Can a model write a file or launch a process?** No. It calls `YardService`, `CLIRunning`,
  `Opener` and `TerminalHandoff`, protocols the app target (or a test fake) implements.
- **What is `client_env` in `doctor`?** The CLI sends its shell's `XDG_CONFIG_HOME` and
  `HOMEBREW_FORBIDDEN_*` so yardd can say where its environment differs. The app has no shell
  environment, so it sends `null`; yardd treats that as empty.
- **Do paths need canonicalizing?** Yes, as the CLI does: a folder from the open panel is sent as
  `URL.resolvingSymlinksInPath().path` to `sites.link`, `sites.init`, `env.print` and `sites.info`,
  since yardd compares resolved paths.
- **How do I see a view without a daemon?** Its preview, fed by `PreviewData`.
- **What counts as done for a UI task?** It builds with `app/scripts/build.sh`, its previews render,
  and its items in the manual checklist pass against a scratch daemon.

## 17. When built

The same change updates: main spec §3.1 (helper registration in M6 or M7, per §18 Q1), §3.2
(`app/` layout), §6.2 (methods, the `state` topic and `reconciled`, fixtures instead of
`cargo xtask schema`), §9 (as built), §10 (Swift testing), §12 (M6); this document's status;
`AGENTS.md` (status, the app's build and test commands, the layout line for `app/`); `README.md`;
the rpc-protocol rule (protocol 2.11, fixtures, the Swift models); the root-helper rule (the
`SMAppService` line); the cli-output rule (`yard php list --available`); and a new project rule
`.hydra/rules/swift-app.md` for `app/**` (module boundaries, no writes, hint allowlist, tests),
if the maintainer wants the app's conventions as a rule like the other areas.

## 18. Open questions for the maintainer

Most important first. Each has a recommended answer; the plan assumes it.

1. **The root helper in M6.** The main spec says the app registers the helper with `SMAppService`
   from M6. That needs a Developer ID–signed app (M7), the user's approval in System Settings, and
   a plist fixed inside the bundle, while today's helper plist carries the owner's uid, user, data
   root and brew prefix as install-time arguments, which is the helper's whole trust model
   (root-helper rule). **Recommended:** M6 keeps `sudo yard helper install`, handed to Terminal from
   onboarding (§8.4, §10.3); `SMAppService` moves to M7 with its own small design of how the helper
   learns its owner.
2. **How the Swift models are kept in step.** The spec says generated from JSON Schema
   (`cargo xtask schema`). **Recommended:** hand-written models checked against fixtures the Rust
   tests write (§6.1); no `schemars`, no generator, and the fallbacks survive.
3. **Which binaries the app uses in M6.** **Recommended:** none of its own: it uses the `yard`
   beside the yardd the LaunchAgent runs (`target/release` on this Mac), with a Settings override
   (§5.8). Carrying them in `Contents/Helpers/` and a `yard` symlink on PATH wait for M7, since
   they move the LaunchAgent and the shims' link target.
4. **Protocol 2.11 in M6.** **Recommended:** yes to the `reconciled` event and `php.list {
   available }` with `yard php list --available` (§13). Without the first the app goes stale
   after any CLI change; without the second it copies yard-core's list of supported versions.
5. **Menu bar app or Dock app.** **Recommended:** a menu bar app (`LSUIElement`), which shows a
   Dock icon only while the main window is open; quitting the app leaves yardd running.
6. **Tuist or plain SwiftPM.** **Recommended:** Tuist with a local package, as zetty does (§4.1).
   Plain SwiftPM is possible with a bundling script that M7 would grow into signing.

Smaller ones, with what the plan assumes: no system notifications in M6 (assumed: none); the Logs
section shows live lines only (assumed: yes); one plan for M6 rather than two (assumed: one plan,
in three parts: Rust, the package, the app).

## 19. Conflicts with the main spec and the code

1. **Generated models (§6.2, §10, rpc-protocol rule).** There is no `cargo xtask schema` and no
   schema dependency, and the hand-written serde shapes (`ServiceState`, flattened tagged enums,
   `#[serde(other)]` on tagged enums) would not survive generation with their fallbacks (§6.1,
   §18 Q2).
2. **`SMAppService` from M6 (§3.1, §9, §11, root-helper rule).** Blocked on signing and on the
   helper's install-time plist arguments (§18 Q1).
3. **Binaries in `Contents/Helpers/` (§9).** The LaunchAgent and the shim links follow wherever the
   running `yard` and `yardd` live; bundling them is a move that M7 has to plan (§14, §18 Q3).
4. **"No polling" (§9) with today's events.** No event says that a site's settings, a service's
   entry, the ports registry or `config_error` changed; only adds, removes and unit states do. The
   `reconciled` event (§13) closes this.
5. **"Logs: unified filterable stream" (§9).** Only live `log_line`s and `services.logs` tails
   exist; nginx, dnsmasq and php-fpm have no history through the API. The app shows live lines and
   opens the log file.
6. **`ErrorCode` has no fallback in Rust.** A CLI talking to a newer yardd that returns a new code
   fails to decode the whole answer (`unexpected shape`). The app tolerates it (§6.3); the CLI fix
   is outside M6.
7. **"Install CLI symlink" in onboarding (§9).** Where to (`/usr/local/bin` needs admin) depends on
   §18 Q3; M6 locates `yard` instead.
8. **Smaller gaps the screens work around:** `php.config` has no "list all overrides" (the app
   opens `99-overrides.ini` and offers a per-key form); a service engine's lines are known only
   inside `services.add` (the app learns them from `SERVICE_VERSION_UNSUPPORTED`); there is no
   `jobs.list` (§13); the per-site PHP picker is refused for a site whose committed `.yard` owns
   PHP (`SITE_HAS_PROJECT_FILE`), so the picker explains instead of failing.
