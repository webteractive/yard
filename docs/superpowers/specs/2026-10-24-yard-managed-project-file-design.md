# Managed `.yard` files — design spec

Date: 2026-10-24
Status: approved by the maintainer on 2026-10-25 ("write the plan and implement"). It amends `2026-10-22-yard-project-file-design.md` (the
project-file spec) and, through it, the main spec `2026-10-19-yard-design.md`.

Decisions marked **(maintainer)** are the maintainer's, from 2026-10-24. Decisions marked
**(builder's call)** were made while writing this and can be reversed on their own.

## 1. Intent

A site's `.yard` can live in Yard's own data root instead of in the project, so Yard
serves the project the way its `.yard` says without adding a file to the repository.

Why: on a shared project such as `control`, some teammates will not use Yard. They run
Docker. A committed `.yard` means nothing to them and is clutter in their repository.
Today `yard init` always writes `.yard` into the project, and the only alternative is
`sites.toml`, which holds just `php` and `secure` and is driven by commands rather than a
file.

## 2. Decisions

- **The managed file is a `.yard` kept in Yard's data root (maintainer).** It has the same format,
  keys and validation as a project's `.yard` (project-file spec §4): `php`, `docroot`,
  `secure`, `extensions`, `[services]`, and from M4c `node`, plus one key of its own,
`project` (§4).
- **It lives at `<data root>/sites/<name>/.yard`**, keyed by the site name
  (`~/Library/Application Support/Yard/sites/control/.yard`) **(builder's call)**. The maintainer
  asked for `sites/<site>.yard/.yard`. The directory is keyed by the bare site name, not
  `<name>.<tld>`, so switching the TLD (spec §5.5) does not orphan every file. Site names are
  host labels (`sites::host_label`), so they are safe as one path segment.
- **A committed `.yard` always wins (maintainer).** When the project root has its own `.yard`,
  Yard serves from it and ignores the managed file. Yard never deletes or edits the
  ignored file, and `yard doctor` warns about it (§6). If the committed file goes away,
  say after switching to a branch without it, the managed file takes over within about
  2 s, with no command.
- **`yard init` writes the managed file by default; `yard init --local` writes `.yard`
  into the project (maintainer).** This reverses the project-file spec's "`yard init` writes it"
  as far as where it writes. `init` in a project that already has a committed `.yard`
  still writes nothing and applies that file, as today.
- **For a site using a managed file, `yard isolate`, `unisolate`, `secure`, `unsecure` and
  `docroot` edit that file instead of being refused (maintainer).** The file is in Yard's own
  folder, so the commands keep working the way they do for a site without any `.yard`.
  They are still refused with `SITE_HAS_PROJECT_FILE` when the project has a committed
  `.yard` (§4).

## 3. Goals, non-goals, anti-goals

### Goals

1. A site with a managed `.yard` serves, shows the setup
   page and reports needs exactly like a site whose project has a `.yard`.
2. The shims run the managed file's PHP for any directory inside the linked project.
3. `yard init`, run in a fresh clone, leaves `git status` clean.
4. A project with neither file behaves exactly as today.

### Non-goals

- Sharing managed files between machines or teammates. A teammate who uses Yard runs
  `yard init` themselves, or the team commits one with `--local`.
- Per-directory managed files. There is one per site, at the site's root, like the
  committed file's serving rule (project-file spec §4).
- Converting every existing site. A site linked without any `.yard` keeps today's
  `sites.toml` resolution until someone runs `yard init` in it.

### Anti-goals (never)

- Yard never writes into the project except `yard init --local`.
- Yard never edits a committed `.yard`. Only `init` and the commands in §4 edit a
  managed one.
- Yard never lets the managed file override a committed `.yard`, and never merges the two.
- `yard unlink` never deletes a managed file. It is the user's, like an nginx override.

## 4. Where it is read

### Serving

For each linked site, the project file is the first of these that exists:

1. `<project root>/.yard` (committed),
2. `<data root>/sites/<name>/.yard` (managed),

and none means today's `sites.toml` resolution. Everything in the project-file spec §5
then holds for whichever file was found, unchanged:

- it is the source of truth and `sites.toml` holds only `path`;
- missing keys take machine defaults;
- the setup page appears for a parse error or an unmet need;
- `isolate`, `unisolate`, `secure`, `unsecure` and `docroot` are refused with
  `SITE_HAS_PROJECT_FILE` for a committed file. For a managed file they edit it instead
  (below).

### Commands that edit the managed file

For a site whose file in use is its managed `.yard` (maintainer, §2):

| Command               | Edit to the managed file             |
|-----------------------|--------------------------------------|
| `yard isolate <v>`    | sets `php = "<v>"`                   |
| `yard unisolate`      | removes `php`                        |
| `yard secure`         | sets `secure = true`                 |
| `yard unsecure`       | sets `secure = false`                |
| `yard docroot <path>` | sets `docroot = "<path>"`            |
| `yard node use --site` (M4c) | sets `node`                   |

- **How it edits.** Each edit goes through `toml_edit`, so every other key, comment and
  the key order survive. It is done under the same process-wide lock as
  `config::edit::TomlFile` **(builder's call)**: either by extending `TomlFile` to the
  managed path, or by a small sibling that shares its lock.
- **What it refuses.** The edited text must parse as a managed file. A value the parser
  would reject is refused before anything is written, for example a docroot outside the
  project, which gives `INVALID_ARGUMENT`. A managed file that does not parse is never
  edited: the command fails with `PROJECT_FILE_INVALID`, naming the line, so Yard never
  guesses at a broken file.
- **What it never touches.** Writing never goes through a symlink (§4). The commands
  never touch `sites.toml` for such a site, so the file stays the only source of truth.
- **Afterwards.** The site reconciles as for any other edit. The commands' output and
  `--json` are unchanged, except that the human line names the file edited:
  `control now runs PHP 7.3 (set in <path>)`.
- **Committed files are untouched.** With a committed `.yard` the refusal and its hint
  are unchanged; Yard never edits a project's own file.

**What counts as "has its own `.yard`" (builder's call).** The committed file wins as
soon as anything is at `<project root>/.yard` (`symlink_metadata`), as
`refuse_project_file` already decides. A committed file that cannot be used never falls
through to the managed one, because silently serving other settings would hide the
problem. That covers a dangling symlink, a FIFO, a file over 64 KiB, an unreadable file
and one that does not parse: each puts the site on the setup page naming that file, and
that file must be fixed. `read_project_file` today treats a dangling link as absent. It
changes to match this rule, so one precedence function serves reconcile, the doctor,
`init`, the refusals and the shims.

The managed file is read under the same limits as the committed one: at most 64 KiB,
and it must resolve to a regular file. Reading follows a symlink in both places, so a
managed file may be a link into a dotfiles repository. Writing never goes through one
(project-file spec §6), and `init` refuses to write a managed file that is a symlink, as
it does for a committed one **(builder's call)**.

**A managed file names its project (builder's call).** The managed file carries one
extra key, `project = "<absolute path>"`, the directory it was written for. A committed
`.yard` with that key is `PROJECT_FILE_INVALID`. The managed file is used only while
`project` equals the site's `path` in `sites.toml`. Site names come from directory
names, so a leftover `sites/app/.yard` from one project must never silently give its
PHP, docroot and services to the next directory linked as `app`. A managed file whose
`project` differs, or that has no `project` key, is not used: the site resolves as if
it had none, and the doctor reports `managed-file-mismatch:<name>` (§6).

**Proxies and ties (builder's call).** A proxy has no project directory, so a managed
file under a proxy's name is never read; the doctor reports it like an orphan. When two
linked sites share a path, the shim picks the managed file whose `project` matches; if
both do, it takes the site today's isolation lookup takes (the last of the innermost
names in `sites.toml`'s sorted order).

### Shims

The shim order (project-file spec §5) gains one step. A shim run in `cwd` uses:

1. the nearest committed `.yard` in `cwd` or its parents;
2. **the managed `.yard` of the innermost linked site whose path contains `cwd`;**
3. that site's isolation in `sites.toml`;
4. the nearest `.yard-php`;
5. the default.

So a committed `.yard` in a subdirectory, or in a directory above the linked project,
still wins for shims, as any committed `.yard` beats a site's settings today. A managed
file that does not parse is an error naming the file, like a committed one; the shim
needs no daemon to say so. The shim
already reads `sites.toml` to find the innermost site (`shim::resolve_version`), so step 2
needs no daemon. `Source` gains a `Managed(path)` variant, so a shim's errors name the
managed file **(builder's call)**.

### Watching

yardd watches `<data root>/sites/` and each `sites/<name>/` directory, as it watches each
project's directory for `.yard`, and reconciles when a managed file appears, changes or
goes away. A managed file for a name with no linked site is never read for serving.

## 5. `yard init`

```
yard init [NAME] [--local] [--force] [--replace] [--yes] [--json]
```

Steps 2 to 4 of the project-file spec §6 (move the settings, link, confirm installs) are
unchanged. Step 1 picks the file:

| Project has `.yard` | Managed file exists | `yard init`                  | `yard init --local`                 |
|---------------------|---------------------|------------------------------|-------------------------------------|
| yes                 | any                 | applies the committed file   | applies the committed file          |
| no                  | no                  | writes the managed file      | writes `<project>/.yard`            |
| no                  | yes                 | applies the managed file     | moves the managed file into the project |

- **Writing** uses the same detection as today (`project::render`). The header line says
  where the file is and why: `# .yard for control, kept by Yard outside the project.`
  The committed header stays `# .yard: how Yard serves this project. Commit it.`
- **`--force`** rewrites the file `init` would use, whichever that is (`project::rewrite`).
  It never touches a managed file that a committed one hides.
- **Moving with `--local`** writes the managed file's text unchanged into
  `<project>/.yard` (with the committed header swapped in), then deletes the managed file.
  It refuses with `PROJECT_FILE_EXISTS` if `<project>/.yard` already exists. Without the
  delete, the doctor would warn about a hidden file forever **(builder's call)**.
- **`--force --local`** with a managed file and no committed one moves the file as
  above, then rewrites the moved file as `--force` does. This is one path, move then
  rewrite, so `extensions` and `[services]` survive **(builder's call)**.
- **Writing `project`.** Every managed file `init` writes or rewrites gets `project` set
  to the site's `path` exactly as `sites.toml` records it (the canonical directory for a
  new link), so the comparison in §4 cannot fail on a symlinked spelling. Moving with `--local` drops the key.
- **Name taken by another directory.** A managed file for the name whose `project` is
  another directory counts as "this directory already has a `.yard`" in the project-file
  spec §6 step 3: `init` asks `Replace it with this directory? [y/N]`, and `--replace`
  answers yes.
  - On yes, `init` keeps the old file as `sites/<name>/.yard-backup`, as the
    config-ownership rule does for a hand-edited generated file.
  - It then writes a fresh managed file detected for this directory.
  - It never re-points an old file's relative `docroot` at a directory it was not
    written for **(builder's call)**.
- **Two clones** linked as `control` and `control-2` get two files.
- **Printing.** `init` prints the file's path and text, so the user knows where to edit
  it.

`yard link` in a project with no committed `.yard` uses the managed file for its name when
the file's `project` is this directory, saying `using <path>`. So unlinking and
relinking the same project keeps its settings. Like `init`, `link` then removes `php`,
`secure` and `docroot` from the site's `sites.toml` entry. A managed file for another
directory is left alone and named in a note: `sites/app/.yard is for /code/other-app, so
it is not used here`.

## 6. RPC, CLI and doctor

- `sites.init` gains `local: bool` (default `false`). Additive; protocol minor bump.
- `SiteInfo.project_file` stays the path of the file in use. It gains
  `project_file_kind: Option<"project" | "managed">` and `ignored_managed_file:
  Option<{ file, because: "committed" | "other_project", project? }>`: a managed file for
  the site that is not used, because a committed one hides it or its `project` is another
  directory (`project` is what it names, absent when it names none). Both fields are
  additive. `yard link`'s note and the doctor's `managed-file-ignored` and
  `managed-file-mismatch` checks read it.
- `yard info` shows `config  managed (<path>)` or `config  .yard`. Its setting sources
  read `managed` instead of `.yard` for a managed file.
- `yard sites --json` carries the new fields. The table is unchanged.
- New code: `PROJECT_FILE_EXISTS`, only for the `--local` move.
- Doctor:
  - `project-file:<site>` checks whichever file is in use, as today.
  - New, a warning: `managed-file-ignored:<site>`. "control has a committed .yard, so
    `<path>` is ignored." The hint is to delete the managed file if it is no longer
    wanted.
  - New, info only: `managed-file-orphan:<name>`, for a managed file whose name has no
    linked site, or names a proxy. The hint is `yard link <name>` or delete the file
    **(builder's call)**.
  - New, a warning: `managed-file-mismatch:<name>`, for a managed file whose `project`
    is not the site's path. The hint is `yard init --replace` in the site's directory,
    or delete the file.

## 7. What it changes elsewhere

When built, the same change updates:

- **The project-file spec:** §2 ("`yard init` writes it"), §3 anti-goal "Yard never
  writes `.yard` except in `yard init`" (now `init`, `init --local` for the project's
  own file, and the §4 commands for a managed one), §4, §5 resolution, the refusals and
  shims, §6 and §7.
- **The main spec:** the "nothing is written into the project" decision, §4.5 and the data
  root layout.
- **`AGENTS.md`'s "Sites are explicit" decision**, the README, `yard init --help`, and the
  `config-ownership` and `cli-output` rules.

## 8. Milestone

**M4a.1 (builder's call).** It amends M4a's project file and builds on M4b's `[services]`,
so it is planned and built after M4b is merged. It needs its own plan, and a real-machine
check, `scripts/e2e-m4a1.sh`. That script makes a scratch project and runs `yard init` in
it, then checks:

- that `git status` is clean;
- that the site serves the managed file's PHP;
- that `yard isolate` edits the managed file and keeps its comments;
- that a committed `.yard` added later wins, and the doctor warns;
- that `init --local` moves the managed file into the project.

## 9. Questions the maintainer answered (2026-10-24)

1. Why: teammates on Docker should not see a `.yard` in the repository.
2. A committed `.yard` wins over a managed one.
3. `yard init` writes the managed file by default; `--local` writes into the project.
4. For a site with a managed file, `isolate`, `unisolate`, `secure`, `unsecure` and
   `docroot` edit it instead of being refused (answered "Yes" on 2026-10-25 to "edit
   that file, or refuse and point at it").
