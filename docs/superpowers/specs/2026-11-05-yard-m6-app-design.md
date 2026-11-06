# Yard M6: Yard.app — design

Date: 2026-11-05, rewritten 2026-11-06 for Rust and GPUI
Status: built 2026-11-06 (maintainer approved the plan that day; built inline on main, Tasks 1–17).
Where the build departed from this text it says so in the commit messages; the main departures:
the table draws plain rows
rather than a virtual list, the log view copies all lines rather than selecting text, GPUI's
accessibility tree cannot be read in tests (widgets assert the id, role and label they set), and
the site detail sets the docroot by Detect and Node by Use default only. The final review
(2026-11-07) found features this text names that the first build lacked (the Overview's CA,
Remove keeping data, Save settings' location and replace, Not now, a `.so` from an open panel,
Range…, per-formula trust, a failed job's output, named shares, Copy URL and the DNS records a stop
leaves); they are built. A named share takes its host as text; the daemon checks it is under a
set-up domain. A picker's popup closes by choosing or by its control, not by a click elsewhere.
Open: in Activity, the accessibility tree gives every job card after the first the wrong or no
log lines, though the screen is right. It extends `2026-10-19-yard-design.md` (the main spec: §3.1, §6.2, §9, §10, §11,
§12) and is planned in `docs/superpowers/plans/2026-11-05-yard-m6-app.md`.

The first draft (2026-11-05) described a SwiftUI app built with Tuist over a Swift package with
hand-written wire models. On 2026-11-06 the maintainer decided the app is written in Rust with GPUI, "so we
have a unified language" **(maintainer)**. This version replaces the Swift draft. What did not depend on
the UI toolkit (screens, refresh policy, jobs, errors and hints, confirmations, onboarding) is kept
and reworded; layout, toolchain, wire models, the menu bar, native integrations and testing are
new.

Decisions marked **(maintainer)** are the maintainer's. Decisions marked **(builder's call)** were made while
writing this, not by the maintainer; each can be reversed without changing anything else here unless it says
otherwise. The questions §18 put to the maintainer were decided at his request on 2026-11-06, and the places where the main spec and
the code disagree with building M6 as the spec describes it are in §19.

## 1. Intent

Yard.app is a Rust client of `yardd`, written with GPUI (Zed's UI framework), beside the `yard`
CLI: a menu bar item for the day-to-day (is everything up, open a site, start a service, flip
Xdebug), a main window for everything the daemon API offers, and onboarding for a Mac where Yard is
not set up yet.

It holds no business logic (main spec §3.1): every change it makes is a JSON-RPC call to yardd or
a run of the `yard` CLI, so everything it can do, the CLI can do. What it adds is presentation:
live state, progress for long jobs, errors with a button that fixes them, and confirmations.

Why now: M1–M5 are built and verified, so the API the app speaks is complete for v1 (main spec
§12). Writing the app in Rust means it uses `yard-proto`'s types as they are, shares a client
crate with the CLI, and is reviewed with the same tools (`cargo test`, clippy, `cargo deny`).
Goal 4 (code quality and clear boundaries) makes the app's boundaries part of the deliverable: a
GPUI-free model crate tested with plain `cargo test`, a client crate the CLI also uses, and a thin
GPUI layer on top.

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
7. Clear crates: the transport in `yard-client` (shared with the CLI), decisions in
   `yard-app-model` (no GPUI, no I/O), views and platform glue in `yard-app`. Most logic is tested
   with `cargo test`.

### Non-goals (M6)

- Signing, notarization, a DMG, auto-update (appdater or Sparkle), a brew tap: M7 (main spec §11).
- Registering the helper or yardd with `SMAppService`, launch at login, and carrying `yard`,
  `yardd`, `yard-helper` and `yard-shim` inside the bundle: M7 (§18 Q1 and Q6).
- System notifications for finished jobs (an in-app record is kept; see §9). GPUI has
  `App::show_system_notification`, but the user notification center needs a signed bundle, so this
  is cheap in M7, not before.
- A history of nginx, dnsmasq and php-fpm logs: the API streams live lines only (§19.5).
- Editing user-owned files inside the app. The app opens them in the user's editor.
- Linux, Windows, Intel Macs (main spec §1). GPUI runs on Linux and Windows, but nothing else in
  Yard does, and the app is built and checked for `aarch64-apple-darwin` only.

### Anti-goals (never)

- The app never writes a TOML, a `.yard`, an override file or anything else in Yard's data root
  or a project. Changes go through RPC or the CLI.
- The app never runs `brew`, `launchctl`, `security`, `sudo` or a shell itself. Offline steps run
  the `yard` CLI with an argument vector; anything needing a password in a terminal is handed to
  the terminal app for the user to run (§10.3).
- The app never executes a hint string through a shell. Hints are parsed into a fixed set of
  typed actions; anything else is offered as text to copy (§10.2).
- The app never touches Herd, its files, or ports 80/443 except through `ports.profile`, which
  yardd refuses while Herd holds them (herd-coexistence rule).
- The app never depends on a Zed crate outside GPUI's own tree. Zed's `ui`, `theme`, `editor` and
  most of the editor are GPL-3.0-or-later; only GPUI and the crates it pulls in are Apache-2.0.
  The licence gate (§4.4) fails a build that pulls one in.

## 3. Constraints

- macOS 14+ (Sonoma), Apple Silicon. GPUI itself needs macOS 12 and the macOS 26 SDK (Zed,
  2026-10-26); nothing in M6 needs 14, so the floor can drop to 12 if the maintainer wants **(builder's
  call)**. Xcode 26.6 is installed on both Macs.
- The app's crates build with the Rust toolchain Zed uses at the pinned GPUI revision (1.98.1),
  in their own Cargo workspace; the daemon, helper, CLI and shims stay on 1.92 (§4.3).
- The protocol is NDJSON JSON-RPC 2.0 over `<root>/yardd.sock`, `hello { protocol, client }`
  first, every Yard error JSON-RPC code `-32000` with `data = YardError` (rpc-protocol rule).
- Types are defined once in `yard-proto` and the app uses them unchanged. The app and the yardd it
  talks to can still be different builds (yardd not restarted after a build, or built from a newer
  commit than the app), so every enum a newer yardd may extend has a fallback in `yard-proto`
  itself (§6.2, §13).
- Herd keeps working (herd-coexistence rule). The default profile stays `standard`.
- Never commit without asking the maintainer; lean prose commit messages (AGENTS.md).
- Disk: a GPUI build is large (several GB of `target/`). Builds clean their output at the end of a
  task. The LaunchAgent runs `target/release/yardd` in the root workspace; never clean it.

## 4. Layout and the GPUI dependency

### 4.1 A second Cargo workspace in `app/` **(builder's call; §18 Q3)**

```
crates/                      # the root workspace, Rust 1.92, unchanged deny.toml
  yard-proto/  yard-core/  yard-daemon/  yard-helper/  yard-cli/  yard-shim/
  yard-client/               # new: the connection to yardd, shared by the CLI and the app (§5)
app/                         # a separate workspace: its own Cargo.lock, toolchain and deny.toml
  Cargo.toml                 # [workspace] members = ["yard-app-model", "yard-app"]
  Cargo.lock
  rust-toolchain.toml        # Zed's channel at the pinned GPUI revision (1.98.1)
  deny.toml                  # the root policy plus what GPUI needs (§4.4)
  yard-app-model/            # lib: decisions, no GPUI, no tokio, no I/O
  yard-app/                  # bin `Yard`: GPUI views, the menu bar, platform glue, the tokio bridge
    build.rs                 # stamps YARD_BUILD_COMMIT and YARD_BUILD_DATE, as yard-cli's does
    macos/Info.plist         # the bundle's plist template
    macos/AppIcon.icns
  scripts/bundle.sh          # cargo build, then assembles Yard.app (§14.2)
```

The app crates reach the root workspace by path (`yard-proto = { path = "../../crates/yard-proto"
}`, likewise `yard-core` and `yard-client`). A path crate keeps inheriting `version.workspace`,
`edition.workspace` and its dependencies from its own workspace, so nothing in `crates/` changes to
be used from `app/`.

Why a second workspace rather than `crates/yard-app` in the root one:

- **Toolchain.** Zed builds GPUI with Rust 1.98.1 at the pinned revision and declares no
  `rust-version`; whether GPUI compiles on 1.92 is untested and would be a gamble at every bump.
  The root workspace pins 1.92 because CI's clippy runs on it and a newer rustc lints differently
  (AGENTS.md). Two toolchains keep the daemon's lint and test baseline still while the app follows
  Zed.
- **Supply chain.** One workspace has one `Cargo.lock` and one `cargo deny` graph. GPUI adds about
  580 crates, six git sources and four licences the root policy does not allow (§4.4). In a
  separate lockfile they never enter the graph `yardd`, the root helper and the shims are built
  from.
- **Cost.** Two `cargo` invocations in CI and two `deny` runs; `yard-proto`, `yard-core` and
  `yard-client` must compile on both toolchains (newer rustc is backward compatible; its new
  warnings in those crates show in the app build but do not fail it, since `cargo clippy -- -D
  warnings` applies only to the app workspace's own members). The draft's `app/` directory name is
  kept, so the main spec's layout line changes only its description.

The alternative is to bump the root workspace to 1.98.1 and add the app as `crates/yard-app`: one
toolchain, one lockfile, but every GPUI bump would move the daemon's toolchain and widen its
licence and source policy.

### 4.2 Where GPUI comes from **(builder's call; §18 Q2)**

What exists on 2026-11-06:

| Source | State | Sources gate | Notes |
|---|---|---|---|
| crates.io `gpui` 0.2.2 | published by Zed (owners `maxbrunsfeld`, `mikayla-maki`, the `zed-industries` crates-io team) on 2025-10-22, Apache-2.0 | passes, all registry | a year behind Zed: no AccessKit, no `gpui_platform` split, no SDK 26 work. Zed: automated releases "very soon"; until then "you will need to pin a git revision" (issue #66275, 2026-10-30) |
| git `zed-industries/zed` at a release tag's commit | `gpui` + `gpui_platform` at `v1.27.0` = `c84af5b99cbc366d675064e52b23664a015b6e35` (2026-11-04) | needs `allow-git` for six repos | the code Zed ships; AccessKit (since 2026-05-27), `set_activation_policy(Accessory)`, system notifications, anchored popups |
| `gpui-unofficial` 1.27.0 (and `*-gpui-unofficial`) | republished from Zed tags by `gpui-archipelago` ("not affiliated with Zed Industries"), Apache-2.0 | passes, all registry | renames every crate and rewrites dependencies (Zed's git `wgpu` becomes crates.io `wgpu` 30.0.1); a third party between Zed and us |
| other forks (`gpui-ce` 0.3.0, `open-gpui`, `adabraka-gpui`, Longbridge's `gpui-pre` under `gpui-component`) | single-maintainer forks with their own changes | mostly registry | diverge from Zed; `gpui-component` would add a second copy of GPUI |

The choice: **a git dependency on `zed-industries/zed`, pinned by `rev` to the commit of a Zed
stable release tag**, `gpui` and `gpui_platform` (feature `font-kit`), and nothing else from Zed.

- It is Zed's code, from Zed's repository, under the licence Zed states (Apache-2.0 for every Zed
  crate in GPUI's tree; checked for `v1.27.0`: `gpui`, `gpui_platform`, `gpui_macos`, `gpui_apple`,
  `collections`, `refineable`, `scheduler`, `sum_tree`, `http_client`, `ztracing` and the others).
  The mirror passes the sources gate only by moving trust to an unaffiliated publisher whose
  transforms change the dependency tree, which is the opposite of what the rust-dependencies rule
  asks when it says to compare a crate's publisher with the real maintainer.
- A `rev` cannot move under us (a tag can), and the lockfile records it. A bump is a deliberate
  task: pick the next Zed stable tag, read its GPUI changes, update the `rev`, review the
  `Cargo.lock` diff, run `cargo deny check`, build, run the app's tests and checklist.
- When Zed publishes current GPUI releases to crates.io (their stated plan), the app moves to
  `gpui = "=<version>"` from crates.io and drops `allow-git`. That is a one-line change per crate
  plus the deny file.

The cost: cargo clones Zed's repository (about 540 MB) and `gfx-rs/wgpu` (about 90 MB) into
`~/.cargo/git` once per machine and per CI cache, and resolving the lockfile needs the network the
first time.

### 4.3 Toolchain and build prerequisites

- `app/rust-toolchain.toml`: `channel = "1.98.1"`, `components = ["clippy", "rustfmt"]`, the
  channel of `rust-toolchain.toml` at the pinned Zed revision. A GPUI bump that changes Zed's
  channel changes this file in the same commit. On each Mac: `rustup toolchain install 1.98.1
  --component clippy,rustfmt` (ask the maintainer; Machines in AGENTS.md records it).
- Xcode 26 (the macOS 26 SDK; GPUI requires it since 2026-10-26). GPUI compiles its Metal shaders
  at build time with `xcrun metal`, which on macOS 26 is a separate Xcode component
  (`xcodebuild -downloadComponent MetalToolchain`). It is installed on the Mac mini (Xcode 26.6).
  `yard-app` has a feature `runtime-shaders` that forwards to `gpui_platform/runtime_shaders`,
  which compiles the shaders when the app starts instead, for CI and for a Mac without the
  component **(builder's call)**. Release builds (M7) use precompiled shaders.
- GPUI on macOS needs no `bindgen` or libclang at `v1.27.0` (only `cbindgen`, a build dependency).

### 4.4 Supply chain

`app/deny.toml` starts as a copy of the root `deny.toml` (the `[bans] deny` tripwire verbatim, per
the rust-dependencies rule) and adds only what the resolved tree needs **(builder's call; §18
Q4)**. For `gpui` + `gpui_platform` at `v1.27.0` resolved in a scratch project on 2026-11-06:

- Licences beyond the root list: `BSD-2-Clause` (`arrayref`, `rav1e`, `av1-grain`, `v_frame`),
  `BSD-3-Clause` (`exr`, `lebe`, `ravif`, `avif-serialize`, `sha1_smol`, `subtle`, `tiny-skia`,
  `tiny-skia-path`), `Zlib` (`foldhash`, `slotmap`, `zlib-rs`) and `MPL-2.0` (`cbindgen`, a build
  tool; `dwrote`, Windows only; `option-ext`). Most arrive through `image`'s codecs. `MPL-2.0` is
  file-level copyleft: we ship none of those files modified, so it costs nothing, but it is a
  policy change the maintainer should see.
- Git sources, exact URLs in `allow-git`: `https://github.com/zed-industries/zed`,
  `https://github.com/zed-industries/font-kit` (macOS text), and four that the lockfile holds only
  for other targets (cargo locks every target): `https://github.com/gfx-rs/wgpu`,
  `https://github.com/zed-industries/oo7`, `https://github.com/zed-industries/scap`,
  `https://github.com/zed-industries/wasm_thread`. A bump that adds a git source fails `cargo deny
  check sources` until it is reviewed and listed.
- `tray-icon` 0.26 and `muda` 0.21 (§8.1, Tauri's, MIT or Apache-2.0) and the `objc2-*` 0.3
  crates (Zlib, Apache-2.0 or MIT) use the same `objc2` 0.6 and `objc2-app-kit` 0.3 GPUI does, and
  add no licence or source.
- Every new crate is vetted per the rust-dependencies rule before it enters `app/Cargo.lock`, and
  `cargo deny check` runs with `--manifest-path app/Cargo.toml` locally and in the supply-chain
  workflow, as a second pair of jobs beside the root ones.
- Builds use `--locked`. Building the app runs GPUI's and its dependencies' build scripts (Metal
  shader compilation, `cbindgen`), which is code execution at build time like any crate; the pin
  and the lockfile review are the defence.

### 4.5 Crates and their boundaries

| Crate | Workspace | Depends on | Holds | Never holds |
|---|---|---|---|---|
| `yard-proto` | root | serde | wire types, now with a fallback on every extendable enum (§13), `methods::ALL` | I/O |
| `yard-core` | root | yard-proto | as today, plus `present` (service state and sleep text, missing Node, install hints, moved from yard-cli) and `launchd::read_agent` (reads the LaunchAgent plist yard-core already renders) | I/O |
| `yard-client` | root | yard-proto, yard-core, tokio | the CLI's `Client` (moved, unchanged), the pipelined `Connection`, the reconnecting `Session`, the typed method catalogue `api` | presentation, GPUI |
| `yard-app-model` | app | yard-proto, yard-core, yard-client (types only) | invalidation and refresh bookkeeping, `JobsStore`, action errors and prompts, `HintAction`, `Health`, settle plans, onboarding steps, the terminal script, locating decisions, the log buffer | GPUI, tokio, files, processes, clocks (`now` is a parameter) |
| `yard-app` | app | everything above, `gpui`, `gpui_platform`, `tray-icon`, `muda`, `objc2*`, tokio | the tokio bridge, GPUI entities (`AppState`, `ActionRunner`, `LogsView`), views, widgets, the menu bar, platform glue (`CliRunner`, NSWorkspace, NSUserDefaults, terminal hand-off) | decisions a test would want to check without GPUI |

The dependency direction is one way: app → model → client → proto/core. `yard-app-model` is the
draft's `YardModel` without the observation layer; GPUI entities in `yard-app` wrap its plain
structs and call `cx.notify()` when one changes.

### 4.6 Threads: GPUI's main thread and a tokio runtime **(builder's call)**

GPUI runs its own executors (a foreground one on the main thread, a background pool); yard-client
uses tokio, like the CLI. `yard-app` owns one multi-thread tokio runtime with two worker threads,
started before GPUI and kept for the app's life (Zed's `gpui_tokio` does the same). The `Session`
runs there. Across the boundary:

- calls: `runtime.spawn(session.call(...))` returns a tokio `JoinHandle`, which any executor can
  await, so a GPUI task awaits it directly;
- updates (connection state and events) and log lines: `futures::channel::mpsc` senders on the
  tokio side, read by a GPUI foreground task (`cx.spawn`) that updates entities.

Nothing blocks the main thread on I/O. `CliRunner` runs `yard` through `tokio::process` on the same
runtime. Menu bar callbacks (§8.1) arrive on the main thread and are forwarded into GPUI through a
channel as well.

## 5. Talking to yardd

The transport lives in `yard-client` in the root workspace, so it is tested with the root's tools
and against a real yardd (§12.1), and the CLI keeps its own words for every failure because the
words are now one copy of code.

### 5.1 Socket and framing

- The socket is `<root>/yardd.sock`; `<root>` is resolved as in §5.8.
- `tokio::net::UnixStream`. A missing socket file or `ECONNREFUSED` is "not running"; any other
  error is reported with its text. The path must fit `sockaddr_un.sun_path` (104 bytes); a longer
  one is an error naming the path, never a truncation.
- Lines are read with `read_until(b'\n')` into a buffer capped at 16 MiB; a line over the cap with
  no newline closes the connection as a protocol error (a `doctor` report or a site list is far
  smaller). Empty lines are skipped.
- Writes are `yard_proto::rpc::encode_line` (one message plus `\n`), written whole. Rust's runtime
  ignores `SIGPIPE`, so a write to a closed socket is an `EPIPE` error, not a signal.

### 5.2 Connections **(builder's call)**

yardd answers the requests on one connection in order, awaiting each handler before reading the
next (`rpc/server.rs`), and pushes events on the same connection once it subscribes. So the
`Session` keeps separate connections, each greeted with `hello`:

| Connection | Opened | Used for |
|---|---|---|
| `events` | at connect, kept | `subscribe` to `units`, `sites`, `ports`, `jobs`, `tunnels` and, from protocol 2.11, `state` (§13); then only read |
| `calls` | at connect, kept | every quick call, pipelined: several requests may be in flight, answers matched by id |
| one-shot | per slow call, closed after its answer | `doctor`, `node.list` with `available`, `services.logs`, `ports.check`; these can take seconds and would hold up a click queued behind them |
| `logs` | while a log view is visible | `subscribe` to `logs` only |

Logs get their own connection because `log_line` is the busiest event and only a visible log view
needs it, and because re-subscribing on `events` to add or drop a topic replaces the subscription,
which could lose a job event in between. Whether a method is slow is a property of its entry in the
method catalogue (`api::Method::slow`, §6.3).

`Connection` (one greeted connection, pipelined) is a reader task that routes each line: a
notification goes to an event channel, a response to the `oneshot` its id waits on. The CLI keeps
its sequential `Client` (moved into yard-client unchanged); it does not need pipelining.

### 5.3 `hello` and protocol versions

- The app sends `hello { protocol: PROTOCOL_VERSION, client: "Yard.app <version> (<commit>)" }`,
  the same constant yardd and the CLI are built with, so the app and the protocol cannot drift
  within one build. `PROTOCOL_VERSION` is `"2.11"` once §13 lands.
- `PROTOCOL_MISMATCH` (another major) closes the connection; the app shows the daemon's message
  and hint (`upgrade yard and yardd together`) and does not retry until the user asks or yardd
  restarts (§5.6).
- A daemon with the same major and an older minor than the app (yardd not restarted after a
  build) is connected but **stale**: the `Session` subscribes only to the topics that minor knows
  (an unknown topic fails the whole `subscribe`, and `Topic` deliberately has no fallback on the
  daemon side), the app shows a banner `yardd speaks protocol <p>; this app needs <m>` with a
  **Restart yardd** button (`yard stop`, then `yard start`, §5.7), and keeps working with what the
  older daemon offers. `METHOD_NOT_FOUND` from a stale daemon is rewritten by the same code the CLI
  uses (`Client::stale`, moved into yard-client): `yardd speaks protocol <p> and has no <method>`,
  hint `yard stop && yard start`.
- A newer minor is fine: everything it adds is optional, and the fallbacks of §6.2 read what the
  app does not know.

### 5.4 Calls

- Ids are per connection, from 1. A response with an id nobody waits for is dropped; a line that
  is neither a response nor an event notification is ignored, as the CLI does.
- Timeout: 60 s per call, the CLI's `CALL_TIMEOUT` (a mutation reconciles, which may wait on the
  helper). On timeout the call fails with the CLI's text, `yardd did not answer <method> within 60
  s`, hint `yard doctor checks what is stuck`, and that connection is closed, since a late answer
  would otherwise arrive for a caller that gave up. `services.remove` waits for the unit to stop
  (30 s at most) inside the call and fits.
- Every failure a call can have is a `YardError` (§10.1): the daemon's own, or one the client makes
  with the CLI's wording: `DAEMON_NOT_RUNNING` (`yardd is not running`, hint `yard start`),
  `INTERNAL` `lost the connection to yardd: …` and `yardd closed the connection` (hint `yard doctor
  checks why`), and `PROTOCOL_MISMATCH` `yardd answered <method> in an unexpected shape: …` (hint
  `upgrade yard and yardd together`).
- Calls are never retried automatically. A mutation whose connection dropped may or may not have
  applied; after the reconnect every list is fetched again (§7), which shows what happened.

### 5.5 Events

- An event is a notification `{ "jsonrpc": "2.0", "method": "event", "params": <Event> }`, tagged
  by `type`. From §13 an unknown `type` reads as `Event::Unknown` and is ignored, so a newer
  daemon's events never break the stream.
- Subscribing happens before any call that starts a job (main spec §6.2: "a client subscribes to
  `jobs` before it calls"). The `events` connection is opened and subscribed before `calls` is
  used, and a job-starting action is refused while `events` is down (`INTERNAL`, `cannot follow
  the job's progress without yardd's events`, hint `yard start`).
- Job events and the answer that started the job travel on different connections, so either can
  arrive first. `JobsStore` handles both orders (§9.2).

### 5.6 Reconnecting

- When `events` or `calls` closes, the `Session` closes the other, fails every pending call with
  the lost-connection error, bumps its **epoch**, and reports `Disconnected`. It then tries to
  connect again after 0.25 s, 0.5 s, 1 s, 2 s, then every 5 s, for as long as the app runs. A
  connect attempt to a missing socket costs one syscall; this is reconnecting, not polling state.
- It also tries at once when the app becomes active, when the menu bar menu opens (if tray-icon
  reports it, §8.1), and after the app itself runs `yard start` (§5.7). While connected, that
  `reconnect_now` does nothing: `yard start` against a running yardd leaves it alone, and the jobs
  the connection follows must not turn lost (found by the final review, 2026-11-07).
- After a successful `hello` and `subscribe`, it reports `Connected(Session info)` with the new
  epoch, and the app fetches what the menu shows and every resource it holds (§7.2). Events that arrive while those fetches
  run are applied after them; every event's effect is idempotent (§7.3), so order does not matter.
- Job ids restart at 1 when yardd restarts (`<kind>-<n>`), so the app keys jobs by
  `(epoch, job_id)`. A job still running when the connection dropped becomes **lost** (§9.1).

### 5.7 When yardd is not running or not installed

The locator (§5.8) reads the LaunchAgent's plist, so the app can tell three cases apart:

| Facts | State shown | Offered |
|---|---|---|
| no `~/Library/LaunchAgents/dev.more.yard.daemon.plist`, or one serving another root, and no answer | **Yard is not set up** | the onboarding window (§8.4) |
| plist present, socket does not answer | **yardd is not running** | **Start yardd**: runs `yard start` |
| answers | connected (or stale, §5.3) | everything |

The offline commands are the CLI's (main spec §3.1, cli-output rule): `start`, `stop`, `version`,
`doctor`, `ports check`, `install-shims`, `trust`/`untrust` (which run `security` as the user),
and `helper install|uninstall` (sudo). The app runs them through `CliRunner` (`yard-app`):
`tokio::process::Command` with the located `yard`, an argument vector, `--json`, and `--root
<root>` when the root is not the default. The agent has one label and serves one root, so `yard
start` and `yard stop` refuse a root the installed agent does not serve (main spec §3): against a
scratch root, Start, Stop and Restart yardd say so instead of taking the agent from `~/.yard`, and
the locator counts an agent as installed only for the root it serves (2026-11-06, found by the
checklist script). Exit 0 decodes the yard-proto type; exit 1 decodes
`{"error": YardError}`; exit 3 is `DAEMON_NOT_RUNNING`; anything else is `INTERNAL` with stderr's
last line. An app launched from Finder has launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), which
is what `yard start` (`launchctl`) and `yard trust` (`/usr/bin/security`) need; `CliRunner` sets
exactly that PATH so a run from a terminal behaves the same.

While yardd is down the menu shows the offline state, the window shows its last fetched data
greyed with the banner above, and every control that calls yardd is disabled.

### 5.8 Finding `yard`, `yardd` and the data root **(builder's call; §18 Q6)**

In M6 the app carries no binaries. It follows the yardd the LaunchAgent runs:

1. **Data root:** the app's `--root <path>` argument (as `yard --root`; `cargo run -p yard-app --
   --root /tmp/yard-dev`, or `open Yard.app --args --root /tmp/yard-dev`), else the LaunchAgent
   plist's `EnvironmentVariables.YARD_ROOT` (written by `yard start --root …`), else `~/.yard`.
2. **`yard`:** the `YardCLIPath` setting when set, else the `yard` beside the plist's
   `ProgramArguments[0]` (`yard start` writes the `yardd` beside the `yard` that ran it, so on both
   Macs both are `target/release/`), else `Contents/Helpers/yard` inside the app when present (the
   M7 layout, main spec §9), else none: CLI actions are disabled with `yard was not found`, hint
   `set its path in Settings`.

Reading the plist is `yard_core::launchd::read_agent(bytes)` (the `plist` crate yard-core already
uses to render it); choosing among the candidates is `yard_app_model::locate(facts)`, pure; only
reading the file and checking the executable bit happen in `yard-app`. Paths under the root come
from `yard_core::Paths` (`daemon_socket`, the override files, `logs/<unit>.log`), so the app never
spells a path the daemon also spells.

## 6. Wire models

### 6.1 One set of types **(the maintainer's Rust decision makes this follow)**

The app uses `yard-proto`'s types directly: `StatusInfo`, `SiteInfo`, `ServiceInfo`, `Event`,
`YardError` and the params structs in `methods.rs`. The draft's hand-written Swift models, the
Rust-written JSON fixtures that checked them, and the main spec's JSON Schema and `cargo xtask
schema` are all unnecessary: a renamed field fails to compile in the app, the CLI and the daemon
in the same `cargo build`. The draft's `ServiceState`, flattened-enum and `Profile` decoding rules
are serde's, as the CLI already relies on.

What the shared types do not settle is the version skew between two builds (§3): the app built
from one commit talking to a yardd built from another.

### 6.2 Unknown values

An older app reading a newer daemon's answer must not fail the whole answer on one value it does
not know. Today some enums have a fallback (`Need::Other`, `ServiceManager::Other`,
`SettingSource::Other`, `ServiceState::Other`, the three tunnel enums) and the rest fail the
answer with `unexpected shape`. §13 adds an `Other` (or `Unknown`) variant the daemon never sends
to every enum yardd may extend in a minor version: `ErrorCode`, `Event`, `UnitState`, `PortMode`,
`PortStatus`, `HolderKind`, `CheckStatus`, `SiteSource`, `SiteKind`, `ProjectFileKind`,
`IgnoredBecause` and `Profile`. The CLI gets the same tolerance for free (§19.6). `Topic` keeps
none: it is what a client sends, and a daemon that silently accepted an unknown topic would hide a
subscription that does nothing.

Presentation of an unknown value: `ErrorCode::Other` shows the error's message and hint, which
carry the meaning; an unknown unit or port state shows `unknown`; an unknown site kind shows the
name and URL only.

### 6.3 Methods

`yard_client::api` lists one typed constant per daemon method:
`pub const SITES_LIST: Method<(), Vec<SiteInfo>>`, `pub const PHP_INSTALL: Method<PhpParams,
JobStarted>`, each with its name (the `yard_proto::methods` const) and whether it is slow (§5.2).
`Session::call(method, params)` encodes the params and decodes the result type, so a caller cannot
pair a method with the wrong types. `node.list` has two entries, `NODE_LIST` and the slow
`NODE_LIST_AVAILABLE`. A test checks that every name in `yard_proto::methods::ALL` except `hello`
and `subscribe` has an entry.

## 7. State in the app

### 7.1 `AppState`

`AppState` is a GPUI entity that owns the update stream from the `Session` and one `Resource<T>`
per list: `status`, `sites`, `services`, `php`, `node`, `tunnels`, `tunnel_domains`, `ports`,
`sources`, `ca`, plus `extensions[php]` and `site_detail[site]` loaded on demand, and `doctor`,
fetched only when asked (it is slow and runs brew). A `Resource` holds the last value, the last
error, whether a fetch is in flight, and when it last succeeded. Views read resources; they never
call the client. The bookkeeping (what to fetch next, coalescing, one fetch in flight per key) is a
plain struct in `yard-app-model` (`RefreshQueue`); `AppState` runs its decisions with GPUI timers
and tasks.

### 7.2 Refresh policy: events invalidate, lists come from their method **(builder's call)**

Most events say that something changed rather than what it is now (`site_added` lacks the
per-request `node`; a sleeping service is a `unit_state_changed` with no `sleeping_since`), so an
event marks resources stale and the app fetches them again, coalesced over 150 ms, with at most
one fetch in flight per resource and one more queued if it was invalidated meanwhile.

| Trigger | Effect |
|---|---|
| connected (new epoch) | `status` and what the menu bar menu and its icon read (`sites`, `services`, `ports`, `tunnels`, and `php` with the `extensions` of each installed version, for the Xdebug submenu), which are always on screen, plus every resource loaded once |
| `reconciled` (§13) | `status`, `sites`, `services`, `php`, `node`, `tunnels`, `ports`, loaded `extensions`, loaded `site_detail` |
| `job_finished` | the same set, plus `sources`, `tunnel_domains` and `ca` |
| `unit_state_changed` | patch `status.units` in place at once; then `services` when the unit is a service instance, `php` when it is a `php-fpm` pool, `tunnels` when it is `tunnel-<site>` |
| `site_added`, `site_removed` | `sites`, `status` |
| `port_conflict` | `ports` |
| `tunnel_changed` | upsert into `tunnels` from the event |
| the main window becoming active, the menu opening | `status`, plus whatever the visible screen shows |

The last row covers a daemon older than 2.11 (no `reconciled`), and costs one local call per
visible list. Nothing runs on a timer, and drawing a screen never fetches: a version's
`extensions` come with the PHP list, so a failed `ext.list` is not asked again each frame.

### 7.3 Idempotence

Applying an event twice, or after the fetch that already reflects it, changes nothing: upserts are
keyed by name, `unit_state_changed` sets a value, and invalidations only cause a fetch.

## 8. Screens

### 8.1 Menu bar: a native status item and menu **(builder's call; §18 Q5)**

GPUI has no status item API (0.2.2 ships a `status_item.rs` that no module includes, and
`gpui_macos` at `v1.27.0` has none), and its anchored popups need a parent GPUI window. Three ways to get one:

1. **`tray-icon` + `muda`** (Tauri's crates): an `NSStatusItem` with a native `NSMenu`, created on
   the main thread inside GPUI's `Application::run` callback; item clicks arrive through
   `MenuEvent::set_event_handler` on the main thread and are forwarded into GPUI by a channel.
2. **A GPUI window as a popover**: an `NSStatusItem` made with `objc2-app-kit`, whose button click
   opens a borderless `WindowKind::PopUp` GPUI window placed under the button's screen frame, closed
   when it loses focus.
3. **A fork's tray API**: ties the whole app to that fork (§4.2).

The choice is 1. A native menu is what macOS users expect from a menu bar item, VoiceOver and
keyboard navigation work in it without any work of ours, and there is no focus, Spaces or
full-screen handling to get right for an accessory app's floating window. tray-icon and muda use
the same `objc2` and `objc2-app-kit` versions as GPUI and add no licence. Option 2 stays open: if a
later milestone wants progress bars or a search field in the menu, the status item can open a GPUI
window instead without touching anything else.

What a native menu cannot hold changes the draft's `MenuBarExtra(.window)` content: no spinner, no
text field, no inline switch. Toggles are checkmark items, progress is a text line updated live,
and site search moves to the main window. The status item's icon is a template image chosen from
`Health` (§10.4): normal, busy (a job runs), warning, error, offline. The menu, top to bottom:

- A status line (disabled item): `Yard <daemon version> · <profile> · .<tld>`, or the offline /
  not set up / stale message, with its action as the next item (**Start yardd**, **Set up Yard…**,
  **Restart yardd**) (§5.3, §5.7).
- Problems, each one item that performs its action: a failed unit that is not a service instance, a
  service that failed (its
  `ServiceState::failure` text), a port held by another program (holder name and kind), a
  `config_error`, a tunnel in `failed`.
- Running jobs: one disabled item each, `<title> — <last output line>` (truncated to fit), followed
  by **Cancel <title>**.
- Sites: the five most recently opened from the app (kept in settings, §8.3), else the first five
  by name; each opens its URL. A **Sites** submenu lists every site.
- Services: one checkmark item each, `<instance> — <state text>` (`running`, `sleeping 12m`,
  `waking`, `stopped`, `failed`), checked while running; choosing it stops or starts the instance.
  An instance managed by a LaunchAgent (emailulator) is a plain item that opens its console.
- **Xdebug** submenu: one checkmark item per installed PHP version that has xdebug in its
  `ext.list` (`xdebug.set`); absent when none has it.
- **Open Yard** (the main window), **Settings…**, **Quit Yard.app**. Quitting the app leaves yardd
  running; **Stop yardd** is in the main window's Overview, behind a confirmation, since it stops
  every site.

The menu follows `AppState` whenever a resource it shows changes: item titles and check marks
change in place (muda), so an open menu shows a job's next line, and the menu is built again only
when an item is added, removed or replaced (a new id, or a problem gaining or losing its fix). A
click on a check item puts its mark back at once: the state, not the click, decides it. Whether
tray-icon reports the menu opening (to refresh `status`, §7.2) is checked in the plan's first app
task; without it the menu relies on events, which is enough from protocol 2.11.

### 8.2 Main window

One GPUI window, titled `Yard`, with a sidebar and a content pane. Opening a screen from anywhere
(the menu, a site row, a hint) shows it in that window when it is open and raises it; whether it is
open is read from the app's windows, since GPUI refuses to update a window from inside its own
click handler (until 2026-11-06 each click on a site row opened another window). The sidebar has a badge per
section with a problem. Each section is a list or table with a detail pane; every action shows its
error inline beside the control that started it (§10).

GPUI is a rendering framework, not a widget kit, and Zed's own widget crates are GPL (§2). The app
builds the few widgets it needs in `yard-app::widgets` **(builder's call)**: button, toggle,
checkbox, text field (adapted from GPUI's Apache-2.0 `examples/input.rs`), a list or table over
`uniform_list` (after `examples/data_table.rs`), a picker (an `AnchoredPopup` menu), a banner, an
error view with its hint button, and a form overlay for the forms below. Confirmations use GPUI's
`Window::prompt`, which on macOS is a native alert (`NSAlert`) with up to a few buttons.
`gpui-component` is not used: it brings a second copy of GPUI (`gpui-pre`).

- **Overview:** yardd's version, protocol and commit (from `hello` and `status.version`), the
  app's own version and commit, profile and TLD, helper version (or **not installed**, with the
  onboarding step), CA trusted or not (**Trust**, `yard trust`; **Untrust**), `config_error` with
  **Open file** at its line, the units table (state, pid, restarts, last error; **Restart**,
  `units.restart`), and **Start / Stop / Restart yardd** (CLI).
- **Sites:** a searchable list (name, URL, PHP or proxy target, secure, a needs or error badge).
  **+** links a folder (GPUI's `prompt_for_paths` open panel; the name defaults to the folder's;
  `sites.link` with the folder's canonical absolute path) or adds a proxy (`sites.proxy`). The
  detail pane (`sites.info`) shows each setting with where it comes from (`SiteDetail.sources`:
  from `.yard`, from the managed file, from `sites.toml`, default) and the controls for it:
  - PHP picker (`sites.isolate`, with **Default** to unisolate). yardd refuses `isolate`,
    `secure` and `docroot` for a site whose project has its own `.yard`, whatever keys it sets,
    and `init` finds the file there, so for such a site the PHP picker, the HTTPS switch,
    **Detect docroot** and **Save settings to .yard** are disabled, the picker saying `set in
    <file>` with **Open .yard**; a managed `.yard` is edited for you and locks nothing.
  - Secure toggle (`sites.secure` / `sites.unsecure`), docroot (`sites.docroot`, with **Detect**
    to clear), and a line saying the site's Node and where it is set (`Node 22 — not installed,
    from sites.toml`, `Node 22.11.0 — from /p/.nvmrc`), with **Use default Node** (`node.unuse`)
    only for a pin Yard made (sites.toml or the managed `.yard`).
  - Services the `.yard` asks for (`SiteDetail.services`) with met or missing; needs and a
    missing Node with **Install…** (the settle flow, §11.1); `project_error` and
    `ignored_managed_file` as notes with their hint.
  - Tunnel: the URL and state, or **Share…** (§8.2 Tunnels).
  - Actions: **Open in browser**, **Open in editor**, **Reveal in Finder**, **Open in Terminal**,
    **Edit nginx override** (opens `nginx/overrides/<site>.conf`), **Copy .env** (the text `yard
    env` prints, from `yard_core::present::env_text`)
    (`env.print { site }`, the lines joined as `yard env` prints them), **Save settings to .yard**
    (`sites.init`, managed by default, with a **Write it into the project** option for `local`;
    `SITE_NAME_TAKEN` asks to replace, §11.3), **Unlink** / **Remove proxy** after a confirmation.
- **Services:** a table (instance, engine and version, state, ports, sleep setting, managed by,
  used by). Row actions: start, stop, restart (`services.restart`), **Logs** (the last 100 lines
  from `services.logs`, then live `log_line`s for that unit), **Copy .env** (`env.print { service
  }`), **Open console** (`console`, MinIO and emailulator), **Port…** (`services.port`), **Sleep
  after…** (`services.sleep.set` with a duration, `off` or `default`), **Remove…** (§11.3). **Add
  service**: engine (MySQL, PostgreSQL, Redis, MinIO, emailulator), an optional line and an
  optional port, as the job `services.add`; `SERVICE_VERSION_UNSUPPORTED` turns the line field
  into a picker of `data.available`. Under the table, each failed instance says why with its fix,
  as `yard services` does (`ServiceState::failure`: `PORT_IN_USE` names the holder and offers
  **Port…**).
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
  `tunnels.domain_add` job; its output lines show the sign-in URL as a link, and the form says
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

A small GPUI window, one at a time (**Settings…** raises it when open): the `yard` path (shows the located one, with **Choose…**), the editor (an app
chosen in an open panel; until one is chosen, **Open in editor** asks), the terminal app (default
Terminal), and the data root in use (read-only; it is set with `--root`). A `yard` path chosen here or in
onboarding is run from then on, with no relaunch; Save says which `yard` is now in use, or that none
was found. Settings, the recent
sites and `did_onboard` live in `NSUserDefaults` under the bundle id, through `objc2-foundation`
**(builder's call)**: they are the app's own preferences, outside Yard's data root, and a later
signed build keeps them.

### 8.4 Onboarding

A window shown on first launch and whenever Yard is not set up (§5.7), also reachable from the
Overview's and the menu's **Set up Yard…**, which raise it when it is open (until 2026-11-07 the
menu's opened the main window and nothing reopened a closed checklist). It is a checklist computed from facts each time it is shown and whenever the app becomes
active, so a step already done shows done, and the user can do a step in Terminal and come back
**(builder's call)**. No timer: **Check again** re-reads the facts. A step's action that fails
says why in this window, under the steps (until 2026-11-06 Start yardd's error showed only on the
main window's Overview).

| Step | Done when | Action |
|---|---|---|
| 1. Homebrew | `/opt/homebrew/bin/brew` exists | none in the app; the brew.sh install command to copy, and the step blocks the rest |
| 2. The `yard` command | `yard` located (§5.8) | **Choose…**; M6 does not install a symlink (§18 Q6) |
| 3. yardd | the socket answers `hello` | **Start yardd** (`yard start`), then waits up to 5 s for the socket, as `yard start` does |
| 4. Root helper (DNS for `.<tld>`, and 80/443 if wanted) | `status.version.helper` is set | **Open in Terminal**: `sudo <yard> helper install` (§10.3), with the reason it needs a password |
| 5. HTTPS (optional) | `ca.info.trusted` | **Trust Yard's certificate** (`yard trust`; macOS asks for the password) |
| 6. Shims on PATH (optional) | never checked (the app has no shell PATH) | shows `yard install-shims`'s line with **Copy** |
| 7. Ports 80/443 (optional) | profile is `privileged` | **Use 80 and 443** (`ports.profile privileged`); yardd refuses while another program holds them and names it (Herd on the maintainer's main Mac) |
| 8. First site (optional) | `sites` is not empty | **Link a folder…** (§8.2 Sites) |

The helper is step 4, not optional: without `/etc/resolver/<tld>` no `.yard` name resolves.

### 8.5 Accessibility

- The menu bar menu is a native `NSMenu` and is read by VoiceOver like any other.
- Alerts from `Window::prompt` and the open panels are native.
- Everything else is drawn by GPUI. GPUI exposes an element to VoiceOver through AccessKit only
  when the element has an id and a role (GPUI's accessibility guide, AccessKit support since
  2026-05-27); an element without them is invisible to assistive technology. So every widget in
  `yard-app::widgets` sets a role and a label, and every list row an id that stays the same across
  frames (the instance or site name), or VoiceOver announces rows as removed and added on each
  refresh.
- Text is the same: GPUI puts a `Text` with an id in the tree and never a plain string, so every
  visible line that is not a control's own label (headings, table cells, Overview's facts, the site
  detail, Doctor's results, job titles, onboarding's steps) goes through `widgets::text(id, …)` with
  an id stable across frames. Until 2026-11-06 the app drew plain strings and VoiceOver could read
  only its controls.
- A control that is greyed out is also unavailable to assistive technology, with its reason as its
  description: GPUI's builder has no disabled state, so `widgets::disabled::Disabled` wraps the
  element and marks its AccessKit node disabled (buttons, the picker, the switch). A press from
  VoiceOver reaches the handler directly (`widgets::Pressable::on_press`): GPUI turns an
  accessibility press into a mouse click at the element's centre, which lands nowhere when the
  element is scrolled out of view. A log line is text like any other, and a table column has a title
  (the PHP and Node tables' **Default**). All found by the checklist script, 2026-11-06.
- The check: GPUI's tree cannot be read in a test, so `app/scripts/checklist.sh` reads it through
  macOS accessibility (`app/scripts/ax.js`) against a scratch yardd: every control on all 11 screens
  has a label (A3); Overview, the Sites table, the site detail, the Services failures, Logs, Doctor
  and onboarding expose their text, and the disabled controls say so and why (A1). What VoiceOver
  says aloud, and whether a refresh re-announces rows (A2), are still checked by ear.
  A screen that does not read well is a bug for M6, not a later polish item, but full parity with a
  SwiftUI app is not promised. GPUI on crates.io (0.2.2) has no AccessKit at all, which is one more
  reason for the git pin (§4.2).

## 9. Jobs and progress

### 9.1 `JobsStore`

A job is a `JobRecord` keyed by `(epoch, job_id)`:

- `title`: set by the action that started it (`Install PHP 8.3`, `Add the redis extension to PHP
  7.1`, `Add MySQL 8.4`, `Install Node 22`, `Share blog`, `Sign in to Cloudflare for example.com`,
  `Trust minio`); a job started elsewhere (the CLI) gets one from its id's kind (`php-install-3` →
  `Install PHP (started elsewhere)`), else its id.
- `phase`: `Waiting { behind }` while its only line is `waiting for <job_id>` (the daemon's first
  line for a queued job), `Running`, `Succeeded(result)`, `Failed(YardError)`, `Cancelled`
  (`JOB_CANCELLED`), or `Lost` (the connection dropped while it ran: `yardd's connection was lost
  while this ran; its result is unknown`, hint `yard doctor`).
- `lines`: the last 2,000 output lines; `started_at`, `finished_at`.

The store keeps every running job and the last 50 finished ones. It is a plain struct in
`yard-app-model`; `yard-app` wraps it in an entity.

### 9.2 Starting a job

`ActionRunner::run_job(method, params, title)` checks `events` is up (§5.5), calls the method, and
waits for the job's end:

- `job_progress` or `job_finished` for an id the store has not seen creates the record (origin
  "elsewhere" until claimed). When the call answers `{ job_id }`, the record is claimed: given its
  title and marked as the app's. Either order gives one record.
- `run_job` resolves to `Succeeded(result)`, `Failed(YardError)`, `Cancelled` or `Lost` when the
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

`yard_proto::YardError { code, message, hint, data }` is the only error the client, the model and
the app produce. The app keeps the last error per action (`ActionId`, e.g. `site:blog:secure`),
cleared when the action next succeeds, so the view shows it beside the control. An action can
start from the menu, a hint on another screen or a file open, so whatever error the open screen
does not show beside its own controls appears above its content: every failure reaches the screen
the person is on (found by the final review, 2026-11-07: Restart on a unit, the menu's Xdebug switch
and a Doctor hint's `yard trust` failed with no word). Connection-level
errors show as the window banner and the menu's status line; a configuration error's banner has
**Open file** for its `data.file`.

### 10.2 Hints become actions **(builder's call)**

`HintAction::parse(hint: &str) -> HintAction` splits the hint with shell-word rules (single and
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
tests walk row by row, with hints taken from the daemon's code. Structured hint actions in
`YardError` would replace the parsing; they are not in M6 (§13).

### 10.3 Terminal hand-off

For the two allowlisted kinds above, the app writes a `0700` file `yard-<uuid>.command` in its temp
directory containing `#!/bin/zsh`, the command with each argument single-quoted, and
`read -k1 '?Press a key to close.'`, then opens it with the chosen terminal app through
`NSWorkspace openURLs:withApplicationAtURL:configuration:completionHandler:` (`objc2-app-kit`),
which runs it. The user types the sudo password there. The file is deleted when the app next
starts. The app never passes a hint's text to this; it rebuilds the command from the parsed
action's arguments. The script text is `yard_app_model::terminal_script(args)`, pure and tested.

### 10.4 Health

`Health::evaluate(...)` is a pure function of the connection state, `status` (`config_error`, units
`failed`), `ports` (a status `in_use`), `services` (any `failed`), `tunnels` (any `failed`) and
running jobs. It returns `Offline`, `Error(problems)`, `Warning(problems)`, `Busy` or `Ok`, with
each problem's text and action. A unit in `backoff` or a service `waking` is not a problem.

### 10.5 Service state text

No port needed: the app calls `yard_proto::ServiceState::failure(name)` (the error a failed
instance reports) and the CLI's `state_text` and `sleep_text` (`sleeping <age>` in the largest
whole unit, `45s`, `12m`, `2h`, `3d`; `waking`), which §13 moves from `yard-cli` into
`yard_core::present` so both clients link one copy.

## 11. Confirmations and installs

The CLI asks on a terminal; the app asks in an alert or a form. These mirror the cli-output rule.

### 11.1 Needs (`yard link`, `yard init`, a site's **Install…**)

`SettlePlan::new(&site)` (yard-app-model) turns `SiteInfo.needs` and a missing Node
(`yard_core::present::missing_node`) into ordered stages, as the CLI's `settle` does: PHP first;
then fetch the site again (a new PHP may have some extensions compiled in) and take its extension
needs; then services; Node last. A `Need::Manual` is listed with its command and never run; an
`Other` is listed as `something a newer yardd asks for`. The lines are `Need::describe()` and
`Need::command()` from yard-proto, so the form lists exactly what the CLI prints: `<what>
(<command>)`, then **Install these?**; each step runs as a job with its own progress. A missing
Node alone never blocks: the site serves, and **Not now** leaves the note `<site> runs Node <spec>,
which is not installed`.

### 11.2 Trust (`FORMULA_UNTRUSTED`)

When a call or job fails with `FORMULA_UNTRUSTED` and `data` names `formula` and `tap`, the app
asks **Trust <formula> (from the <tap> tap)?**; yes runs the `sources.trust` job and then repeats
the original call once. A skipped settle never answers trust for the user, as `--yes` never does.
The main window asks; when no window is open (an install started from the menu bar), the main window
opens on Activity to ask it.

### 11.3 Deleting and replacing

- **Remove service:** first refuse what yardd would refuse, from the data at hand (unknown
  instance; `used_by` not empty → `SERVICE_IN_USE` naming the sites, hint the `.yard` to edit),
  then ask **Delete <i> and its data?** with the buttons **Delete**, **Remove, keep its data**
  (`keep_data`) and **Cancel**.
- **Unlink / remove proxy:** **Stop serving <site>?** A named tunnel's `SITE_TUNNELLED` is shown
  with its hint.
- **Save settings to .yard** over another directory's site: `SITE_NAME_TAKEN` asks **Replace it
  with this directory?** and repeats with `replace`.
- **Stop yardd:** **Stop yardd? Every site and service stops until it starts again.**

## 12. Testing

### 12.1 `yard-client` (root workspace, `cargo test`)

- `Connection` against a scripted fake server (a tokio `UnixListener` under a short
  `/tmp/yard-test-*` directory, per the rust-workspace rule): `hello` is the first request; a
  `PROTOCOL_MISMATCH` answer ends the connection; answers are matched by id when the fake answers
  out of order; an event that arrives before an answer is delivered as an event; a missing socket
  is `DAEMON_NOT_RUNNING`; a call the fake never answers times out (the timeout is a parameter,
  50 ms in tests); a path over 104 bytes is refused; a 300 KB line split across reads arrives
  whole; an over-long line closes the connection.
- `Session`: subscribes before the first call; a stale minor subscribes without `state`; stopping
  the fake fails pending calls with the lost-connection error and reports `Disconnected`; starting
  it again reconnects, re-subscribes and reports a new epoch; a slow method uses its own connection
  while a quick one answers; logs use their own connection.
- One test in `crates/yard-daemon/tests/rpc.rs` runs a `Session` against a real yardd
  (`support::daemon::prepared`): connect, `sites.list`, a hand edit to `sites.toml` arrives as
  `reconciled`.
- The CLI's tests pass unchanged after its `Client` moves.

### 12.2 `yard-app-model` (app workspace, `cargo test`)

Plain unit tests, no GPUI and no runtime: `Invalidation::keys` per row of §7.2;
`RefreshQueue` coalescing and one-in-flight; `JobsStore` in both arrival orders, `waiting for`,
cancel, `JOB_NOT_FOUND`, lost-on-reconnect and id reuse in a new epoch; action errors kept and
cleared; the trust prompt; `HintAction` over the §10.2 table; `Health`; `SettlePlan`; onboarding
steps from facts; `locate`; `terminal_script` quoting; the log buffer.

### 12.3 `yard-app` (app workspace, `cargo test`)

- GPUI's test harness (`#[gpui::test]`, `TestAppContext`, `gpui` with `test-support` as a
  dev-dependency) runs entities headless with a deterministic clock. `AppState` and `ActionRunner`
  are tested against a `FakeDaemon` (a scripted implementation of the app's `Daemon` trait, no
  socket): the §7.2 refresh table, coalescing with `advance_clock`, `run_job` in both orders, the
  trust flow, the link and settle flows.
- Each screen gets a smoke test that renders it from sample data built from yard-proto types in a
  test window and does not panic. This replaces the draft's SwiftUI previews as "a broken view is
  caught before it is run".
- `CliRunner` against a stub `yard` shell script that prints JSON and exits 0, 1 or 3; the terminal
  launcher's file (mode, contents, cleanup) in a temp directory, without opening it.
- No UI automation in M6. The plan ends with a manual checklist on this Mac against a scratch
  daemon and then the real one, VoiceOver included.

### 12.4 CI

- The root `test` job is unchanged (Rust 1.92 on `macos-15`) and now also covers `yard-client`.
- A new job `app` on `macos-26` (the image with Xcode 26 and the macOS 26 SDK; GitHub made it
  generally available on 2026-02-26): `rustup show active-toolchain` in `app/` (1.98.1 from
  `app/rust-toolchain.toml`), `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked
  --features yard-app/runtime-shaders -- -D warnings`, `cargo test --workspace --locked --features
  yard-app/runtime-shaders`, with `~/.cargo/git` and `app/target` cached. `runtime-shaders` keeps
  CI independent of the Metal toolchain component; the first run records whether the image has it.
- The supply-chain workflow gets `gates` and `advisories` jobs for `app/` (`cargo-deny-action` with
  `manifest-path: app/Cargo.toml`).
- No bundle is built in CI in M6; M7 adds the signed build.

## 13. Rust-side changes

Five changes in the root workspace, each additive, before any app code:

1. **Fallbacks** (§6.2): an `Other`/`Unknown` variant the daemon never sends on `ErrorCode`,
   `Event`, `UnitState`, `PortMode`, `PortStatus`, `HolderKind`, `CheckStatus`, `SiteSource`,
   `SiteKind`, `ProjectFileKind`, `IgnoredBecause` and `Profile`, with `#[serde(other)]` (a unit
   variant also catches an unknown tag with fields on an internally tagged enum, as `Need::Other`
   does). The CLI shows them as `unknown`. Plus `yard_proto::methods::ALL`, every method const in
   file order. No wire change, no protocol bump.
2. **`reconciled` event:** `Topic::State` (`"state"`) and `Event::Reconciled { config_error:
   Option<YardError> }`, emitted by `Reconciler::reconcile` after every attempt, success or failure.
   Without it a client cannot see a change made from the CLI or a hand edit (a site secured, a
   service stopped, a TOML that stopped parsing) without polling, which main spec §9 rules out
   (§19.4). The CLI subscribes to no new topic, and an older CLI skips the unknown event.
3. **`php.list { available }`:** `PhpListParams { available: bool }` (`null` is the defaults, as
   for `node.list`); with `available`, every supported version that is not installed is listed
   too, with `installed: false`. The CLI gets `yard php list --available`, so the app still does
   nothing the CLI cannot. Without it the app would copy yard-core's `SUPPORTED` list.
4. **`yard-client`:** a new crate holding the CLI's `Client` (moved from `yard-cli/src/client.rs`,
   behaviour unchanged), the pipelined `Connection`, the reconnecting `Session` and the typed `api`
   catalogue. `yard-core` gains `present` (`state_text`, `sleep_text`, `missing_node`,
   `install_hint`, moved from yard-cli) and `launchd::read_agent`. No new third-party crate:
   tokio, serde_json, `time` and `plist` are already in the root lockfile.
5. `PROTOCOL_VERSION` becomes `"2.11"` with changes 2 and 3. Each touches, per the rpc-protocol
   rule, the proto const and types, the daemon, the CLI's `MockDaemon`-based tests, and main spec
   §6.2.

Not in M6, noted for later: a `jobs.list` method (jobs that started before the app connected are
seen from their next line on) and structured hint actions in `YardError` (§10.2 parses text).

## 14. What M7 must be able to do, and how M6 keeps it open

### 14.1 Identity

- **Bundle id** `dev.more.yard.app` **(builder's call)**, beside the launchd labels
  `dev.more.yard.daemon` and `dev.more.yard.helper`; M7's `SMAppService` plists name it in
  `AssociatedBundleIdentifiers`.
- **No App Sandbox:** the app connects to a socket in `~/.yard`, runs `yard`, and opens arbitrary
  project paths. The hardened runtime needs no exception for any of that or for GPUI (Metal needs
  no entitlement; no JIT), so M7 can turn it on with no entitlements.
- **Version stamping:** `yard-app/build.rs` sets `YARD_BUILD_COMMIT` and `YARD_BUILD_DATE` as
  `yard-cli/build.rs` does; `bundle.sh` writes `CFBundleShortVersionString` from the workspace
  version (`0.1.0`), `CFBundleVersion` from `git rev-list --count HEAD`, and `YardBuildCommit` into
  the plist, so the app can say which build it is in its About line and `hello.client`.
- **Activation:** `LSUIElement` true in the plist, and GPUI's `set_activation_policy` switches to
  `Regular` while the main or onboarding window is open (a Dock icon and app menu) and back to
  `Accessory` when they close.

### 14.2 The bundle **(builder's call)**

`app/scripts/bundle.sh` (`set -euo pipefail`): `cargo build --release --locked -p yard-app` in
`app/`, then assembles `app/target/Yard.app/Contents/{MacOS/Yard, Info.plist, Resources/AppIcon.icns}`
from `yard-app/macos/`, stamps the plist with `plutil`, checks the executable with `lipo -archs`
(arm64), and seals it ad hoc (`codesign --force --sign -`), printing that this build is for this
Mac only and not for shipping. A script rather than `cargo-bundle` or `cargo-packager`: the bundle
is three files, and M7 has to grow the script into the release-signing rule's order anyway (nested
binaries in `Contents/Helpers/` signed inside out, Developer ID with `--timestamp --options
runtime`, notarize, staple, DMG), following zetty's `scripts/package.sh`, the rule's reference; a
packaging tool would be one more dependency with its own signing config to verify.

### 14.3 Native pieces M7 adds, from Rust

- **`SMAppService`** (helper as a LaunchDaemon, yardd as a LaunchAgent, the app as a login item):
  `objc2-service-management` (`SMAppService::daemonServiceWithPlistName`, `agentServiceWithPlistName`,
  `mainAppService`, `register`, `status`). Blocked on signing and on the helper's install-time
  arguments (§18 Q1).
- **Notifications:** GPUI's `App::show_system_notification` (`objc2-user-notifications` inside
  `gpui_macos`); needs the signed bundle.
- **Auto-update:** Sparkle.framework in `Contents/Frameworks/`, driven through a few `objc2`
  `extern_class!` bindings to `SPUStandardUpdaterController`, or appdater (main spec §11); either is
  nested code signed before the app.
- **Binaries in `Contents/Helpers/`:** §5.8 already looks there last, so M7 only changes the order.
  Moving yardd into the bundle changes the LaunchAgent's `ProgramArguments` and the shims' link
  target (reconcile links `bin/<shim>` to the `yard-shim` beside yardd), which M7 must plan. `yard`,
  `yardd`, `yard-helper` and `yard-shim` come from the root workspace's release build;
  `bundle.sh` copies them in.

## 15. Failure modes

| Situation | What the app does |
|---|---|
| yardd restarts while a job runs | the job shows **lost**; lists are fetched again; a new job may reuse the id, under the new epoch |
| The user runs `yard secure blog` in a terminal | `reconciled` arrives; `sites` and the open detail are fetched again |
| A TOML stops parsing | `reconciled { config_error }`; banner with the file and line and **Open file**; the last good state keeps showing |
| yardd is older than the app (not restarted after a build) | connected and stale; banner with **Restart yardd** (§5.3) |
| yardd is newer than the app and returns an error code or state the app does not know | the fallback reads it (§6.2); the message and hint show as usual |
| A brew install takes 30 minutes and the window is closed | the menu shows it running with its last line; reopening the window shows its whole log |
| The helper is not installed | Overview and onboarding say so; DNS for `.<tld>` does not work; **Open in Terminal** for the install |
| `yard` cannot be found | CLI actions disabled with the hint to set its path; RPC still works |
| Another program holds a service's port | the service shows failed with `PORT_IN_USE` naming the holder, and **Port…** |
| A hint the app does not know | shown with **Copy** |
| The socket path is too long (a deep `--root`) | an error naming the path and the 104-byte limit |
| A build on a Mac without the Metal toolchain | `cargo build` fails in GPUI's build script naming `metal`; the hint (AGENTS.md) is `xcodebuild -downloadComponent MetalToolchain`, or build with `--features runtime-shaders` |

## 16. Builder check

Questions a Rust engineer new to GPUI and this repo would ask, with the answers folded into the
sections above.

- **How do I run a daemon to build against?** `cargo run -p yard-daemon --bin yardd -- --root
  /tmp/yard-dev` from the repo root, then `cargo run -p yard-app -- --root /tmp/yard-dev` from
  `app/`. The CLI against it is `cargo run -p yard-cli -- --root /tmp/yard-dev …`. Never point a
  development build at `~/.yard` while changing behaviour (AGENTS.md, "Verifying a change on a
  real machine").
- **Which toolchain?** Whatever directory you run cargo in: the root pins 1.92, `app/` pins
  1.98.1. `cargo test` at the root never builds GPUI.
- **Where is the wire format?** `crates/yard-proto/src/{types,methods,events,error,rpc}.rs`; the app
  uses those types, and `yard_client::api` says which params and result go with which method.
- **How do I learn GPUI?** The pinned revision's `crates/gpui/README.md`, `src/_accessibility.rs`,
  `src/_ownership_and_data_flow.rs` and `examples/` (`input.rs`, `data_table.rs`, `uniform_list.rs`,
  `popover.rs`, `a11y.rs`, `testing.rs`). Read them at the pinned `rev`, not on Zed's main, which
  changes daily. Never copy from Zed's `ui` or `editor` crates (GPL).
- **Who owns which thread?** GPUI's foreground executor owns entities and views on the main thread;
  the tokio runtime owns sockets and processes; channels and `JoinHandle`s cross between (§4.6).
  Nothing blocks the main thread on I/O.
- **Why does an event arrive before the call that caused it answers?** Different connections
  (§5.2); `JobsStore` and the refresh policy are built for either order.
- **Can the model crate write a file or launch a process?** No. `yard-app-model` has no tokio and
  no `std::fs` or `std::process` use; it takes facts and returns decisions. `yard-app` does the I/O.
- **What is `client_env` in `doctor`?** The CLI sends its shell's `XDG_CONFIG_HOME` and
  `HOMEBREW_FORBIDDEN_*` so yardd can say where its environment differs. The app has no shell
  environment, so it sends `null`; yardd treats that as empty.
- **Do paths need canonicalizing?** Yes, as the CLI does: a folder from the open panel is sent as
  `std::fs::canonicalize(path)` to `sites.link`, `sites.init`, `env.print` and `sites.info`, since
  yardd compares resolved paths.
- **How do I see a view without a daemon?** Its smoke test renders it from sample data; to look at
  it, run the app against a scratch daemon.
- **What counts as done for a UI task?** `cargo clippy` and `cargo test` in `app/` pass, its smoke
  tests pass, `scripts/bundle.sh` builds the app, and its items in the manual checklist pass
  against a scratch daemon.
- **How do I bump GPUI?** A task of its own: next Zed stable tag, its commit as `rev`, Zed's
  toolchain channel at that commit into `app/rust-toolchain.toml`, review the `Cargo.lock` diff and
  any new git source or licence, `cargo deny check --manifest-path app/Cargo.toml`, tests,
  checklist.

## 17. When built

The same change updates: main spec §2 Goal 4 and the intro (Rust and GPUI), §3.1 (helper
registration in M6 or M7, per §18 Q1), §3.2 (`app/` as a second workspace, `yard-client`), §6.2
(the `state` topic and `reconciled`, the fallbacks; no JSON Schema, no `cargo xtask schema`), §9 (as
built: the native menu, GPUI), §10 (the app's tests), §11 (M7's signing of a Rust bundle), §12
(M6); this document's status; `AGENTS.md` (status, Goal 4, the layout lines for `yard-client` and
`app/`, Build & test: the app's commands and toolchain, Machines: the 1.98.1 toolchain and the
Metal component on each Mac); `README.md`; the rpc-protocol rule (protocol 2.11, the fallbacks, the
"Swift models" line removed); the rust-workspace rule (`yard-client`, the second workspace and
toolchain); the root-helper rule (the `SMAppService` line); the cli-output rule (`yard php list
--available`); and a new project rule `.hydra/rules/app.md` for `app/**` (crate boundaries, no
writes, the hint allowlist, roles and labels on every widget, how to bump GPUI, never a Zed GPL
crate), if the maintainer wants the app's conventions as a rule like the other areas (he did: added
2026-11-07).

## 18. Questions, decided

Most important first. The Rust + GPUI choice itself is decided **(maintainer, 2026-11-06)**. On
2026-11-06 the maintainer asked Claude to decide the rest ("decide what's best"), and each was decided as
recommended below, the smaller ones included **(Claude, at the maintainer's request, 2026-11-06)**. The maintainer can
still reverse any of them; the plan follows these answers. Approving the plan stays the maintainer's.

1. **The root helper in M6.** The main spec says the app registers the helper with `SMAppService`
   from M6. That needs a Developer ID–signed app (M7), the user's approval in System Settings, and
   a plist fixed inside the bundle, while today's helper plist carries the owner's uid, user, data
   root and brew prefix as install-time arguments, which is the helper's whole trust model
   (root-helper rule). **Decided:** M6 keeps `sudo yard helper install`, handed to Terminal from
   onboarding (§8.4, §10.3); `SMAppService` moves to M7 with its own small design of how the helper
   learns its owner.
2. **Where GPUI comes from.** crates.io `gpui` 0.2.2 is Zed's but a year old (no AccessKit); the
   `gpui-unofficial` mirror is current but republished by a third party with rewritten
   dependencies; forks diverge. **Decided:** a git dependency on `zed-industries/zed` pinned by
   `rev` to a stable release tag's commit (`v1.27.0`, `c84af5b9…`), bumped deliberately, moving to
   crates.io once Zed's automated GPUI releases land (§4.2).
3. **A second workspace and toolchain for the app.** GPUI at that revision is built by Zed with
   Rust 1.98.1; the root pins 1.92. **Decided:** `app/` as its own Cargo workspace with its own
   `rust-toolchain.toml` (1.98.1), lockfile and deny policy, using `yard-proto`, `yard-core` and
   `yard-client` by path; the daemon's toolchain and supply-chain policy do not move (§4.1). The
   alternative is bumping everything to 1.98.1 in one workspace.
4. **The app's supply-chain policy.** GPUI's tree needs `BSD-2-Clause`, `BSD-3-Clause`, `Zlib` and
   `MPL-2.0` on the licence list and six git repositories on `allow-git` (§4.4). **Decided:**
   allow them in `app/deny.toml` only; the root `deny.toml` stays as it is.
5. **The menu bar.** **Decided:** a native status item and `NSMenu` through `tray-icon` and
   `muda`: accessible and native for free, at the cost of the draft's in-menu spinner, search field
   and switches, which become live text lines, a Sites submenu and checkmark items (§8.1). The
   alternative is a GPUI popover window under a status item made with `objc2`.
6. **Which binaries the app uses in M6.** **Decided:** none of its own: it uses the `yard`
   beside the yardd the LaunchAgent runs (`target/release` on both Macs), with a Settings override
   (§5.8). Carrying them in `Contents/Helpers/` and a `yard` symlink on PATH wait for M7, since
   they move the LaunchAgent and the shims' link target.
7. **Protocol 2.11 in M6.** **Decided:** yes to the `reconciled` event and `php.list {
   available }` with `yard php list --available` (§13). Without the first the app goes stale
   after any CLI change; without the second it copies yard-core's list of supported versions.
8. **Menu bar app or Dock app.** **Decided:** a menu bar app (`LSUIElement`), which shows a
   Dock icon only while the main or onboarding window is open; quitting the app leaves yardd
   running.

Smaller ones, decided as the plan assumes them: no system notifications in M6 (assumed: none); the Logs
section shows live lines only (assumed: yes); one plan for M6 rather than two (assumed: one plan,
in three parts: the root workspace, the app's model and platform, the screens); macOS 14 as the
floor (assumed: 14; GPUI allows 12).

## 19. Conflicts with the main spec and the code

1. **Generated models (§6.2, §10, rpc-protocol rule).** The main spec's `cargo xtask schema`, JSON
   Schema and generated Swift models no longer apply: the app uses `yard-proto` directly (§6.1).
   Those lines go, and nothing replaces them.
2. **SwiftUI in the main spec (§1 intro, §2 Goal 4, §3.2, §9, §10).** The main spec says the app is
   Swift/SwiftUI with `MenuBarExtra`, `NavigationSplitView` and `@Observable` stores, and that
   `app/` is a Swift package. Decided otherwise (maintainer, 2026-11-06); §17 lists the edits.
3. **`SMAppService` from M6 (§3.1, §9, §11, root-helper rule).** Blocked on signing and on the
   helper's install-time plist arguments (§18 Q1).
4. **"No polling" (§9) with today's events.** No event says that a site's settings, a service's
   entry, the ports registry or `config_error` changed; only adds, removes and unit states do. The
   `reconciled` event (§13) closes this.
5. **"Logs: unified filterable stream" (§9).** Only live `log_line`s and `services.logs` tails
   exist; nginx, dnsmasq and php-fpm have no history through the API. The app shows live lines and
   opens the log file.
6. **Enums without a fallback.** `ErrorCode` and ten other enums fail a whole answer when a newer
   yardd sends a value an older build does not know. With a Swift app this was the app's problem
   to work around; with a Rust app sharing `yard-proto` it is fixed once, in M6, for the app and
   the CLI (§13.1).
7. **"Install CLI symlink" in onboarding (§9).** Where to (`/usr/local/bin` needs admin) depends on
   §18 Q6; M6 locates `yard` instead.
8. **Binaries in `Contents/Helpers/` (§9).** The LaunchAgent and the shim links follow wherever the
   running `yard` and `yardd` live; bundling them is a move that M7 has to plan (§14.3, §18 Q6).
9. **One toolchain (AGENTS.md, README: "`rust-toolchain.toml` pins 1.92").** True for the root
   workspace; the app's workspace pins Zed's (§4.3, §18 Q3).
10. **Smaller gaps the screens work around:** `php.config` has no "list all overrides" (the app
    opens `99-overrides.ini` and offers a per-key form); a service engine's lines are known only
    inside `services.add` (the app learns them from `SERVICE_VERSION_UNSUPPORTED`'s
    `data.available`, which yardd sent only from 2026-11-06); there is no
    `jobs.list` (§13); `isolate`, `secure` and `docroot` are refused for a site whose project has
    its own `.yard` (`SITE_HAS_PROJECT_FILE`), so their controls explain instead of failing.
