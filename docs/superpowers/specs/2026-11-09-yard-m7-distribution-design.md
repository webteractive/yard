# Yard M7: distribution — design

Status: approved by the maintainer on 2026-11-09. Main spec §11 and §12 (M7), M6 design §14 and §18
(Q1, Q6). Decisions are marked with who made them: **(maintainer)** for what the maintainer said on 2026-11-09,
**(Claude's call)** for what this design decided and the maintainer can still reverse.

## 1. Intent

Yard leaves the two dev Macs. A teammate, and later anyone, installs Yard with one command and
gets a signed, notarized build that keeps itself current. The first release makes the repository
public under MIT.

What a teammate gets is bare: the `yard` CLI with `yardd`, `yard-helper` and the shim, and
Yard.app beside them **(maintainer)**. Nothing else is installed up front. PHP, Node and the service
engines are fetched from brew when a project needs them, as they are today (`yard link --yes`,
`yard php install`, `yard service add`).

The CLI is the required piece; the app is a companion that is always installed with it and never
required **(maintainer)**. Everything works from a terminal for someone who never opens the app.

## 2. Goals

1. One install command, two channels to the same artifact **(maintainer)**:
   - `brew install --cask webteractive/tap/yard` (a tap Yard owns);
   - `curl -fsSL https://raw.githubusercontent.com/webteractive/yard/main/install.sh | sh`.
2. The install puts `yard` on PATH and Yard.app in Applications, together **(maintainer)**.
3. Three update paths **(maintainer)**: `brew upgrade` for a brew install, `yard self-update` for a curl
   install, and the app updating itself when a Settings switch allows it.
4. Every shipped binary is Developer ID signed with the hardened runtime and notarized; the app
   and DMG are stapled (macos-release-signing rule).
5. Releases are built, signed and published from a maintainer's Mac by one script; no CI signs
   anything **(maintainer: "we do the release and signing in here")**. GitHub Releases hosts the files
   **(maintainer)**.
6. The repository goes public at the first release, MIT licensed **(maintainer)**, with the
   maintainer's name, home paths (`/Users/<name>`) and per-machine notes gone from the docs, plans,
   scripts and code, in the tree and in every past version of those files, so the history is
   rewritten **(maintainer)**. Commit author and committer names stay as they are **(maintainer)**.

## 3. Non-goals

- GitHub Actions signing or releasing **(maintainer)**. CI keeps running fmt, clippy and tests.
- Intel Macs (main spec non-goal; arm64 only, `lipo -archs` checked).
- `SMAppService` for the helper or yardd **(Claude's call, §7)**.
- Sparkle, appdater or any update framework inside the app **(Claude's call, §6)**.
- A Homebrew formula for a CLI-only install (main spec §11 had one): the cask is the brew route,
  since the CLI lives inside the app (§4).
- Bundling PHP, Node, nginx, dnsmasq or any engine. Homebrew stays a hard dependency.
- Downgrades. Updates only move forward.
- System notifications, a login item, Mac App Store.

## 4. One bundle holds everything

**(maintainer approved: "Yes")**

```
/Applications/Yard.app/Contents/
  MacOS/Yard                 the app
  Helpers/yard               the CLI
  Helpers/yardd              the daemon
  Helpers/yard-helper        the root helper, copied out by `sudo yard helper install`
  Helpers/yard-shim          the shim bin/<shim> links to
  Info.plist, Resources/AppIcon.icns
```

- `yard` on PATH is a symlink to `Helpers/yard`. `yard` already resolves its own symlink to find
  its siblings (`lifecycle::sibling`), so `yard start` writes a LaunchAgent whose
  `ProgramArguments[0]` is `/Applications/Yard.app/Contents/Helpers/yardd`, and yardd links
  `~/.yard/bin/<shim>` to the `yard-shim` beside it. Both paths are stable across updates, because
  an update replaces the bundle at the same path.
- The CLI, yardd, shim and app always come from one release, so they cannot drift apart in
  protocol. The helper is the exception: it is copied to `/Library/PrivilegedHelperTools/` and so
  only changes on `sudo yard helper install`. `yard doctor` already warns when the helper's version
  differs from yard's (`doctor.rs`, "the helper is X, but yard is Y"); an update that changes the
  helper says so (§6.3).
- The app finds `yard` as it does today (M6 §5.8): the setting, else beside the LaunchAgent's
  yardd (now `Contents/Helpers/`), else `Contents/Helpers/yard`. No change is needed beyond the
  order the M6 spec already allows.
- **Where it lives.** `/Applications/Yard.app` **(Claude's call)**. When `/Applications` is not
  writable by the user (not an admin), `install.sh` installs to `~/Applications/Yard.app` instead
  and says so. The brew cask follows brew's own `appdir`.
- **Dev builds keep working.** `target/release/yard start` rewrites the LaunchAgent to that build's
  yardd, as today; running `/Applications/Yard.app/Contents/Helpers/yard start` (or the PATH
  `yard` once it points there) moves it back. The builder must confirm `yard start` rewrites an
  agent for the same root whose `ProgramArguments[0]` differs, and make it do so if it does not.

## 5. Installing

### 5.1 Release artifacts

A release `vX.Y.Z` on GitHub Releases carries **(Claude's call on names)**:

| Asset | Used by |
|---|---|
| `Yard-X.Y.Z-arm64.zip` | `install.sh`, `yard self-update`, the app's updater (the stapled app, `ditto -c -k --keepParent`) |
| `Yard-X.Y.Z-arm64.dmg` | the cask, and anyone downloading by hand |
| `SHA256SUMS` | checksums of the zip and DMG, written last |
| `latest.json` | the update check: `{ "version", "zip", "sha256", "notes_url" }` |

`latest.json` is fetched as `https://github.com/webteractive/yard/releases/latest/download/latest.json`,
which redirects to the newest release's asset without touching GitHub's rate-limited API.

### 5.2 `install.sh` (repo root)

POSIX `sh`, `set -eu`, idempotent. It:

1. Refuses anything but macOS on arm64 and macOS older than 14 (the app's floor), and says why.
2. Requires Homebrew at `/opt/homebrew` (Yard's hard dependency) and stops with
   `install Homebrew first: https://brew.sh` when it is missing. It installs nothing through brew
   itself; `yard start` does (§5.4).
3. Refuses when a brew cask install of Yard exists (`brew list --cask yard` or
   `$(brew --prefix)/Caskroom/yard`), pointing to `brew upgrade --cask yard`.
4. Downloads `latest.json`, then the zip, into a fresh `mktemp -d`; checks the sha256; unpacks
   with `ditto -x -k`; checks the app with `codesign --verify --deep --strict` and with Gatekeeper
   (`spctl -a -t exec`, which accepts only a notarized Developer ID app). The script carries no
   Team ID or identity (macos-release-signing rule) **(Claude's call)**; `self-update` adds the
   Team ID check, read from the running bundle (§6.1).
5. Stops yardd if it runs from a previous install (`yard stop` via the old bundle), moves the new
   bundle into place (rename within one volume: stage beside the target, then `mv`), and starts
   it again if it was running.
6. Links `~/.local/bin/yard` → `<app>/Contents/Helpers/yard` **(Claude's call: no sudo; brew's
   `bin` belongs to brew)**, and prints the line to add to the shell profile when `~/.local/bin`
   is not on PATH.
7. Prints the next steps (§5.4).

`YARD_VERSION=X.Y.Z` installs that release instead of the latest.

### 5.3 The cask

`webteractive/homebrew-tap`, `Casks/yard.rb` **(Claude's call on the repo name)**: `url` the DMG,
`sha256`, `depends_on arch: :arm64`, `depends_on macos: ">= :sonoma"`, `app "Yard.app"`,
`binary "#{appdir}/Yard.app/Contents/Helpers/yard"`. `auto_updates` is false, because the brew
install never updates itself (§6.2). Brew runs a cask's `uninstall` stanza on
`brew upgrade` too, so the stanza does **not** boot yardd out (every upgrade would leave every site
down). Instead a `postflight` restarts yardd on the new build (`launchctl kickstart -k
gui/<uid>/dev.more.yard.daemon`) when the agent is loaded and runs this bundle's yardd, as
`self-update` does **(Claude's call)**. Uninstalling is `yard uninstall` (§10), which stops yardd first. `zap`
removes the app's preferences only, **never** `~/.yard` (it holds databases).

Homebrew 6 loads a third-party tap's formula or cask only once it is trusted (AGENTS.md
findings). The README's brew route is therefore `brew trust webteractive/tap` then the install,
unless the builder finds that casks are exempt; the builder checks with the real brew and writes
what holds.

### 5.4 After installing

The same for both channels:

1. `yard start`. Today a missing nginx or dnsmasq only fails with a `brew install` hint, and the
   README asks for them by hand. In M7, "installed on demand" **(maintainer)** reaches them too:
   `yard start` lists the brew formulas Yard itself needs that are missing (nginx, dnsmasq; both
   from Homebrew's core, so no trust question), asks once, installs them with brew's progress shown,
   then starts. `--yes` skips the question; a non-interactive run without it fails with the
   `yard start --yes` hint. Onboarding in the app runs the same through its CLI runner
   **(Claude's call)**.
2. `sudo yard helper install` in a terminal. The app's onboarding keeps handing this command to
   Terminal, as in M6.
3. `yard doctor`.

PHP stays on demand: `yard link --yes` installs the PHP a project needs, as today. The
`shivammathur/php` and `shivammathur/extensions` taps are third-party, and Yard never grants brew
trust (AGENTS.md key decisions), so the README carries one prerequisite line before the first link:
`brew tap shivammathur/php && brew tap shivammathur/extensions`, then `brew trust` for both, and
`yard link` refuses with that hint when they are not trusted, as `FORMULA_UNTRUSTED` already does
for MinIO. Nothing in the README is about quarantine.

## 6. Updating

### 6.1 One updater, in the CLI **(Claude's call)**

`yard self-update [--check] [--yes]`:

1. Reads where it runs from: the bundle containing the running `yard` (canonicalized). A `yard`
   not inside a `Yard.app` (a dev build) refuses: `this yard is a development build`, hint
   `cargo build --release`.
2. Refuses a brew install (the bundle path is brew's `appdir` and `Caskroom/yard` exists):
   `Yard was installed with Homebrew`, hint `brew upgrade --cask yard`.
3. Fetches `latest.json`; with `--check`, prints the installed and latest versions and exits
   (`--json` as every command, cli-output rule).
4. Downloads and checks the zip as `install.sh` does (sha256, `codesign --verify --deep --strict`,
   Team ID equal to the running bundle's own, read from its signature at run time, so the Team ID
   is not compiled in).
5. Swaps the bundle: stage the new app beside the old one, rename the old one aside, rename the new
   one into place, remove the old one. A failure after the first rename puts the old one back.
6. Restarts yardd on the new build (`launchctl kickstart -k`) when the LaunchAgent runs this
   bundle's yardd; sites are down for a second or two. A yardd run from elsewhere (a dev build) is
   left alone and named.
7. Says when the new release carries a different helper than the one installed (§8.2), with the
   hint `sudo yard helper install`.

It asks before the swap unless `--yes`, prints what it did, and uses no brew.

### 6.2 Who updates which install **(maintainer; the stand-down is Claude's call)**

| Installed by | Updated by | `yard self-update` and the app |
|---|---|---|
| brew cask | `brew upgrade --cask yard` | refuse, naming `brew upgrade` |
| `install.sh` | `yard self-update`, or the app | update |
| a dev build | rebuilding | refuse |

### 6.3 The app

- A Settings switch, **Install updates automatically**, off by default **(Claude's call, from
  the maintainer's "auto update if set in settings")**.
- The app checks `latest.json` at launch and every 24 hours **(Claude's call)**, whatever the
  switch says, and shows an available update on the Overview and as a menu item
  (**Update to X.Y.Z…**). With the switch on, it installs without asking.
- Installing runs `yard self-update --yes` through the app's CLI runner, as a job with its output
  on Activity, then relaunches the app from the same path. On a brew install the item reads
  **Update with Homebrew** and copies `brew upgrade --cask yard`.
- When an update carries a new helper (§8.2), doctor's existing helper problem shows with the
  install hint; an update that does not touch the helper asks for no password.

## 7. The root helper stays on `sudo yard helper install` **(maintainer; dropping `SMAppService` is Claude's call)**

the maintainer chose the CLI route as the main one. `SMAppService` would register the same label a second
way, from a plist fixed inside the bundle, while today's helper plist carries the owner's uid,
user, data root and brew prefix as install-time arguments (M6 §18 Q1). Two registrations under one
label invite a conflict for one saved click, so M7 drops it: onboarding keeps handing the command
to Terminal. The main spec §3.1, §9 and §11, the M6 spec §14.3 and the root-helper rule change to
say so.

## 8. Signing and releasing

### 8.1 `scripts/release.sh X.Y.Z` **(maintainer: on the Mac; Claude's call on the steps)**

Run by the maintainer on a Mac with the Developer ID Application certificate and the `notary`
keychain profile. An agent runs it only with the maintainer's approval (it commits, tags and
publishes). Steps, in order:

1. **Preflight, before anything irreversible:** on `main`, clean tree, up to date with `origin`;
   `X.Y.Z` is greater than the latest tag and not tagged; the single valid Developer ID
   Application identity resolves (by SHA-1, env override `YARD_SIGN_IDENTITY`; fail on zero or
   several); `xcrun notarytool history --keychain-profile notary` succeeds; `gh auth status` works;
   the tap repo is reachable; both workspaces' fmt, clippy and tests pass.
2. **Version:** sets `X.Y.Z` in both workspaces' `[workspace.package] version` and their
   lockfiles; `yard --version`, `yardd --version`, the helper and the app's plist all report it.
3. **Build:** `cargo build --release --locked` at the root (yard, yardd, yard-helper, yard-shim;
   not yard-fake-unit) and `app/scripts/bundle.sh --release`, which builds the app with
   precompiled shaders and copies the four binaries into `Contents/Helpers/`. `lipo -archs` on
   all five executables must print `arm64`.
4. **Sign inside out:** each `Helpers/*` binary, then `MacOS/Yard`, then the app, with
   `codesign --force --timestamp --options runtime --sign <hash>`, no entitlements (M6 §14.1:
   no exception is needed). `codesign --verify --deep --strict`.
5. **Notarize and staple the app** (zip with `ditto`, `notarytool submit --wait --output-format
   json`, fail unless `Accepted`, printing the log), then build the DMG from the stapled app, sign
   it (`--timestamp`, no runtime), notarize and staple it. `spctl -a -vvv -t exec` on the app and
   `-t open --context context:primary-signature` on the DMG.
6. **Zip the stapled app** for updates, then write `SHA256SUMS` and `latest.json` last.
7. **Publish:** commit `release X.Y.Z` (commit-messages rule), tag `vX.Y.Z`, push, `gh release
   create` with the four assets and notes from the commits since the last tag; update
   `Casks/yard.rb` (version, sha256) in the tap repo, commit and push.
8. **Install what was published** with `install.sh` on this Mac and check `yard --version`
   (release-install rule), then remove `dist/` and staging (releases rule).

`scripts/release.sh --adhoc` builds the same bundle sealed ad hoc, for a Mac without the
certificate, labelled not for shipping, and publishes nothing. The current `bundle.sh` (ad hoc,
no Helpers) stays the dev build.

### 8.2 Version

The first public release is `0.1.0` **(Claude's call)**, the version both workspaces carry today.
Every part (CLI, daemon, shim, helper and app) is bumped to the release's version on every
release, whether it changed or not **(maintainer)**. Protocol versions stay separate (rpc-protocol
rule).

So that a release whose helper did not change asks for no password, doctor stops treating a
version difference as a problem **(Claude's call)**. The helper gains a `HELPER_REVISION`
constant, bumped only when its code, verbs or plist change, which `helper.version` reports beside
the version; `yard` carries the revision it was built with. Doctor's helper check fails only on a
revision difference (hint `sudo yard helper install`) and shows a version-only difference as
information. Today it compares versions, so every release would ask for `sudo`.

## 9. Going public **(maintainer: MIT, at the first release)**

Before the first `release.sh` publishes:

- `LICENSE` (MIT, copyright line chosen by the maintainer), `license = "MIT"` in both workspaces'
  `[workspace.package]`, and `cargo deny check` passing in both.
- **No name, no home paths:** the maintainer's first name, surname, login names, email and home
  paths appear in no tracked file. (Commit metadata is not a file and keeps the author as is.) Today that is about 400 mentions of the first name: AGENTS.md,
  the specs, the plans, the hydra rules, five scripts (the e2e scripts and
  `app/scripts/checklist-real.sh`) and test fixtures in `yard-cli`, `yard-core`, `yard-helper` and
  `yard-daemon`'s tests, which use the login name as a sample user. Decision markers naming the
  maintainer become "(maintainer, <date>)", a Mac named after the maintainer becomes "the main dev
  Mac", and test fixtures use a neutral user (`me`, as other fixtures already do). The full name
  and a home path are in `yard-core`'s render tests and in five docs. The patterns live in a
  git-ignored file (`.release-scrub`, one pattern per line) so the check itself does not publish
  them; `release.sh` preflight greps tracked files for them and fails on a hit. Generic fixture
  paths (`/Users/me`, `/Users/x`) stay.
- **History rewritten:** every past version of every file gets the same replacements as the
  tree, so no old commit's files show the name or a path. Authors, committers and dates are kept
  **(maintainer)**; commit messages are rewritten only where they contain a pattern. The private repository as
  it stands is kept as an archive first (a bare mirror clone, and the original kept private on
  GitHub under another name or as a tarball) **(Claude's call)**. The rewrite uses `git
  filter-repo --replace-text` (and `--replace-message`) from the same pattern file; `git
  filter-repo` is a brew formula, installed only with the maintainer's approval; the
  rewritten repository is checked with the same grep over every blob of every commit
  (`git log -p --all`) before it is pushed. Pushing it is a force push of `main` and the only
  branch, done once, with the maintainer's go-ahead, and any other clone (the second Mac's)
  re-clones afterwards. Commit hashes cited in docs (AGENTS.md cites several) change; a map from
  `filter-repo`'s `commit-map` rewrites them in the same pass.
- **The signature names the certificate holder.** Releases are signed with the Developer ID
  Application certificate on the maintainer's dev Mac **(maintainer)**, whose holder's name shows
  in `codesign -dv` and Gatekeeper's dialogs; that is accepted.
- **AGENTS.md generic:** the "Machines" subsections for the two Macs (computer names, what is
  installed, paths) move out of tracked files into a git-ignored `AGENTS.local.md` that AGENTS.md
  points to; "this machine" in Status becomes "the maintainer's main Mac". Findings that hold on
  any Mac stay. The same pass covers the hydra rules and the specs where they name a Mac.
- README rewritten for a stranger: what Yard is, the two install commands, the three steps after,
  updating, uninstalling (§10), building from source.
- A secrets scan of the full history before the repository is made public; any hit stops the
  release until the maintainer decides **(Claude's call; which scanner needs the maintainer's approval
  if it must be installed)**.
- Making the repository public is the maintainer's own click in GitHub, after the scan.

**Order (Claude's call).** There is no private team phase: the first release is public
**(maintainer: open source when releasing starts)**, so no download needs a token. The order is:
scrub the tree and commit; archive the private repository; rewrite the history and check it;
force-push; run the secrets scan; create the tap repository (public); make `webteractive/yard`
public; then the first `release.sh`. Its preflight refuses while either repository is private
(`gh repo view --json visibility`), since `install.sh`, `latest.json` and the cask would all 404.

## 10. Uninstalling: `yard uninstall` **(maintainer: removes Yard completely by default)**

`yard uninstall [--keep-data] [--yes]` takes Yard off the Mac. By default it removes everything
Yard put there; `--keep-data` keeps `~/.yard` for a reinstall **(Claude's call)**.

**Before anything changes,** it prints what it will remove, the sizes of the data directories
(`~/.yard/data/*`, so the databases are named) and what it leaves, and asks for `uninstall` typed
back **(Claude's call: it deletes databases)**. `--yes` skips the question; without a tty and
without `--yes` it refuses.

**In order** (each step says what it did; a failed step is reported and the rest still run, and
the command exits non-zero naming what is left with the command that removes it):

1. Stops yardd and everything it supervises (`yard stop`, which also removes the LaunchAgent).
2. Removes the local CA from the login keychain when it is trusted (`yard untrust`; macOS asks for
   the password).
3. Uninstalls the root helper (`sudo yard helper uninstall`, run in the same terminal; sudo asks
   for the password), which removes the helper, its LaunchDaemon and the `/etc/resolver` files Yard
   wrote. Herd's resolver is never touched (herd-coexistence rule).
4. Removes `~/.yard`: sites, overrides, generated config, logs, service data, Node versions, the
   CA, the shims (unless `--keep-data`).
5. Removes the app's preferences (`~/Library/Preferences/dev.more.yard.app.plist`) and any
   caches it wrote.
6. Removes the bundle and the `yard` link: on a brew install by running `brew uninstall --cask
   yard`; on a curl install by deleting `Yard.app` and `~/.local/bin/yard`. The running `yard` is
   inside that bundle, so this step is last; deleting an executing binary is safe on macOS.

**What it never touches:** brew and everything installed through it (PHP, extensions, nginx,
dnsmasq, fnm, the engines; Yard never runs `brew uninstall` for a formula, key decisions), brew
trust, the projects and their committed `.yard` files, Herd, DBngin, Postgres.app, emailulator,
and DNS records of named tunnels at Cloudflare (it names the hostnames it knows from
`tunnels.toml` so they can be deleted there). The closing lines say so and list the brew formulas
Yard used, for anyone who wants them gone too.

**From the app:** Settings gains **Uninstall Yard…**, which hands `yard uninstall` to Terminal as
onboarding hands the helper install (it needs sudo and a typed confirmation), then quits.

**Dev builds:** `yard uninstall` from a `target/release` build does steps 1–5 and leaves the
build's files alone.

The README's uninstall section is that one command.
## 11. Testing

- `self-update`'s decisions (dev build, brew install, version compare, Team ID mismatch, checksum
  mismatch, swap and rollback) are pure functions over facts, unit tested in the CLI; the swap
  is tested on temp directories with fake bundles.
- `yard uninstall` plans its steps from facts (install kind, CA trusted, helper installed,
  `--keep-data`) as a pure function, unit tested; the removal runs against a temp `HOME` and data
  root with the fake `security`, launchctl and helper stand-ins the tests already use. It is never
  run against the real data root without the maintainer's approval.
- `install.sh` and `release.sh` stay `shellcheck` clean. `install.sh` takes `YARD_RELEASE_URL` (a
  base URL) so a test serves a local fake release over `file://` or a local HTTP server into a
  temp `HOME` and temp applications directory.
- The app's update item and Settings switch get GPUI tests against the existing fakes.
- The release itself is checked by step 8 of `release.sh`: install the published artifact and run
  it. A first release goes to the maintainer's two Macs, each switching its LaunchAgent from
  `target/release` to the installed bundle, then `yard doctor`, `scripts/e2e-m1.sh` and the app's
  `checklist-real.sh` (each with the maintainer's approval as today).

## 12. What changes in other docs

Main spec §2 (Goal 3 detail), §3.1 (helper registration), §9 (the bundle's Helpers, no CLI
symlink step in onboarding), §11 (this design), §12 (M7 line); M6 spec §14 (points here) and
§18 Q1/Q6 (resolved); root-helper rule; AGENTS.md (Status, Machines moved out, Build & test gains
`release.sh`, `install.sh`); README.

## 13. Left to the builder

Exact asset names and `latest.json` fields, how the app shows update progress beyond the job card,
the cask's `uninstall` details, how `release.sh` writes notes, the tap repo's README, and how the
`/Users/` check is wired (test or preflight).

## 14. Open for the maintainer

Nothing. Commit identities stay as they are, and the certificate on the dev Mac signs
**(maintainer, 2026-11-09)**.
