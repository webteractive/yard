# The `.yard` project file — design spec

Date: 2026-10-22
Status: approved by the maintainer on 2026-10-22. It extends `2026-10-19-yard-design.md` (the main
spec). M4a built `php`, `docroot`, `secure`, `extensions` and `yard init`
(`docs/superpowers/plans/2026-10-22-yard-m4a-project-file.md`, §8); `node` and
`[services]` follow in M4c and M4b.

Decisions marked **(maintainer)** are the maintainer's, from 2026-10-22. Decisions marked
**(builder's call)** were made while writing this and can be reversed on their own.

## 1. Intent

A project says what it needs to run in a file it commits, `.yard`, and Yard serves it
that way. A teammate who clones the project and runs `yard init` (or `yard link`) gets
the right PHP, extensions and services without being told.

Why: eepower, an ExpressionEngine site, needed PHP 7.3, and nothing in the repo said so.
The only per-project hint Yard reads today is `.yard-php`, and only the shims read it;
the site itself is served from `sites.toml`, which lives on one machine.

## 2. Decisions

- **When a project has a `.yard`, that file is the source of truth for how Yard serves
  it (maintainer).** Nothing in `sites.toml` overrides it.
- **`yard init` writes it (maintainer).** It is the only time Yard writes into a project, and
  only because the user ran it there.
- **`yard init` also links the project when it is not linked yet (maintainer),** so one command
  takes a fresh clone to served.
- **Yard asks before installing anything a `.yard` needs (maintainer).** It lists what is
  missing (`brew install shivammathur/php/php@7.3`, an extension, a service) and waits
  for a yes, on the first link and again whenever the file asks for something new.
- **Tunnels never come from `.yard` (builder's call, from the tunnels spec).** A cloned
  repo must not be able to make the Mac public; `yard tunnel` stays a command.

## 3. Goals, non-goals, anti-goals

### Goals

1. A `.yard` at a project's root sets that site's PHP version, docroot, HTTPS,
   extensions, and (with M4) Node version and services.
2. `yard init` writes one from what Yard can tell about the project, and links it.
3. Editing `.yard` takes effect within about 2 s, like the TOMLs.
4. A project without `.yard` behaves exactly as today.

### Non-goals

- Per-site php.ini settings. `yard php config` is per version today; a `[php.ini]` table
  can come later.
- The site's name. It stays the directory name or `yard link <name>`, so two clones of
  one repo can sit side by side **(builder's call)**.
- Tunnels (§2), proxies (a proxied site has no project directory to hold the file), and
  anything Herd-related.

### Anti-goals (never)

- Yard never writes `.yard` except in `yard init`, and `init` never overwrites one
  without `--force`.
- Yard never reads `.env` to fill in `.yard`.
- A `.yard` never installs software, opens a port beyond what its services register in
  `ports.toml`, or runs a command, without the confirmation in §2.

## 4. The file

TOML, at the project root (the directory `yard link` was run in), named `.yard`. For
serving, only that one is read; a `.yard` in a subdirectory of a linked project is ignored
(maintainer). The shims, which work without a link, use the nearest one instead (§5). The maintainer
asked for the name; TOML is the builder's call, to match Yard's other files.

```toml
# .yard: how Yard serves this project. Commit it.
php = "7.3"
docroot = "public"
secure = true
extensions = ["redis", "imagick"]

# From M4:
node = "20"

[services]
mysql = "8.0"
redis = true
emailulator = true
```

| Key | Type | Meaning | Today's equivalent |
|---|---|---|---|
| `php` | `"7.0"`–`"8.5"` | the site's PHP version | `yard isolate` |
| `docroot` | path, relative to the project | what nginx serves | the docroot `yard link` detects |
| `secure` | bool | HTTPS | `yard secure` / `unsecure` |
| `extensions` | list of names | must be installed and enabled for the site's PHP version | `yard ext add --php` |
| `node` | major version (M4) | the Node the site's shims use | none yet |
| `[services]` | name → version or `true` (M4) | services that must be running | `yard service add` (M4) |

- Unknown keys are an error naming the file and the key, as in `sites.toml`
  (`deny_unknown_fields`). A file that does not parse puts only that site on the setup page (§5).
- **Extensions are per PHP version, not per site.** `extensions = ["redis"]` means
  "redis must be installed and enabled for 7.3", which every 7.3 site then has. Yard never
  disables an extension because a `.yard` stopped listing it; removing one stays
  `yard ext remove` **(builder's call)**.
- Names take the same forms as `yard ext add`, pinned releases included
  (`"xdebug-2.9.8"`). A name compiled into brew's PHP (`mbstring`, `intl`) counts as
  present. Yard learns which those are from `php -m`, run once per keg version and cached
  under the data root. Reconcile, including one triggered by a `.yard` edit, reads the
  cache, and runs `php -m` only for a keg version it has not seen (a new install or a
  `brew upgrade`), never on every reconcile **(builder's call)**. An extension the user turned off in `yard.toml` (`enabled = false`,
  e.g. `yard xdebug off`) stays off; doctor notes that the file lists it
  **(builder's call)**.
- **Services are shared too.** `mysql = "8.0"` means "the mysql 8.0 instance must run".
  Two projects asking for it share one instance. Yard does not stop a service when a
  `.yard` stops asking for it. Services are added and started when `init` or `link` is
  confirmed; reconcile never restarts one the user stopped with `yard service stop`.
  `true` means M4's default version. `emailulator = true` is met by any emailulator Yard
  can reach, its own LaunchAgent included, and since Yard never installs it, a missing
  one is a need whose hint is emailulator's installer **(builder's calls; M4's design
  may refine them)**.

## 5. How it changes serving

- **Resolution.** For a site with `.yard`, each setting comes from the file, and
  `sites.toml` holds only the link (`path`). A key the file leaves out takes the machine
  default (the default PHP, `secure_by_default`, the detected docroot), never a
  `sites.toml` value, so the file alone decides **(builder's call)**. A site without
  `.yard` keeps today's resolution (main spec §4.5).
- **Commands that contradict it are refused.** `yard isolate`, `unisolate`, `secure`,
  `unsecure`, `docroot` and (M4) `node use --site` on a site with `.yard` fail with
  `SITE_HAS_PROJECT_FILE`, hint `edit <path>/.yard` (and the key to change), whether or
  not the file sets that key **(builder's call)**. Yard never edits the file to make them
  work.
- **Shims.** A shim looks for `.yard` in the cwd and its parents, as it does for
  `.yard-php`, whether or not the project is linked, so `composer install` before
  `yard init` already runs under the project's PHP **(builder's call)**. It uses the
  nearest `.yard`, so in a subdirectory with its own file the shims follow that file while
  serving follows the root's (§4) **(builder's call)**. It uses the file's `php` (and, from M4, `node`); a file with no `php` key means the default.
  `.yard` replaces `.yard-php`: when both exist, `.yard` wins and `yard doctor` warns
  that `.yard-php` is ignored. A directory with only `.yard-php` works as today. A
  `.yard` that does not parse is an error naming the file, like a bad `.yard-php`; one
  whose PHP is not installed is `FORMULA_MISSING`, hint `yard init`.
- **Node (M4).** The order is `.yard`'s `node`, then `.nvmrc` / `.node-version`, then
  the default **(builder's call)**.
- **Watching.** yardd watches each linked project's `.yard` the way it watches the TOMLs,
  and reconciles on a change. Reconcile never installs anything.
- **A site whose `.yard` cannot be met serves a setup page (maintainer).** When the file asks for
  something not installed (a PHP version, an extension, a service), or does not parse,
  that site's nginx config answers every request with `503` and a short page naming what
  is missing and the command that fixes it:

  ```
  eepower needs PHP 7.3.
  Run yard init in /Users/me/Yard/eepower to install it.
  ```

  For a parse error the page names the file, the line and the message, and the fix is to
  edit the file. Only that site is affected: its problem is recorded as the site's state,
  every other site reconciles as usual, and Yard keeps no memory of earlier settings.
  `yard sites` shows `needs setup`, and `yard doctor` the same as a failing check.

## 6. `yard init`

```
yard init [NAME] [--force] [--yes] [--json]
```

Run in a project directory.

1. **Write `.yard`** from what Yard can tell, never from guesses:
   - `php`: when the directory is a linked site with settings in `sites.toml`
     (`yard isolate`), that version. Otherwise `composer.json`'s `require.php`, resolved
     to the highest installed version that satisfies it, else the lowest supported one that
     does. Otherwise it is left out and the site uses the default.
   - `docroot`: a `yard docroot` override carried over, else what `yard link` would
     detect (`public/` today).
   - `secure`: the site's effective setting, whether it comes from `sites.toml` or
     `secure_by_default`, so a teammate gets the same HTTPS **(builder's call)**.
   - `[services]` (from M4b): written commented out, since Yard does not read `.env`
     (maintainer). The user uncomments what the project needs and runs `yard init` again, which
     applies it. M4a writes no `[services]` lines.
   When a `.yard` already exists, `init` writes nothing and goes on with steps 2–4, so
   it is also the command that applies a cloned project's file. `--force` rewrites the
   keys detection produces and keeps the ones it cannot (`extensions`, `node`,
   `[services]`) **(builder's call)**: `php` is written only when detected and `secure`
   only when true, so `--force` never removes either. A `.yard` that is a symlink is
   never written through, so a cloned project cannot point `init --force` at another
   file **(builder's call)**. The file is printed.
2. **Move the site's settings into it.** If the project is already linked, its `php`,
   `secure` and `docroot` leave `sites.toml`, so the file is the only source of truth (§2). Serving does
   not change.
3. **Link it** when it is not linked yet, as `yard link [NAME]` would.
4. **Confirm installs** (§2): list what the file needs that is missing and ask. `--yes`
   answers yes; with no terminal and no `--yes`, it lists them, installs nothing, and exits
   1 with `NEEDS_INSTALL`, hint `yard init --yes` **(builder's call)**.

`yard link` in a project that already has a `.yard` does steps 3 and 4: it links, reads
the file, and asks before installing.

For eepower today, `yard init` writes (the `[services]` lines from M4b):

```toml
# .yard: how Yard serves this project. Commit it.
php = "7.3"
docroot = "public"

# [services]
# mysql = "8.0"
```

## 7. RPC and errors

- `sites.init { path, name?, force? }` returns the written file's text and a `SiteInfo`;
  the CLI does the confirmation and then runs the installs as the existing jobs
  (`php.install`, `ext.add`, from M4 `service.add`).
- `SiteInfo` gains `project_file: Option<String>` (the path) and `needs: Vec<Need>`
  (what the file asks for that is not installed). Additive; protocol minor bump.
- New codes: `SITE_HAS_PROJECT_FILE`, `NEEDS_INSTALL`, and `PROJECT_FILE_INVALID` (parse or unknown key, with the
  file and line in `data`).
- Doctor: `project-file:<site>` (parses, everything it needs is present) and
  `yard-php-ignored:<site>`.

## 8. Milestone

Part of **M4 Node and services (builder's call)**. M4 was split on 2026-10-22: **M4a**
landed `.yard` with `php`, `docroot`, `secure` and `extensions`, the setup page, the
shims following the file, and `yard init`; **M4b** adds `[services]` with the services
themselves, and **M4c** adds `node` with the fnm wrapper. Until then `node` and
`[services]` are unknown keys, so a file that has them is `PROJECT_FILE_INVALID`.

When built, the same change updates the main spec (§4.5 resolution, §5.1 sites, the
"nothing is written into the project" decision, which gains the `yard init` exception,
§6.2–6.3, §12), `AGENTS.md`'s key decisions, the README, and the cli-output and
config-ownership rules.

## 9. Questions the maintainer answered (2026-10-22)

1. A site whose `.yard` cannot be met serves a setup page naming the fix (§5).
2. Only the project root's `.yard` is read (§4).
3. `yard init` leaves `[services]` commented out; rerunning it applies what the user
   uncomments (§6).
