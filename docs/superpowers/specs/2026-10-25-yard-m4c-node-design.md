# M4c Node — design spec

Date: 2026-10-25
Status: approved by the maintainer on 2026-10-26. It refines the main spec `2026-10-19-yard-design.md`
§8.1 and the project-file spec `2026-10-22-yard-project-file-design.md` (its `node` key), and
fills the managed-file spec's `yard node use --site` row.

Decisions marked **(maintainer)** are the maintainer's, from 2026-10-25. The maintainer asked Claude to make the rest
("decide for me"); those are marked **(Claude's call)** and can be reversed one by one.
Decisions marked **(builder's call)** are left to whoever plans and builds this.

## 1. Intent

`node`, `npm` and `npx` run the Node version each project asks for, with no nvm in the
shell and no paywall, the way `php` and `composer` already run each project's PHP.

Why: the maintainer's projects span Node 16 to 25. Three laravel-mix projects in `~/Herd` need 16
(`maker` pins `16.14.0`), `campaignbuilder` pins `18.16.0`, `ebv` 21, most pin 22, and two
pin 25. Herd gates its Node manager behind a paywall, and nvm makes every shell slower and
has to be told to switch. Brew ships only current Node lines (`node@18` is disabled, so are
16 and 21), so brew alone cannot serve these projects.

What exists in the projects (a read-only scan on 2026-10-25 of 31 folders in `~/Herd` and
43 in `~/AI` that show Node):

- `.nvmrc` is how projects pin Node. Only one uses `.node-version`, and none uses
  `.tool-versions`, mise, volta or `packageManager`.
- `.nvmrc` holds a major (`22`), an exact version (`16.14.0`, `22.17.1`) or an LTS name
  (`lts/iron`, `lts/*`).
- About a third pin nothing, so the default version decides what most of them run.
- Everything uses npm except one pnpm project (`karaoke`, no `packageManager`).

## 2. Goals, non-goals, anti-goals

Goals:

- `node`, `npm` and `npx` shims that pick the version from the project, with no daemon and no
  shell hook.
- `yard node install | uninstall | list | default | use | unuse`.
- `.yard`'s `node` key, committed or managed.
- `yard link` and `yard init` offer to install the project's Node with what else it needs.

Non-goals (for M4c):

- pnpm, yarn and corepack shims. npm comes with each Node, and pnpm can be installed by the
  user. Corepack can follow later.
- `package.json` `engines.node`. It is a range (`>=18`) rather than a pin, so Yard ignores it.
  (fnm reads it by default, but Yard always gives fnm an explicit version, §4.)
- Node older than 16. No Apple Silicon build of Node exists before 16.0.0, and Intel Macs are
  a non-goal of Yard. fnm's x64 builds under Rosetta could run one; the maintainer tabled that on
  2026-11-01, since no project of his pins older than 16.
- Global npm packages on the shell's PATH. `npm install -g` installs into that version's
  directory. It is reachable from `npm run` scripts, which put that version's `bin` first
  (§6), but not from a bare shell.
- Running Node processes as units, such as a Vite dev server. `yard proxy` already covers
  pointing a site at one.

Anti-goals: anything that would make it a failure even if it works.

- Touching nvm or Herd's nvm in any way: `~/.nvm`, `~/Library/Application Support/Herd/config/nvm`,
  and the user's shell files. Yard never reads them to reuse a version either **(Claude's
  call)**. This mirrors "never Herd's PHP".
- A shim that downloads something. A missing version is an error with a hint (§6).
- A shim that falls through a broken pin to another version (same rule as PHP's shims since
  M4a.1).

## 3. Where Node comes from

- **fnm, from brew's `fnm` formula (main spec §8.1, the maintainer chose fnm on 2026-10-25).** fnm downloads official
  Node builds from nodejs.org. It is a Rust binary and needs no shell hook to install.
- **Yard gives fnm its own directory.** Yard runs fnm with `FNM_DIR=<data root>/node`, so
  installed versions live at `<data root>/node/node-versions/v<x.y.z>/installation/`
  **(Claude's call)**. Yard never uses fnm's default directory. A user who also uses fnm
  directly keeps their own versions apart.
- **No space in the path.** The data root moved to `~/.yard` on 2026-10-25 (maintainer), so
  versions live at `~/.yard/node/node-versions/…`. Native-module builds (node-gyp, old
  node-sass) on Node 16 never meet a quoted path. The e2e still builds one (§11), because
  the laravel-mix projects depend on it.
- **Yard runs fnm only for `install`, `uninstall` and listing what nodejs.org has.** The
  shims never run fnm. They read the installed versions from the directory listing
  (§6), so fnm's layout is a contract that the e2e checks (§11). A version
  directory without `installation/bin/node` counts as not installed.
- **When fnm is missing**, `yard node install` installs it with `brew install fnm`, which is
  in homebrew/core so it needs no trust gate. That runs inside the same job, as `yard php
  install` installs PHP. No other command installs fnm.
- fnm installs run as daemon jobs (main spec §6.2): they are queued, stream progress and can
  be cancelled. fnm runs with `--progress never` and `FNM_LOGLEVEL=info`, so job lines stay
  readable **(builder's call)**.

## 4. Version specs

A **spec** is what a file or command names. A **version** is an installed `x.y.z`.

| Spec | Meaning | Example |
|---|---|---|
| major | the newest installed `<major>.x.y` | `22` |
| major.minor | the newest installed `<major>.<minor>.y` | `22.17` |
| exact | exactly that version | `16.14.0` |
| LTS name | the newest installed release of that LTS line | `lts/iron` (20) |
| `lts/*` | the newest LTS line Yard knows | 24 today |

- **An exact pin means exactly that version (Claude's call, the maintainer asked Claude to decide).**
  Whoever wrote `16.14.0` chose it on purpose, often for an old native module, and teammates
  on nvm run exactly that version. Each extra patch version costs about 100 MB of disk.
- A leading `v` is accepted (`v22.17.1`). Surrounding whitespace is ignored. In `.nvmrc` and
  `.node-version`, the first line that is neither empty nor a `#` comment is the spec; a
  `#` comment after it on the same line and a leading UTF-8 BOM are ignored (2026-10-26).
- **Refused specs:** `node`, `stable`, `latest`, `system`, `iojs`, any range (`>=18`, `^20`),
  and a major below 16. Each is an error that names the file and line, with the hint to write
  a version such as `22`. From a command it is `INVALID_ARGUMENT`; from a `.yard` it is
  `PROJECT_FILE_INVALID`.
- **LTS names are resolved offline.** Yard keeps a map from codename to major at
  `<data root>/node/lts.toml`. It is generated, carries the usual header, and is rewritten
  whenever Yard asks nodejs.org (each install, and `yard node list --available`). Until that
  file exists, a map built into Yard is used (argon 4 … jod 22, krypton 24). `lts/*` means
  the highest major in the map. Yard translates an LTS spec to its major before running fnm,
  so it never relies on fnm's handling of `lts/*`, which fnm's docs do not mention
  **(Claude's call)**.
- **The spec is matched against installed versions, and not re-resolved against
  nodejs.org.** `22` keeps running 22.17.1 until a newer 22 is installed with
  `yard node install 22`. That command installs the newest 22 even when an older one is
  there. The older version stays until `yard node uninstall`.

## 5. Which spec a directory gets

The shims (and `yard info` for a site) walk up from the cwd, and **the nearest pin wins**
(maintainer left it to Claude on 2026-10-26, after the final M4c review; nvm, fnm and mise all let
a nearer file win). In one directory they take the first of these **(Claude's call; the
order follows main spec §8.1 and project-file spec §5)**:

1. **A `.yard` that sets `node`.** A `.yard` without `node` is passed over, and the walk goes
   on.
2. **The site's setting**, in the root of the innermost linked site containing the cwd: either
   - the `node` of its managed `.yard`, when that file is for the site's path (the same
     rules as PHP, managed-file spec §4), or
   - its own `node` in `sites.toml` (set by `yard node use`, §7).

   This step is skipped when the site's project root has a committed `.yard`. A committed
   file always wins, and `yard node use` is refused for such a site, so a `node` left in
   `sites.toml` from before the file was committed is ignored, as a left-over `php` is.
3. **`.nvmrc`**, then **`.node-version`**.

With nothing found up to `/`, it is **the default**: `default_node` in `yard.toml`. When that
is unset, the built-in default is `22` **(Claude's call: most of the maintainer's pinned projects use
22)**.

- **Nearer beats kind.** A monorepo's `frontend/.nvmrc` wins in `frontend/` over the root's
  `.yard` `node` and over `yard node use`, which still beat an `.nvmrc` beside them in the
  root, and the site's setting beats a `.yard` above the site. The first build ranked kinds
  first (any `.yard` before the site's setting before any `.nvmrc`), which silently ran the
  wrong Node in such a subfolder; the only way out was a nested `.yard`, which also moves
  that folder's PHP to the default.

- **A `.yard` with no `node` passes over, and does not mean the default.** This differs from
  PHP, where a `.yard` with no `php` means the default PHP. Almost every project pins Node
  with `.nvmrc` and nothing else, and a `.yard` written for PHP should not override it
  **(Claude's call)**. The same holds for a managed `.yard` with no `node`: step 2 then
  looks at nothing else, and the walk goes on to step 3.
- **Whether a site uses Node.** The shims always resolve a version, since running `node` asks
  for one. A *linked site*, though, uses Node only when its root has a `package.json`, its
  own setting gives it a spec, or a pin file in its root does **(Claude's call)**. A pin
  above the root, such as `~/.nvmrc`, still decides the version of a site that uses Node,
  but alone does not make one (Claude, 2026-10-26: otherwise one `~/.nvmrc` gave every
  PHP-only site install offers and doctor warnings). A PHP-only site has no Node:
  - `link` and `init` offer it no Node install;
  - `yard doctor` gives it no `node:<site>` warning;
  - `yard info` shows no `node` line for it;
  - it does not count as a user of any version for `yard node list` or `NODE_IN_USE`.
- **A file that exists but cannot be used stops the search.** This covers a FIFO, a broken
  symlink, a file too large, or an unreadable spec. The shim then errors and names the file,
  as `read_small` does for PHP. A directory named `.nvmrc` is skipped.
- The resolution lives next to `shim::resolve_version` in `yard-core`, as a pure function
  over the same `read` and `canonical` closures **(builder's call: one walker shared with
  PHP, or a sibling)**.

## 6. The shims

- `bin/node`, `bin/npm` and `bin/npx` are symlinks to the same `yard-shim` binary.
  Reconcile keeps them, as it keeps the PHP shims.
- **What a shim runs.** It resolves the spec (§5), picks the matching installed version
  (§4) and `exec`s
  `<data root>/node/node-versions/v<x.y.z>/installation/bin/<name>` with the arguments
  unchanged.
- **PATH for the child.** It is that version's `bin`, then Yard's `bin`, then the inherited
  PATH without Herd's directories (`shim::child_path`). npm and npx are
  `#!/usr/bin/env node` scripts, so they and every `npm run` script reach the same Node.
- **Local HTTPS.** When `NODE_EXTRA_CA_CERTS` is unset and Yard's CA exists, the shim sets it
  to `ca/yard-ca.pem`. Node then trusts `https://<site>.yard` (Vite, SSR, HTTP clients)
  **(Claude's call)**. A value the user set is never replaced. The doctor advice to set it
  (main spec §5.4) becomes advice for Node that does not come through the shims.
- **A missing version is an error.** The shim exits 1 with:

  ```
  yard: Node 16.14.0 is not installed (/Users/me/Herd/maker/.nvmrc)
    hint: yard node install 16.14.0
  ```

  The error code is `NODE_MISSING`, new. When the spec came from the default, the message
  says `(the default)`. Every `yard node install` hint single-quotes a spec with a glob
  character (`yard node install 'lts/*'`), so it pastes into zsh (added in the final
  review, 2026-10-26). The shim never runs fnm and never downloads anything **(Claude's
  call, the maintainer asked Claude to decide)**.
- `YARD_SHIM_DEBUG=1` prints the version and where its spec came from, as it does for PHP.

## 7. Commands

All of these are daemon RPCs (main spec §6.2: `node.list|install|uninstall|default|use|unuse`).
They go into one new protocol minor version **(builder's call: which number)**.

- **`yard node install [<spec>]`** is a job. With no spec it installs what the cwd resolves
  to (§5), so running it in `maker` installs 16.14.0. It installs fnm first when fnm is
  missing (§3), then the newest release matching the spec, or the exact version. A version
  already installed finishes at once with "already installed".
- **`yard node uninstall <version>`** is a job that runs `fnm uninstall`. It is refused with
  `NODE_IN_USE` before fnm runs when this is the only installed version matching the default,
  or the spec of the root of a linked site that uses Node (§5). The hint is the edit or command that moves it.
  A project that is not linked is not checked. `uninstall` takes an exact version, so
  `uninstall 22` is `INVALID_ARGUMENT` with the hint `yard node list`.
- **`yard node list`** shows each installed version, which one the default resolves to, and
  the linked sites whose root resolves to it. **`yard node list --available [<major>]`** asks
  nodejs.org through `fnm ls-remote` for the newest release of each line still supported
  (≥ 16), marks the LTS lines, and refreshes `lts.toml`.
- **`yard node default <spec>`** sets `default_node` in `yard.toml`. A version matching the
  spec must already be installed, as `yard php default` requires. This splits the main spec's
  `yard node use <ver>` (without `--site`) into its own command, the way PHP separates
  `php default` from `isolate` **(Claude's call)**.
- **`yard node use <spec> [--site <name>]`** works like `yard isolate`. The site is the one
  containing the cwd unless `--site` names it.
  - A site with a committed `.yard` is refused with `SITE_HAS_PROJECT_FILE`, hint
    `edit <path>/.yard` and set `node`.
  - A site with a managed `.yard` gets `node = "<spec>"` written into that file (the
    managed-file spec's table, with its editing rules: toml_edit, the lock, refusing a
    broken file).
  - Any other site gets `node` set in its `sites.toml` entry.
  - The spec does not have to be installed. The command prints the install hint when it
    is not.
- **`yard node unuse [--site <name>]`** removes what `use` set, from the managed file or
  `sites.toml` (as `unisolate` does), and keeps the comments around the key.

## 8. `.yard`'s `node` key, `init` and `link`

- **`node` is a string spec (§4)**, in a committed or a managed `.yard`. This widens the
  project-file spec, which said "major version" **(Claude's call, so `.yard` takes what
  `.nvmrc` takes)**. A bad spec is `PROJECT_FILE_INVALID` at its line. An older yardd still
  treats `node` as an unknown key, so the file is invalid there, as the project-file spec
  already says.
- **A missing Node never puts a site on its setup page.** nginx and PHP-FPM do not need it.
  It is reported separately: `yard info`, `yard doctor`, and `link`/`init` (below).
- **`yard init`**:
  - moves a `node` from `sites.toml` into the file it writes, as it moves `php`;
  - never writes `node` from `.nvmrc`, because the project already pins it there and two
    copies drift;
  - keeps `node` with `--force`, as the project-file spec already says.
- **`yard link` and `yard init` offer to install the site root's Node** when the site uses
  Node (§5) and that version is missing.
  It goes in the same "Install these? [y/N]" list as PHP, extensions and services (the CLI's
  `settle`), installed after PHP and extensions. `--yes` installs it without asking. When
  nothing is interactive and `--yes` is not given, a missing Node alone does not make
  `link` or `init` fail with `NEEDS_INSTALL`. It prints the hint and exits 0, because the
  site is served anyway **(Claude's call)**.
- `SiteInfo` gains the site root's Node: the spec, where it came from (the `.yard`, the
  managed file, `sites.toml`, `.nvmrc`, `.node-version` or the default), the version it
  resolves to, and whether that is installed **(builder's call: the shape, following
  `SettingSource` and its `Other` fallback)**. When the pin cannot be used, it carries the
  error, and its source and file are the pin at fault (2026-10-26).

## 9. `yard info`, `yard doctor` and `install-shims`

- `yard info` adds a `node` row after `php`, in the style of its other settings:
  `node  16.14.0  from .nvmrc`, `node  22.18.0 (22)  from .nvmrc` when the spec is not the
  version, or `node  16.14.0, not installed  from .nvmrc`, with
  `Node 16.14.0  (yard node install 16.14.0)` under `needs`. An unusable pin shows
  `unusable pin`, and its message and hint go in the error lines (changed while building,
  2026-10-26, to match the rows beside it).
- `yard doctor` adds:
  - `node:<site>` (warn): a site that uses Node (§5) whose version is not installed, with
    the install hint;
  - `node-spec:<site>` (warn): its `.nvmrc` or `.node-version` cannot be used;
  - `node-default` (warn): `default_node` matches nothing installed while a site that uses
    Node resolves to the default.
  
  Doctor never runs fnm.
- `yard install-shims` warns about `node` as it does about `php`. If the first `node` on
  PATH is not Yard's, it says whose it is when it can tell: Herd's, nvm's (`/.nvm/`), mise's
  (`/mise/`), fnm's own (`fnm_multishells`) or brew's (`/opt/homebrew/bin`). The fix in every
  case is Yard's `bin` coming first.

## 10. Data root and config ownership

```
node/                        # FNM_DIR: written only by fnm, run by Yard (install/uninstall)
node/lts.toml                # generated: LTS codename → major (§4)
yard.toml   default_node = "22"          # user-owned, set by `yard node default`
sites.toml  [sites.<name>] node = "16"   # user-owned, set by `yard node use` for a site with no .yard
```

- Yard never deletes `node/` and never edits inside it except through fnm.
- `.nvmrc` and `.node-version` are the project's files. Yard only reads them.
- The config-ownership and homebrew hydra rules gain these lines in the same change.

## 11. Testing and the real-machine check

- Unit tests in `yard-core` cover: spec parsing (each row of §4 and each refused form); matching
  against an installed list (the newest patch for a major, an exact pin that is absent, LTS
  through a map); resolution order (§5), including a `.yard` without `node` falling through to
  `.nvmrc`; and an unusable `.nvmrc` stopping the search.
- Shim tests in `yard-shim/tests` use a temp data root, with a fake
  `node/node-versions/v16.14.0/installation/bin/node` script, as the PHP tests fake a keg.
- Daemon tests use a fake `fnm` script on a test `PATH`, as tests fake `brew` and `security`.
  They never download Node.
- **`scripts/e2e-m4c.sh`** checks this on the maintainer's machine after `e2e-m1.sh`. It needs
  `brew install fnm`, so ask the maintainer first. It downloads Node 16.14.0 and the newest 22. In a
  scratch project outside `~/Herd`, it checks that:
  - `.nvmrc` `16.14.0` gives `node -v` = `v16.14.0`, and an `npm run` script that prints
    `node -v` prints the same;
  - `npm install --build-from-source` of a small native module (such as `bufferutil`)
    succeeds under 16.14.0 (§3);
  - `yard node install 22` leaves `jod = 22` in `lts.toml`, read from the real fnm's
    release list (added 2026-10-26);
  - `yard node use 22` (managed `.yard`) wins over the `.nvmrc`;
  - a pin that is not installed gives the error and hint of §6;
  - `yard node uninstall` of a version in use is refused.

  It then uninstalls only the versions it installed, and restores `yard.toml` and
  `sites.toml` byte for byte. It fails if a listing of `~/.nvm/versions/node` or of Herd's
  `config/nvm` changed. `shellcheck` stays clean.

## 12. Docs that move with it

The main spec §3.2 (the `node/` folder), §6.2 (the method list) and §8.1, which this
replaces in detail; the project-file spec's `node` row and §5 Node line; the managed-file
spec's table; `AGENTS.md`'s status and key decisions (a "Never nvm's or Herd's Node" line next
to "Never Herd's PHP"); the README; `--help`; and the cli-output, config-ownership,
homebrew and rpc-protocol rules.

## 13. Left to the builder

- Plan tasks and order, crate placement within the existing boundaries (pure logic in
  `yard-core`, side effects in `yard-daemon`), names of types and functions.
- The protocol version number, and the exact wire shape of `node.*` results and of
  `SiteInfo`'s Node part.
- How fnm's job output is trimmed, and how `--available` is formatted.
- Whether the Node and PHP shims share one directory walker.
