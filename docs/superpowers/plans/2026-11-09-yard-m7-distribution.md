# M7 Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** draft, 2026-11-09, for the maintainer's review.

**Goal:** Yard installs with one command (a brew cask or `install.sh`), as one signed, notarized
Yard.app that carries the CLI, daemon, helper and shim; it updates through `brew upgrade`, `yard
self-update` or the app; `yard uninstall` removes it; and the repository goes public under MIT with
the maintainer's name and paths gone from its files and history.

**Architecture:** Root workspace first: a helper revision so updates stop asking for `sudo`, a pure
`yard_core::install` module (install kind, versions, the release feed, the uninstall plan), then
three CLI commands built on it (`self-update`, `uninstall`, and `start` installing nginx and
dnsmasq). Then packaging as shell: `bundle.sh --release` puts the four binaries in
`Contents/Helpers/`, `scripts/release.sh` signs, notarizes and publishes, `install.sh` installs, and a
cask template feeds the tap. Then the app's update and uninstall items, which run the CLI. Last,
the scrub, the history rewrite and the first release, each gated on the maintainer's approval.

**Tech Stack:** Rust 1.92 (root) and 1.98.1 (app, GPUI); `serde_json` for `latest.json`;
`/usr/bin/curl`, `ditto`, `codesign`, `spctl`, `launchctl`, `security` as subprocesses (no new
crates); POSIX `sh` for `install.sh`, bash for `release.sh`; `xcrun notarytool`, `stapler`,
`hdiutil`; `gh`; `git filter-repo` (brew, approval needed).

**Spec:** `docs/superpowers/specs/2026-11-09-yard-m7-distribution-design.md` (approved
2026-11-09). Section numbers below (§N) are that spec's.

## Global Constraints

- One version for every part (CLI, daemon, shim, helper, app), bumped together each release (§8.2).
- The helper's `HELPER_REVISION` changes only when its code, verbs or plist change (§8.2).
- arm64 only; `lipo -archs` prints exactly `arm64` for every shipped executable; macOS 14 floor.
- Bundle id `dev.more.yard.app`; labels `dev.more.yard.daemon`, `dev.more.yard.helper`.
- Release assets: `Yard-X.Y.Z-arm64.zip`, `Yard-X.Y.Z-arm64.dmg`, `SHA256SUMS`, `latest.json`
  `{ "version", "zip", "sha256", "notes_url" }`; feed URL
  `https://github.com/webteractive/yard/releases/latest/download/latest.json`, overridable by
  `YARD_RELEASE_URL` (a base URL; tests use `file://`).
- No Team ID, identity name or Apple credential in any script or source; the identity is found at
  run time by SHA-1 (`YARD_SIGN_IDENTITY` overrides), notarization uses the `notary` keychain profile.
- `codesign --force --timestamp --options runtime`, no entitlements, inside out; `notarytool submit
  --wait --output-format json` must say `Accepted`.
- Yard never runs `brew uninstall` for a formula and never grants brew trust; it may run `brew
  install nginx dnsmasq` (core formulas) on `yard start` after asking, and `brew uninstall --cask
  yard` in `yard uninstall` on a brew install.
- `~/.yard` is deleted only by `yard uninstall` without `--keep-data`, after the typed confirmation
  or `--yes`; the cask's `zap` never touches it.
- Herd, DBngin, Postgres.app, emailulator and Herd's `/etc/resolver/test` are never touched.
- Tests never touch the real data root, `/Applications`, the real LaunchAgent, the keychain or brew.
- Commits: lowercase prose subject, no type prefix, no trailers; ask the maintainer before each
  commit and push (AGENTS.md).
- Docs move with the code; no tracked file may contain a pattern from `.release-scrub` (Task 10).

## Review Focus

- `self-update` with the app running from the bundle it replaces: the swap is two renames on one
  volume, the running app keeps its open files, and the app relaunches from the same path (Task 3,
  Task 9 tests).
- The LaunchAgent runs a dev build's yardd when `self-update` runs from the installed bundle: yardd
  is not restarted and the output names the yardd it left (Task 3).
- `yard uninstall` on a half-installed Mac (no yardd running, no helper, no CA, no app, dev build):
  each missing piece is a skipped step, not a failure (Task 5).
- `install.sh` over an existing `~/.local/bin/yard` that links to `target/release/yard`: the link is
  replaced and the output says from what; a regular file there is refused (Task 7).
- The feed says a version older than or equal to the installed one (a dev bundle ahead of the
  release): "up to date", never a downgrade (Task 2).

---

## Part 1 — the root workspace

### Task 1: The helper's revision

**Files:**
- Modify: `crates/yard-helper/src/lib.rs` (constant beside `VERSION`), the `status` verb's answer
- Modify: `crates/yard-proto/src/helper.rs` (`HelperStatus`, `HelperInstall`)
- Modify: `crates/yard-daemon/src/doctor.rs:220-232`
- Modify: `crates/yard-cli/src/commands/helper.rs` (install output names the revision)
- Test: `crates/yard-daemon/src/doctor.rs` tests, `crates/yard-proto/src/helper.rs` tests

**Interfaces:**
- Produces: `yard_helper::HELPER_REVISION: u32` (starts at `1`); `HelperStatus { version: String,
  #[serde(default)] revision: u32 }` (an old helper answers without it and reads as `0`).

- [ ] **Step 1: Write the failing tests**
  - `helper_status_without_revision_reads_as_zero`: `serde_json::from_str::<HelperStatus>(r#"{"version":"0.1.0"}"#)` gives `revision == 0`.
  - In doctor's tests (fake helper status): `a_helper_of_another_version_but_this_revision_is_ok`
    (version `"0.0.9"`, revision `HELPER_REVISION` → check `helper` is ok, message contains `0.0.9`);
    `a_helper_of_another_revision_asks_for_install` (revision `HELPER_REVISION + 1` → warn, hint
    `sudo yard helper install`, message `the helper is revision {r}, but yard needs revision {R}`).
- [ ] **Step 2:** `cargo test -p yard-proto -p yard-daemon doctor helper` — Expected: FAIL (no field, no constant).
- [ ] **Step 3: Implement.** Add the constant and field; the helper's `status` fills it; doctor's
  helper check compares `s.revision == yard_helper::HELPER_REVISION`: equal → `ok("helper", "the
  helper {version} (revision {r}) is running")`, else the warn above. Keep `VERSION` reporting.
  A doc comment on `HELPER_REVISION` says when to bump it (§8.2).
- [ ] **Step 4:** `cargo test --workspace` — Expected: PASS. `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 5:** Update the root-helper rule (`.hydra/rules/root-helper.md`) with the bump rule. Commit `the helper reports a revision, and doctor asks for a reinstall only when it changes`.

### Task 2: `yard_core::install`

**Files:**
- Create: `crates/yard-core/src/install.rs`; Modify: `crates/yard-core/src/lib.rs`
- Test: unit tests in `install.rs`

**Interfaces:**
- Produces:
  ```rust
  pub const RELEASE_URL: &str = "https://github.com/webteractive/yard/releases/latest/download";
  pub fn release_url() -> String;            // YARD_RELEASE_URL or RELEASE_URL, no trailing '/'
  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
  pub struct Version { pub major: u32, pub minor: u32, pub patch: u32 }
  impl std::str::FromStr for Version { type Err = YardError; }  // "0.1.0", a leading 'v' allowed
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct Latest { pub version: String, pub zip: String, pub sha256: String, pub notes_url: String }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum InstallKind { Dev, Bundle { app: PathBuf }, Brew { app: PathBuf } }
  /// `exe` canonicalized; `caskroom` is `<brew prefix>/Caskroom/yard`.
  pub fn install_kind(exe: &Path, caskroom_exists: bool) -> InstallKind;
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum Update { UpToDate, Available(Version) }
  pub fn update(installed: Version, latest: Version) -> Update;
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
  pub enum UninstallStep { StopDaemon, Untrust, HelperUninstall, RemoveData, RemovePrefs, RemoveBundle, RemoveBrewCask }
  pub struct UninstallFacts { pub kind: InstallKind, pub agent_installed: bool, pub ca_exists: bool,
                              pub helper_installed: bool, pub keep_data: bool }
  pub fn uninstall_plan(f: &UninstallFacts) -> Vec<UninstallStep>;
  ```
- `install_kind`: `Bundle`/`Brew` when `exe` is `<X>.app/Contents/Helpers/yard` (the app is `<X>.app`),
  `Brew` when additionally `caskroom_exists`; anything else `Dev`.
- `uninstall_plan` order (§10): `StopDaemon` (if `agent_installed`), `Untrust` (if `ca_exists`),
  `HelperUninstall` (if `helper_installed`), `RemoveData` (unless `keep_data`), `RemovePrefs`
  (always; removing an absent file is a no-op), then `RemoveBundle` for `Bundle`, `RemoveBrewCask`
  for `Brew`, nothing for `Dev`. The CA trust check works without yardd (`yard_daemon::tls::trusted`,
  Task 5), so untrusting after the stop, as §10 orders it, is fine.

- [ ] **Step 1: Write the failing tests:** `versions_compare_numerically` (`"0.10.0" > "0.9.9"`,
  `"v0.1.0" == "0.1.0"`, `"0.1"` and `"x"` are `InvalidArgument`); `an_older_or_equal_feed_is_up_to_date`
  (`update(0.2.0, 0.1.0)` and `update(0.1.0, 0.1.0)` are `UpToDate`; `update(0.1.0, 0.1.1)` is
  `Available(0.1.1)`); `the_bundle_decides_the_install_kind` (`/Applications/Yard.app/Contents/Helpers/yard`
  → `Bundle`, same with caskroom → `Brew`, `/x/target/release/yard` → `Dev`, `/x/Yard.app/Contents/MacOS/yard` → `Dev`);
  `latest_json_round_trips` (the four fields; an unknown extra field is ignored);
  `the_uninstall_plan_follows_the_facts` (full facts on `Brew` → all of `StopDaemon, Untrust,
  HelperUninstall, RemoveData, RemovePrefs, RemoveBrewCask`; nothing installed on `Dev` with
  `keep_data` → `[RemovePrefs]`); `the_release_url_takes_the_override` (env set → that, trailing `/` trimmed).
- [ ] **Step 2:** `cargo test -p yard-core install` — Expected: FAIL (module missing).
- [ ] **Step 3: Implement** the module as specified; no I/O except `release_url`'s env read.
- [ ] **Step 4:** `cargo test -p yard-core` — Expected: PASS.
- [ ] **Step 5:** Commit `yard-core knows how yard was installed, how versions compare and what uninstall removes`.

### Task 3: `yard self-update`

**Files:**
- Create: `crates/yard-cli/src/commands/update.rs`; Modify: `cli.rs` (`SelfUpdate { check: bool, yes: bool }`), `commands/mod.rs`
- Test: unit tests in `update.rs`; `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 2's `install_kind`, `Version`, `Latest`, `update`, `release_url`.
- Produces (used by Task 9 through `--json`): `yard self-update --check --json` prints
  `{ "installed": "0.1.0", "latest": "0.1.1", "available": true, "kind": "bundle"|"brew"|"dev", "notes_url": "…" }`;
  `yard self-update --yes --json` prints `{ "from", "to", "restarted_yardd": bool, "helper_changed": bool }`.
- In `update.rs`, testable seams:
  ```rust
  fn swap(app: &Path, staged: &Path) -> Result<(), YardError>;  // staged sits beside app
  fn team_id(app: &Path) -> Result<String, YardError>;          // from `codesign -dv` stderr "TeamIdentifier="
  ```
- Flow (§6.1): kind `Dev` → `InvalidArgument` `this yard is a development build`, hint `cargo build
  --release`; `Brew` → `Yard was installed with Homebrew`, hint `brew upgrade --cask yard`, for both
  `--check` and the update (`--check` still prints the versions, with `kind: "brew"`). Fetch
  `<release_url()>/latest.json` with `/usr/bin/curl -fsSL`; `--check` prints and exits 0. Else ask
  `Update Yard {from} to {to}? [y/N]` unless `--yes` (non-interactive without `--yes` refuses with
  hint `yard self-update --yes`); download the zip into a `tempfile` dir; check the
  sha256 (`sha2`); `ditto -x -k` into a staging dir **inside the app's parent directory** (`.Yard.app.new-<pid>`);
  `codesign --verify --deep --strict` the staged app; its `team_id` must equal the running app's;
  `swap`; when `read_agent` of the LaunchAgent says `ProgramArguments[0]` is inside `app`, run
  `launchctl kickstart -k gui/<uid>/dev.more.yard.daemon`, else name that yardd and leave it; read the
  new bundle's helper revision by running `<app>/Contents/Helpers/yard version --json` and compare
  with the installed helper's `status` (through the helper client the CLI already has); a
  difference prints `the new helper needs installing: sudo yard helper install`.
- `swap`: rename `app` → `<app>.old-<pid>`, rename `staged` → `app`, remove the old one; if the
  second rename fails, rename the old one back and return the error.

- [ ] **Step 1: Write the failing tests.** Unit: `swap_replaces_the_app_and_removes_the_old_one`
  (temp dir with `Yard.app/marker=old` and staged `marker=new` → `Yard.app/marker == "new"`, no
  `*.old-*` left); `swap_puts_the_old_app_back_when_the_new_one_cannot_move` (staged path missing →
  error, `Yard.app/marker == "old"`); `team_id_is_read_from_codesign_output` (parse a captured
  `codesign -dv` stderr sample into `"ABCDE12345"`; a sample without the line is an error).
  CLI (`tests/cli.rs`, the test binary is a dev build): `self_update_refuses_a_development_build`
  (exit 1, stderr contains `development build`, hint `cargo build --release`);
  `self_update_check_reads_the_feed` (`YARD_RELEASE_URL=file://<tmp>` holding a `latest.json` →
  `--check --json` prints `"latest":"9.9.9"`; a dev build still answers `--check`, with `"kind":"dev"`).
- [ ] **Step 2:** `cargo test -p yard-cli self_update swap team_id` — Expected: FAIL.
- [ ] **Step 3: Implement** the command as above; `--check` is allowed for every kind, the update only for `Bundle`.
- [ ] **Step 4:** `cargo test --workspace` — Expected: PASS; clippy clean; `yard self-update --help` reads right.
- [ ] **Step 5:** README "Updating" section and `cli-output` rule example if it lists commands. Commit `yard self-update replaces the whole bundle from the latest release`.

### Task 4: `yard start` installs what Yard itself needs

**Files:**
- Modify: `crates/yard-cli/src/commands/lifecycle.rs` (`start`), `cli.rs` (`Start { yes: bool }`)
- Test: unit tests in `lifecycle.rs`; `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Produces: `fn missing_formulas(brew: &Brew, exists: &dyn Fn(&Path) -> bool) -> Vec<&'static str>`
  (`"nginx"` when `brew.nginx_bin()` is absent, `"dnsmasq"` when `brew.dnsmasq_bin()` is absent, in that order).
- Behaviour (§5.4): before writing the agent, `missing_formulas`; none → as today. Some → ask
  `Yard needs {list} from Homebrew. Install now? [y/N]` (`--yes` skips; non-interactive without it
  fails `FormulaMissing`, hint `yard start --yes`); run `<brew> install <list>` with inherited stdio
  and `Brew::job_env()`; a non-zero exit fails with brew's exit code named; then start. `--json`
  implies non-interactive. `YARD_BREW_PREFIX` keeps working, so the installs checklist's fake brew is
  what tests run.
- [ ] **Step 1: Write the failing tests:** `missing_formulas_lists_nginx_then_dnsmasq` (exists = none →
  `["nginx", "dnsmasq"]`; only nginx present → `["dnsmasq"]`); CLI
  `start_without_nginx_asks_for_yes_when_not_interactive` (`YARD_BREW_PREFIX` = an empty temp dir,
  `--root <tmp>`: exit 1, stderr contains `yard start --yes`, nothing written to `Library/LaunchAgents`
  under the temp `HOME`).
- [ ] **Step 2:** run them — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5:** README setup steps (no more manual `brew install nginx dnsmasq`); AGENTS.md "Setting up a Mac" step 3. Commit `yard start installs nginx and dnsmasq when they are missing, after asking`.

### Task 5: `yard uninstall`

**Files:**
- Create: `crates/yard-cli/src/commands/uninstall.rs`; Modify: `cli.rs` (`Uninstall { keep_data: bool, yes: bool }`), `commands/mod.rs`
- Test: unit tests in `uninstall.rs`; `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 2's `install_kind`, `UninstallFacts`, `uninstall_plan`, `UninstallStep`;
  `lifecycle::stop`; `yard_daemon::tls::{trusted, ca_sha256}`; `ctx.security`.
- Produces (Task 9): `yard uninstall` exists; the app only hands it to Terminal.
- Facts: `agent_installed` = the LaunchAgent plist exists; `ca_exists` = `paths.ca_cert()` exists and
  `trusted(security, ca)` is `Some(true)`; `helper_installed` = `/Library/PrivilegedHelperTools/<helper>` exists.
- Before acting: print the plan, one line per step, with `du -sk`-style sizes for `~/.yard` and each
  directory under `<root>/data/` (computed in Rust by walking), and the "never touches" paragraph of
  §10; then require `uninstall` typed back (`Type uninstall to continue: `) unless `--yes`;
  non-interactive without `--yes` refuses, hint `yard uninstall --yes`. With `--root` set (a scratch
  root), `RemoveBundle`/`RemoveBrewCask`/`HelperUninstall` are dropped from the plan and the output says so.
- Steps: `StopDaemon` → `lifecycle::stop`; `Untrust` → `security delete-certificate -Z <ca_sha256> -t <login keychain>` (the code `trust::untrust` uses, factored into a shared fn that needs no yardd);
  `HelperUninstall` → `sudo <this yard> helper uninstall` with inherited stdio; `RemoveData` →
  `remove_dir_all(paths.root())`; `RemovePrefs` → `~/Library/Preferences/dev.more.yard.app.plist`
  and `~/Library/Caches/dev.more.yard.app`; `RemoveBundle` → `remove_dir_all(app)` and the
  `~/.local/bin/yard` link if it points into `app`; `RemoveBrewCask` → `<brew> uninstall --cask yard`.
  Each step prints done/skipped/failed; failures are collected; exit 1 at the end listing each
  leftover with its command. `--json` prints `{ "steps": [{ "step", "outcome", "message" }] }`.
- Then print the brew formulas Yard used (`sites.toml`/`services.toml` read before `RemoveData`) as
  a line the user can run, and the named-tunnel hostnames from `tunnels.toml`.
- [ ] **Step 1: Write the failing tests.** Unit: `the_typed_word_must_be_uninstall` (`"uninstall\n"`
  ok; `"y\n"`, `"UNINSTALL "` trimmed-but-case-sensitive → only exact `uninstall` after trim passes);
  `data_sizes_name_each_data_directory` (temp root with `data/mysql-8.4/a` 2048 bytes and
  `data/redis/b` → two entries with those names). CLI: `uninstall_refuses_without_yes_when_not_interactive`
  (exit 1, hint `yard uninstall --yes`, temp root still there); `uninstall_yes_on_a_scratch_root_removes_it`
  (`--root <tmp>` with a few files, temp `HOME`, `YARD_SECURITY` = the fake: exit 0, root gone,
  `--json` steps contain `remove_data: done`, and no `helper_uninstall` or `remove_bundle` step);
  `uninstall_keep_data_keeps_the_root`.
- [ ] **Step 2:** run them — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** `cargo test --workspace` — Expected: PASS; clippy clean.
- [ ] **Step 5:** README "Uninstalling" is `yard uninstall` (with `--keep-data`). Commit `yard uninstall takes Yard off the Mac, data included unless --keep-data`.

## Part 2 — packaging and releasing

### Task 6: The bundle with its Helpers, and `scripts/release.sh` up to the artifacts

**Files:**
- Modify: `app/scripts/bundle.sh` (`--release <dir>` mode); Create: `scripts/release.sh`, `packaging/yard.rb.tmpl`
- Test: `shellcheck`, a `--adhoc --no-publish` run

**Interfaces:**
- `app/scripts/bundle.sh` unchanged without arguments (dev build, ad hoc). `bundle.sh --release
  <root-target-release-dir>`: also copies `yard`, `yardd`, `yard-helper`, `yard-shim` into
  `Contents/Helpers/`, builds the app without `runtime-shaders` (precompiled), checks `lipo -archs`
  on all five, and does **not** sign (release.sh signs).
- `scripts/release.sh X.Y.Z [--adhoc] [--no-publish]` writes `dist/X.Y.Z/` with the four assets.
  Steps 1–6 of §8.1 here; steps 7–8 (publish, install) come in Task 12 behind `--no-publish` off.
  Preflight per §8.1 step 1; when publishing (not `--no-publish`), also `.release-scrub` exists
  and `scripts/scrub-check.sh` (Task 10) passes; with `--adhoc` the identity, notary and visibility checks are skipped and every output is
  named `…-adhoc` and refused by publishing.
- Version stamping: sets `version = "X.Y.Z"` in `Cargo.toml` and `app/Cargo.toml`
  `[workspace.package]`, then `cargo update -w` in each to refresh the lockfiles.
- `packaging/yard.rb.tmpl`: the cask of §5.3 with `@VERSION@` and `@SHA256@` placeholders, the
  `postflight` kickstart only when `launchctl print gui/<uid>/dev.more.yard.daemon` succeeds and its
  `program` is inside the app; `zap trash: ["~/Library/Preferences/dev.more.yard.app.plist", "~/Library/Caches/dev.more.yard.app"]`.
- [ ] **Step 1:** `shellcheck scripts/release.sh app/scripts/bundle.sh` — Expected: clean.
- [ ] **Step 2:** `scripts/release.sh 0.0.0 --adhoc --no-publish` — Expected: `dist/0.0.0/` holds the
  zip, DMG, `SHA256SUMS` and `latest.json`; `unzip -l` shows `Yard.app/Contents/Helpers/{yard,yardd,yard-helper,yard-shim}`;
  `shasum -a 256 -c SHA256SUMS` passes; the version stamping was reverted by the script (`--adhoc`
  never leaves a bumped version; `git status` clean afterwards).
- [ ] **Step 3:** With the maintainer's approval (it submits to Apple): `scripts/release.sh 0.0.1 --no-publish`
  — Expected: notarization `Accepted` twice, `spctl` accepts the app and DMG, `codesign -dv` on
  `Helpers/yardd` shows `flags=0x10000(runtime)`; the version bump is reverted (no publish, no commit).
- [ ] **Step 4:** AGENTS.md Build & test gains `release.sh`; `bundle.sh` comment updated. Commit `release.sh builds, signs and notarizes the bundle with its helpers`.

### Task 7: `install.sh`

**Files:**
- Create: `install.sh`, `scripts/test-install.sh`
- Test: `scripts/test-install.sh`

**Interfaces:**
- Consumes: Task 6's assets layout; `YARD_RELEASE_URL` (base URL), `YARD_VERSION` (pin), and for
  tests only `YARD_APPS_DIR` (default `/Applications`, or `~/Applications` when that is not
  writable) and `HOME`.
- Behaviour: §5.2 steps 1–7. Step 4 checks: sha256, `codesign --verify --deep --strict`, `spctl -a
  -t exec`; no Team ID. Step 6: `~/.local/bin/yard` replaced when it is a symlink (prints the old
  target), refused when it is a regular file (hint: remove it). A pinned `YARD_VERSION` fetches
  `https://github.com/webteractive/yard/releases/download/vX.Y.Z/latest.json`.
- [ ] **Step 1: Write `scripts/test-install.sh`:** builds a fake release in a temp dir (an unsigned
  `Yard.app` with `Contents/Helpers/yard` as a shell stub, zipped with `ditto`, `latest.json`,
  `SHA256SUMS`) and asserts, each with `PATH` stubs for `uname`/`sw_vers`/`brew` where needed:
  `refuses_intel` (stub `uname -m` → `x86_64`: exit 1, message names arm64); `refuses_without_brew`;
  `refuses_a_cask_install` (stub `brew list --cask yard` succeeds: message names `brew upgrade --cask yard`);
  `refuses_a_bad_checksum` (corrupt `latest.json`'s sha256: exit 1, nothing in `YARD_APPS_DIR`);
  `refuses_an_unsigned_app` (correct sha256, unsigned app: exit 1 at the codesign check, nothing installed).
- [ ] **Step 2:** `scripts/test-install.sh` — Expected: FAIL (no `install.sh`).
- [ ] **Step 3: Write `install.sh`** (POSIX sh, `set -eu`).
- [ ] **Step 4:** `scripts/test-install.sh` and `shellcheck -s sh install.sh scripts/test-install.sh` — Expected: PASS, clean.
  Then against Task 6 Step 3's notarized `dist/0.0.1/` served with `YARD_RELEASE_URL=file://…`,
  `YARD_APPS_DIR=<tmp>`, temp `HOME`: installs, links `<HOME>/.local/bin/yard`, `yard --version` prints `0.0.1`.
- [ ] **Step 5:** README "Installing" with both commands and the after-install steps (§5.4). Commit `install.sh installs the latest signed release`.

## Part 3 — the app

### Task 8: The app finds its bundled CLI first

**Files:**
- Modify: `app/yard-app-model/src/locate.rs`; Test: its tests

- Ruling against M6 §5.8's order: in a bundle that carries `Contents/Helpers/yard`, that one wins
  over "beside the LaunchAgent's yardd" (the setting still wins over both). Otherwise a Mac whose
  agent still points at a dev build would run the dev CLI from the release app. Cost if wrong: the
  maintainer's dev Mac uses the bundled CLI until the setting is set.
- [ ] **Step 1:** test `the_bundled_yard_wins_over_the_agents` (input with both present → bundled; setting set → setting).
- [ ] **Step 2:** `cargo test -p yard-app-model locate` (in `app/`) — Expected: FAIL.
- [ ] **Step 3:** reorder in `locate`; update its doc comment and M6 spec §5.8.
- [ ] **Step 4:** `cargo test --workspace --locked` in `app/` — Expected: PASS.
- [ ] **Step 5:** Commit `the app runs the yard inside its own bundle first`.

### Task 9: Updates and uninstall in the app

**Files:**
- Modify: `app/yard-app/src/platform/settings.rs` (`auto_update`), `settings_window.rs`, `menubar.rs`,
  `overview.rs`, `state.rs` (an `UpdateState`), `env.rs` (the check and the install), `main.rs` (timer)
- Create: `app/yard-app-model/src/update.rs` (decisions)
- Test: `yard-app-model` unit tests; GPUI tests beside the files above with the existing fakes (`testing.rs`)

**Interfaces:**
- Consumes: Task 3's `self-update --check --json` and `--yes --json` outputs; Task 5's command.
- Produces: `Settings::auto_update() -> bool` / `set_auto_update(bool)` (key `AutoUpdate`, default false);
  `yard_app_model::update::{Check, UpdateItem, item(check: &Check) -> Option<UpdateItem>}` where
  `Check` mirrors Task 3's `--check` JSON and `UpdateItem { title: String, action: UpdateAction }`,
  `UpdateAction::{Install, CopyBrewCommand}`; titles `Update to {latest}…` and `Update with Homebrew`
  (copies `brew upgrade --cask yard`); `None` when not available or `kind == "dev"`.
- Behaviour (§6.3): check at launch and every 24 h (a GPUI timer; tests drive it with the fake clock);
  with `auto_update` on and `kind == "bundle"`, install without asking; else the item appears in the
  menu (before Settings…) and on the Overview. Install = a CLI job `self-update --yes` on Activity;
  on success, spawn `/bin/sh -c 'sleep 1; open "<app>"'` and quit. Settings gains the
  **Install updates automatically** switch and **Uninstall Yard…**, which hands `yard uninstall` to
  Terminal through the existing `TerminalLauncher`, then quits.
- [ ] **Step 1: Write the failing tests:** model `the_item_follows_the_check` (available bundle →
  `Install` titled `Update to 0.2.0…`; brew → `CopyBrewCommand`; dev or not available → `None`);
  GPUI `an_available_update_shows_in_the_menu_and_overview`; `auto_update_installs_without_asking`
  (fake CLI records `self-update --yes`); `auto_update_off_only_offers`;
  `uninstall_hands_the_command_to_terminal` (fake terminal launcher records `yard uninstall`).
- [ ] **Step 2:** run them in `app/` — Expected: FAIL.
- [ ] **Step 3: Implement.** Follow `.hydra/rules/app.md` (button descriptions, sheet keys, errors in view).
- [ ] **Step 4:** `cargo fmt --check`, clippy and `cargo test --workspace --locked` in `app/` — Expected: PASS.
  `app/scripts/checklist.sh` gains U1 (the update item appears against a `YARD_RELEASE_URL` feed newer
  than the build) and passes in full.
- [ ] **Step 5:** M6 spec §14.3 points to M7; README mentions the switch. Commit `the app offers updates, installs them when allowed, and can uninstall Yard`.

## Part 4 — going public and the first release

### Task 10: Scrub the tree, license, docs

**Files:**
- Create: `.release-scrub` (git-ignored; `literal==>replacement` lines, `git filter-repo
  --replace-text` syntax, `regex:` lines allowed), `scripts/scrub-check.sh`, `LICENSE`, `AGENTS.local.md` (git-ignored)
- Modify: `.gitignore`, every tracked file `scripts/scrub-check.sh` names, `Cargo.toml` and
  `app/Cargo.toml` (`license = "MIT"`), `README.md`, `AGENTS.md`, main spec §2/§3.1/§9/§11/§12, M6
  spec §14/§18, `.hydra/rules/*`

**Interfaces:**
- `scripts/scrub-check.sh [--history]`: reads the left side of each `.release-scrub` line, greps
  `git ls-files` (or, with `--history`, every blob of `git rev-list --all` and every commit message)
  case-insensitively, prints `file:line` hits without printing the matched pattern's line text
  (it prints the file and line number only), exits 1 on any hit. Used by `release.sh` preflight
  (Task 6) and Task 11.
- `.release-scrub` content is written by the maintainer's session, not committed: the first name,
  surname, login names, email, home-path prefixes, the Macs' names; replacements: decision markers
  → `maintainer`, a person's name in prose → `the maintainer`, possessive → `the maintainer's`,
  login names in code fixtures → `me`, home paths → `/Users/me`. Longest patterns first.
- [ ] **Step 1:** write `scripts/scrub-check.sh` and `.release-scrub`; run it — Expected: FAIL, about 400 hits.
- [ ] **Step 2:** apply the replacements to tracked files (a script reading `.release-scrub`, then
  read each diff hunk for grammar); move AGENTS.md's per-machine subsections into `AGENTS.local.md`
  and leave a pointer; rename "this machine" to "the main dev Mac" in Status.
- [ ] **Step 3:** `scripts/scrub-check.sh` — Expected: PASS. `cargo test --workspace` (root) and in
  `app/` — Expected: PASS (fixtures renamed consistently, snapshots updated with `cargo insta`
  review where they changed).
- [ ] **Step 4:** add `LICENSE` (MIT, copyright line from the maintainer), `license` in both
  workspaces, `cargo deny check` in both — Expected: PASS. Rewrite README for a stranger (§9). Update
  the main spec, the M6 spec and the root-helper rule per §12 (no `SMAppService`, the bundle layout,
  the cask instead of a formula).
- [ ] **Step 5:** Commit `yard is ready to be read by strangers: MIT, no personal names or paths, generic agent notes`.

### Task 11: Rewrite the history, scan it, go public

Every step here is outward-facing or destructive; each needs the maintainer's go-ahead at the time.

- [ ] **Step 1:** with approval, `brew install git-filter-repo`.
- [ ] **Step 2:** archive: `git clone --mirror git@github.com:webteractive/yard.git <scratchpad>/yard-archive.git`
  and `tar czf ~/yard-archive-2026-11-XX.tar.gz` of it; confirm with the maintainer where the archive lives.
- [ ] **Step 3:** in a fresh clone: `git filter-repo --replace-text .release-scrub --replace-message .release-scrub`;
  then rewrite cited hashes in docs from `.git/filter-repo/commit-map` (a short script over `git
  ls-files '*.md'`), committed in the rewritten repo.
- [ ] **Step 4:** `scripts/scrub-check.sh --history` in the rewritten clone — Expected: PASS. `cargo
  test --workspace` there — Expected: PASS. Authors and dates unchanged: `git log --format='%an %ad' | sort -u` equals the archive's.
- [ ] **Step 5:** secrets scan of the rewritten history (`gitleaks detect` if the maintainer approves
  installing it, else `git log -p --all` grepped for key/token/password patterns, read by hand) —
  Expected: no finding, or the maintainer decides each.
- [ ] **Step 6:** with approval, force-push `main` and the tags; re-clone on the second Mac; on this
  Mac reset the working clone to the rewritten `origin/main`.
- [ ] **Step 7:** with approval, `gh repo create webteractive/homebrew-tap --public` with a README; the
  maintainer makes `webteractive/yard` public in GitHub.

### Task 12: The first release

- [ ] **Step 1:** `release.sh` steps 7–8 (§8.1): publish (commit `release X.Y.Z`, tag, push, `gh
  release create` with the four assets, render `packaging/yard.rb.tmpl` into the tap's
  `Casks/yard.rb`, commit and push there), then install with `install.sh` from the public URL and
  check `yard --version`. Its preflight refuses while either repository is private.
- [ ] **Step 2:** `shellcheck scripts/release.sh` clean; with approval, `scripts/release.sh 0.1.0` —
  Expected: release `v0.1.0` with four assets, cask at 0.1.0, this Mac's `yard` → `/Applications/Yard.app/Contents/Helpers/yard`.
- [ ] **Step 3:** with approval, `yard start` from the installed bundle (the agent moves off
  `target/release`), `sudo yard helper install` by the maintainer in a terminal, `yard doctor` all ok,
  `scripts/e2e-m1.sh` and `app/scripts/checklist-real.sh`. On the second Mac: `brew install --cask webteractive/tap/yard` first without `brew
  trust`; whether brew asks for trust decides the README's brew line (§5.3), fixed in the same
  commit; then the same checks.
- [ ] **Step 4:** a `0.1.1` release made only to exercise updates: `yard self-update` on the curl Mac,
  `brew upgrade --cask yard` on the other, and the app's update item; each restarts yardd on the new
  build and asks for no `sudo` (helper revision unchanged).
- [ ] **Step 5:** AGENTS.md Status records M7 done with what was verified; commit with approval.
