# Yard runtimes: a contract, recipes, Composer per project and PHP builds — design

Status: **approved by the maintainer on 2026-11-18.** A builder read it cold the same day; what it found
is folded in (Composer's index lists only channel heads, the `php-fpm-<build>` unit name, Node's
managed-file and site-use rules, doctor's levels, the 8.5 variants' link step), and its two
questions for the maintainer are answered in §20. It builds on the main spec's PHP sections
(`docs/superpowers/specs/2026-10-19-yard-design.md` §4), M4c Node
(`docs/superpowers/specs/2026-10-25-yard-m4c-node-design.md`), the `.yard` project file
(`docs/superpowers/specs/2026-10-22-yard-project-file-design.md`) and its managed form, and the
service recipes (`docs/superpowers/specs/2026-11-16-yard-service-recipes-design.md`), whose recipe
format and `download`/`path` sources it reuses. It changes one key decision ("PHP comes from
Homebrew": brew stays the source of the plain builds, §10) and the main spec's `composer` shim
(§4.5: it no longer runs brew's `composer`, §9).

**Plan 1 (the contract and Composer, §17) is built on main (2026-11-19),** planned in
`docs/superpowers/plans/2026-11-18-yard-runtimes-1-contract-composer.md`, with protocol 2.14. What
the build decided is in §21. `scripts/e2e-composer.sh` has not run on a real Mac yet.

Decisions marked **(maintainer)** are the maintainer's, from the 2026-11-18 conversation. Decisions
marked **(my call)** were made while writing this and can be reversed on their own.

## 1. Intent

The service recipes design named three kinds of pluggable thing: services (done), runtimes
(per-project versioned tools behind shims: PHP, Composer, Node) and the server (later). This is the
runtimes design.

Two problems make it the next one:

- **Composer.** The `composer` shim runs brew's single `composer` (2.11.2) under whatever PHP the
  project picked. Composer 2.3 and later need PHP 7.2.5, so `composer install` cannot run in a PHP
  7.0 or 7.1 project; those need the 2.2 LTS line. Brew's formula also pulls in brew's own `php`.
  On a Mac without it (the Mac mini today) every `composer` fails with `brew install composer`.
- **PHP is one build per minor version.** The maintainer wants ZTS builds (FrankenPHP, tabled, needs
  one), debug builds (chasing crashes), builds with his own compile flags or extensions, and a way
  to keep running an old PHP once brew's tap stops carrying it **(maintainer)**. Today a PHP is
  named only `7.0`–`8.5` and always comes from `shivammathur/php/php@<v>`.

He asked for the services' shape **(maintainer: "doing this like the service will prolly be
good")**: a contract in Rust, with every runtime defined by a recipe **(maintainer: "all will follow
runtime recipes for its definition")**, so a new runtime or a new PHP build is a file, not a release.

## 2. Decisions

- **A `Runtime` contract in Rust, and a recipe for every runtime (maintainer: A and B).** Every
  runtime, PHP, Composer and Node included, is defined by a recipe: its shims, its pin files, its
  `.yard` key, its sources and its default. The recipe is data; what a recipe cannot say lives in a
  **driver**, Rust code a recipe names. The set of drivers is closed and only Yard's carried recipes
  may name one (§4).
- **Three carried runtimes this round:** `composer` (a plain recipe, no driver: §9), `node` (the
  `fnm` source, Node's spec rules: §11) and `php` (the `php` driver: FPM pools, Yard's ini, the
  conf.d mirror, the CA bundle, isolation, extensions: §10).
- **User runtime recipes** in `~/.yard/runtimes/<key>.toml` add a runtime (Bun, Deno, Python, Go…)
  with no release **(maintainer)**. Yard never fetches a recipe from the network.
- **PHP builds (maintainer).** A PHP is named by a **build name**: `8.4` (brew's plain build, as
  today), `8.4-zts`, `8.4-debug`, `8.4-debug-zts` (shivammathur's variants, carried), or
  `8.4-<label>` (the user's own build from a `path`, one file in `~/.yard/runtimes/php/`). PHP
  builds Yard installs itself, downloaded or compiled from source, are a separate design,
  `docs/superpowers/specs/2026-11-18-yard-php-builds-installed-design.md` (parked) **(maintainer,
  2026-11-18: "add a separate spec for it so we can focus on it later")**; a `path` build covers an
  old PHP brew no longer carries until then **(maintainer: "these are good")**.
- **Composer's hash may come from getcomposer.org (maintainer, 2026-11-18).** The carried Composer
  recipe takes each phar's SHA-256 from getcomposer.org over HTTPS (§6.2); a user recipe still
  needs its hashes in the file.
- **Composer per project (maintainer: Composer first).** Yard downloads Composer's phars itself,
  each checked by SHA-256, and a project with no pin runs the newest Composer line its PHP can run:
  2.2 under PHP below 7.2.5, 2 otherwise (§9).
- **The shims never download and never install (unchanged, main spec §4.5, M4c §6).** A missing
  version is an error with the install command as its hint.
- **Three plans, one per row of §17, each shipping working software.**

## 3. Goals, non-goals, anti-goals

Goals:

- `composer install` works in every project from PHP 7.0 to 8.5 on a Mac with no brew `composer`.
- A site can run on a ZTS or debug PHP, or on the user's own build, with the same pools, ini,
  overrides, CA bundle, sleep/wake and doctor as a brew PHP.
- A new runtime is one TOML file; `yard runtimes`, the shims, `.yard`, `yard info` and doctor see
  it within about 2 s, with no restart.
- Nothing changes for a project that pins only `php` and `node` today: same versions, same paths,
  same commands.

Non-goals:

- PHP builds Yard downloads or compiles (§10.6, their own spec), FrankenPHP (tabled), PHP below 7.0, Intel Macs.
- Brew as a source for user runtime recipes (§6). PHP and Node keep their own sources.
- Recipes from a registry, URL or git repo.
- Version ranges (`^2.2`, `>=1.1`) as pins, as for Node.
- `yard ext add <brew formula>` for a build that is not brew's plain one (§10.4).

Anti-goals (a failure even if it works):

- A shim that downloads, installs or runs fnm.
- A shim or recipe that can run Herd's, nvm's, pyenv's, mise's or any other manager's binaries, or
  a program found on PATH.
- A broken user recipe that stops `php`, `composer`, `node` or a site from working.
- Composer 2.3+ run under PHP 7.0/7.1, or Composer 2.2 silently run under PHP 8.x because it was
  the only one installed (§9.2).
- `composer self-update` rewriting a phar Yard installed.
- A site on a build named `8.4-zts` whose pool, socket, ini or `conf.d` collides with `8.4`'s.

## 4. The contract

`yard-core/src/runtimes/` holds the trait, the registry, the recipe loader and the drivers, the way
`services/engines/` does for services. The trait's exact shape is the builder's; it must carry:

| Fact | Composer today | Node today | PHP today | On the contract |
|---|---|---|---|---|
| key, display name | — | `node` | `php` | `key()`, `display()` |
| shims | `composer` (a PHP shim) | `NODE_SHIMS` | `PHP_SHIMS` minus composer | `shims()`: each name and what it execs |
| pin in `.yard` | — | top-level `node` | top-level `php` | `yard_key()`: `php`, `node`, else `[runtimes].<key>` |
| pin files | — | `.nvmrc`, `.node-version` | `.yard-php` | `pin_files()` |
| spec syntax | — | major, major.minor, exact, LTS names | build name | the driver's or the recipe engine's (§7.1) |
| installed versions | brew's `bin/composer` | `node/node-versions/` | brew kegs | `installed(paths)`: read from disk, never a daemon |
| install / uninstall | `brew install composer` | fnm, as a job | brew, as a job | `install(spec)`, `uninstall(version)`: jobs |
| default | — | `default_node`, else 22 | `default_php`, else 8.4 | `default()`: rules, can depend on another runtime (§9.2) |
| runs on | PHP | — | — | `runs_on()`: `php` or `node` |
| site uses it | — | `package.json` in the root | always | `uses_when()`: files in the site root |
| wakes services | yes | no | yes (`php`) | `wake()` |
| doctor | — | `node:<site>` | many | `doctor(ctx)` |

- **Drivers** are `php` (§10) and the `fnm` source (§11). A recipe names a driver with `driver =
  "php"` or `kind = "fnm"`; only a carried recipe may, and a user recipe that does fails to load
  (`RUNTIME_RECIPE_INVALID`) **(maintainer: closed set, so "driver" never becomes a plugin system)**.
- The `php` and `node` keys cannot be shadowed by a user recipe (they need a driver). `composer` can:
  a user `~/.yard/runtimes/composer.toml` replaces the carried one, as a user engine replaces a
  carried engine **(my call)**.
- No code outside `runtimes/` matches on a runtime's key, except the PHP-specific code the `php`
  driver already is (pools, ini, extensions).

## 5. Runtime recipes

One file per runtime, `~/.yard/runtimes/<key>.toml` (the stem is the key), loaded with
`deny_unknown_fields`. The carried ones live in `crates/yard-core/runtimes/*.toml`, compiled in with
`include_str!`, as the carried engines are. An example with every field:

```toml
schema = 1
key = "bun"                         # [a-z][a-z0-9-]*; not php or node
name = "Bun"
description = "JavaScript runtime and package manager"

pin_files = [".bun-version"]        # nearest wins (§7); first non-comment line is the spec
uses_when = ["bun.lockb", "bun.lock"]   # files in a site root that mean the site uses it
# runs_on = "php"                   # composer: the shim execs it under the project's PHP
# wake = true                       # ask wake.sock for the site's sleeping services first

path = ["bin"]                      # dirs under {home} put first on the child's PATH; default ["bin"]
env = { BUN_INSTALL_CACHE_DIR = "{cache}" }

[[default]]                         # first matching rule wins; no rules: newest installed
spec = "1"
# php_below = "7.2.5"               # only with runs_on = "php" (composer, §9.2)
# exclude = ["2.2"]                 # versions with these prefixes never match this rule's spec

# [[requires]]                      # only with runs_on: versions from `from` on need this PHP
# from = "2.3.0"
# php = "7.2.5"

[shims.bun]                         # one table per shim name
exec = ["{home}/bin/bun"]           # default: ["{home}/bin/<name>"]
[shims.bunx]
exec = ["{home}/bin/bun", "x"]
# refuse = [{ args = ["upgrade"], hint = "yard runtime install bun <version>" }]

[source]
kind = "download"
url = "https://github.com/oven-sh/bun/releases/download/bun-v{version}/bun-darwin-aarch64.zip"
strip = 1
# file = "composer.phar"            # the name a bare (non-archive) download is saved as

[versions."1.2.4"]                  # pinned: the hash is in the recipe
sha256 = "…"
# [versions."3.12"] path = "/Library/Frameworks/Python.framework/Versions/3.12"   # a path version

# and/or an index (§6.2):
# [source.index]
# kind = "composer"                 # closed set: composer (this round)
# url = "https://getcomposer.org/versions"
# sha256_url = "https://getcomposer.org/download/{version}/composer.phar.sha256sum"
```

### 5.1 Placeholders

`{home}` (the version's install root, §6), `{version}`, `{cache}` (`~/.yard/cache/<key>`, made on
first use), `{php}` (with `runs_on = "php"`: the resolved PHP binary), `{node}` (with `runs_on =
"node"`), `{root}` (the data root). Filled as whole values, never re-parsed, as in engine recipes.
An unknown placeholder or one with no value there fails the recipe at load, with its line.

### 5.2 Load-time checks

A recipe loads only if: it parses; `schema` is known; the stem is the key; the key is valid and not
`php` or `node`; every shim name is `[a-z0-9][a-z0-9._+-]*`, is not one of the carried runtimes'
shims (except those of the carried runtime the recipe replaces: a user `composer.toml` owns
`composer`), `yard`, `yardd` or `yard-shim`, and is not another loaded recipe's (the second to load, in
file-name order, fails, naming the first); every `exec[0]` is under `{home}` or is `{php}`/`{node}`;
`download` URLs are `https://`; each `[versions]` entry has a 64-hex `sha256` or an absolute `path`;
an index appears only in a carried recipe and names a known kind and an `https://` `sha256_url`; `[[requires]]` and `php_below` appear
only with `runs_on`. Whether a program exists is the install's job, not a load check.

### 5.3 Loading, watching, a broken recipe

As engine recipes (recipes spec §7): yardd loads the carried recipes, then the user's, at start and
when `~/.yard/runtimes/` changes, and keeps the last good version of a recipe that turns broken. A
broken user recipe is reported by `yard status`, `yard runtimes`, doctor (`runtime:<key>`) and
Yard.app with its file and line.

**The shims read recipes themselves (my call).** A shim invoked as a name that is not a carried
shim loads `~/.yard/runtimes/*.toml` to find the recipe owning that name; the composer shim loads
`composer.toml` when present. A user recipe that fails to load is skipped with one line on stderr,
falling back to the carried recipe where there is one (composer) and otherwise exiting 1 naming the
file. The shims never read engine recipes, so this does not reopen the recipes spec's rule that
`[services]` keys a Mac does not know never stop `php` or `composer`.

**Shims on disk.** Reconcile keeps `bin/<name>` a link to `yard-shim` for every shim of every
loaded runtime, and removes links it made for a runtime whose recipe is gone (it records them in
`state/shims.json`); any other file there is left alone and reported, as today.

## 6. Sources and installs

| Source | Who may use it | Install root | Install does |
|---|---|---|---|
| `download` | any recipe (not PHP builds, §10.6) | `~/.yard/installs/<key>/<version>/` | the engine recipes' download: curl into a stage, SHA-256, unpack (or save as `file`), rename into place |
| `path` | any recipe (per version); PHP builds | the path itself | checks it exists and is allowed; installs nothing |
| `fnm` | `node` only | `node/node-versions/v<x>/installation/` | fnm, as M4c |
| `brew` | `php` only | `<prefix>/opt/<formula>` | brew, behind brew's trust gate, as today |

- `installs/` is a new top-level directory beside `programs/` (services) **(my call: a runtime key
  and an engine key may be the same word)**. Each version holds `.yard-install.toml` (generated):
  the version, the URL, the SHA-256 it was checked against, `requires`, and when.
- A `path` is refused under Herd's, DBngin's or Postgres.app's folders (`PATH_NOT_ALLOWED`, the
  recipes spec's list), and under `~/.nvm`, `~/.pyenv`, `~/.local/share/mise`, `~/.asdf` and fnm's
  own default directory **(my call: other managers' installs are theirs, as nvm's are for Node)**.
- Uninstall deletes `installs/<key>/<version>/` (never a `path`), refused while it is the only
  installed version matching the default or a linked site's pin, as `NODE_IN_USE` is today
  (`RUNTIME_IN_USE`, naming the site and the edit that moves it).

### 6.1 Versions and specs

A spec is a major (`2`), major.minor (`2.2`) or exact (`2.2.32`) version; it matches the newest
installed version with that prefix (less any a `[[default]]` rule's `exclude` names, when the spec
came from that rule), never re-resolved against the network (Node's rule, M4c §4). A
leading `v` is accepted. Ranges, `latest` and `stable` are refused naming the file and line.

### 6.2 Where versions come from

- **`[versions]` table:** only the versions listed can be installed; the recipe's hash is the check.
  Bumping a version is an edit to the recipe.
- **An index:** the hash comes from the vendor's `sha256_url`, over HTTPS. The one index kind this
  round is `composer`. `https://getcomposer.org/versions` lists only each channel's newest release
  (`2` → 2.11.2, `2.2` → 2.2.32, `1` → 1.10.28 on 2026-11-18), so with an index **(my call)**:
  - a spec naming a channel (`2`, `2.2`, `1`) installs that channel's newest release;
  - an exact version (`2.7.7`) installs that release by the URL template, whether or not the index
    lists it (getcomposer.org keeps every release's phar and `.sha256sum`);
  - any other major.minor (`2.7`) is refused with the hint to name an exact version.
  - A `[versions]` entry in the same recipe is used when the index does not list its channel, so a
    channel the index drops keeps installing (§9.1 pins 2.2.32 for that reason: its LTS maintenance
    ends on 2026-12-31).
- **What a version needs** comes from the recipe's `[[requires]]` rules, not the index (whose
  `min-php` covers only the channel heads): for Composer, 2.3.0 and later need PHP 7.2.5. It is
  recorded in `.yard-install.toml` at install.

**Only a carried recipe may have an index (maintainer, 2026-11-18).** An index trusts the vendor's
TLS for both the file and its hash (the same host), which the engine recipes never allowed; it is
accepted for the carried Composer recipe so a Composer security fix needs no Yard release. A user
recipe with `[source.index]` fails to load; its versions are a `[versions]` table.

## 7. Which version a directory gets

The M4c walk, made generic **(my call: Node's rule is the newer and better one; PHP keeps its own,
§10.2)**: walking up from the cwd, the nearest directory with any of these wins, in this order
inside one directory:

1. a `.yard` that sets the runtime's key (`[runtimes].<key>`, or `node` for Node); a `.yard`
   without it is passed over;
2. the site's setting, in the root of the innermost linked site containing the cwd: when the site
   has a managed `.yard` for its path, that file's key and nothing else (a managed file without the
   key hides `sites.toml`, as Node's `site_node` does today); otherwise `sites.toml`'s (set by
   `yard runtime use`). Skipped when the root has a committed `.yard`;
3. the recipe's `pin_files`, in order.

Nothing up to `/`: the default, `yard.toml`'s `[runtimes.<key>] default` (or `default_node`), else
the recipe's `[[default]]` rules (§9.2), else the newest installed version. Every M4c rule about
files that exist but cannot be used, sizes, BOMs and comments carries over.

### 7.1 `.yard`

`[runtimes]` is a new table: `composer = "2.2"`, `bun = "1.2"`. `php` and `node` stay top-level. A
key no loaded recipe defines is an unmet need, not an invalid file: `yard info` and doctor name it
(`RUNTIME_UNKNOWN`, hint: add `~/.yard/runtimes/<key>.toml`), the site is served (a runtime never
puts a site on its setup page; nginx and PHP-FPM do not need it), and the shims ignore it **(my
call, following the recipes spec's unknown-engine rule)**. An older yardd reads `[runtimes]` as an
unknown table, so the file is invalid there, as `node` once was.

## 8. The shims

- `yard-shim` dispatches on `argv[0]`: a carried shim name goes to its driver or carried recipe, any
  other to the user recipe that owns it (§5.3).
- It resolves the spec (§7), picks the installed version, expands the shim's `exec` and `env`, and
  `exec`s it with the caller's arguments after the recipe's. With `runs_on`, it first resolves that
  runtime for the cwd the same way (the PHP the `php` shim would run, with Yard's `PHPRC` and
  `PHP_INI_SCAN_DIR`), and checks `requires` (§9.2).
- PATH for the child: the version's `path` dirs, then (with `runs_on`) that runtime's bin, then
  Yard's `bin`, then the caller's PATH without Herd's directories (`shim::child_path`), as today.
- `refuse` entries: when the arguments start with one of `args`, exit 1 with `yard: <shim> <args> is
  managed by Yard` and the entry's hint.
- A missing version: `RUNTIME_MISSING`, `<Name> <spec> is not installed (<where the spec came
  from>)`, hint `yard runtime install <key> <spec>`. Node keeps `NODE_MISSING` and its message.
- `wake = true`: the M4e wake ask before exec, as `php` and `composer` do today.
- `YARD_SHIM_DEBUG=1` prints the runtime, version, spec and where it came from.

## 9. Composer

### 9.1 The carried recipe

```toml
schema = 1
key = "composer"
name = "Composer"
runs_on = "php"
wake = true
uses_when = ["composer.json"]

[[default]]
php_below = "7.2.5"
spec = "2.2"
[[default]]
spec = "2"
exclude = ["2.2"]                   # an 8.4 project never gets 2.2 because it is all there is

[[requires]]
from = "2.3.0"
php = "7.2.5"

[shims.composer]
exec = ["{php}", "{home}/composer.phar"]
refuse = [{ args = ["self-update"], hint = "yard runtime install composer <version>" },
          { args = ["selfupdate"], hint = "yard runtime install composer <version>" }]

[source]
kind = "download"
url = "https://getcomposer.org/download/{version}/composer.phar"
file = "composer.phar"

[source.index]
kind = "composer"
url = "https://getcomposer.org/versions"
sha256_url = "https://getcomposer.org/download/{version}/composer.phar.sha256sum"

[versions."2.2.32"]                 # the 2.2 LTS, pinned: used if the index stops listing 2.2
sha256 = "241a3d13b7c6d35d970b1f69a000298b66943541302148a438a932f5dd44a9fc"
```

(`composer.phar.sha256sum` holds `<hash>  composer.phar`; checked on 2026-11-18 for 2.2.25, 2.2.32
and 2.7.7. The 2.2.32 hash above is that file's, fetched the same day.)

### 9.2 Which Composer runs

- A pin (`.yard` `[runtimes] composer`, `yard runtime use composer`, or the default) wins. With no
  pin, the `[[default]]` rules pick against the PHP the cwd resolves to: `2.2` below 7.2.5, else
  `2` excluding 2.2.x. A rule's `php_below` compares the full PHP version, which the shim reads
  without running PHP: a brew keg's from its Cellar directory name (`full_version`), a `path`
  build's from what `php-config --version` said when the build was last checked (§10.3).
- **No fallback to another line.** With only 2.2 installed, an 8.4 project gets `RUNTIME_MISSING`
  for `2` (the rule's `exclude`), not 2.2 (anti-goal). With only 2 installed, a 7.1 project gets
  `RUNTIME_MISSING` for 2.2. A pin the user wrote, `composer = "2"`, is a plain prefix and does
  match 2.2.x, subject to the next rule.
- A pinned version whose recorded requirement the project's PHP does not meet is refused before
  exec: `Composer 2.11.2 needs PHP 7.2.5 or later; this project runs PHP 7.1 (<where the PHP came
  from>)`, hint `yard runtime install composer 2.2`, or pin `composer = "2.2"`.
- `composer.json`'s own `config.platform` and `require.php` are not read for this.

### 9.3 Moving off brew's Composer

- **For one release, the shim falls back to brew's `composer`** when no Yard Composer is installed
  at all, brew's is there, and the project's PHP is 7.2.5 or later, printing one stderr line: `yard:
  using brew's composer; yard runtime install composer for per-project versions`. Doctor reports
  `composer-brew` (`ok` with a hint: doctor has no info level, as the recipes spec §16 found),
  naming the sites brew's runs, and gives those sites no `runtime-missing`, since their `composer`
  works (a PHP 7.1 site, which it cannot run on, still warns). The release after removes the fallback, as `yard db` went **(my call:
  nobody's working `composer` breaks on update)**.
- `yard link` and `yard init` offer the site's Composer in their install list when the root has a
  `composer.json`, as they offer Node. `yard runtime install composer` with no spec installs what
  the cwd resolves to; from outside any project, `2`.
- Yard never installs Composer on its own, at reconcile or after an update.
- Composer keeps reaching local HTTPS sites through PHP's `ca-bundle.pem` (main spec §5.4),
  unchanged: it runs under the shim's PHP and ini.

## 10. PHP builds

### 10.1 Names

A **build name** is `<major>.<minor>` (brew's plain build) or `<major>.<minor>-<label>`, label
`[a-z][a-z0-9-]*`. `zts`, `debug` and `debug-zts` are reserved for shivammathur's variants; any
other label is a user build file. The major.minor is the PHP the build is, so everything that
reasons by version (`require.php` checks, Composer's rule, extension formula names) keeps working.

- `PhpVersion` stays the major.minor; a new `PhpBuild { version, label: Option<Label> }` is what
  `.yard` `php`, `.yard-php`, `yard isolate`, `default_php`, `[php."<build>"]` in `yard.toml`,
  `sites.toml`, `yard php install|uninstall|default`, the setup page and every RPC that names a PHP
  carry. `8.4` parses as today, so no file changes for an existing user.
- Paths: `php/<build>/` (php.ini, fpm.conf, conf.d, pecl.conf, ca-bundle.pem), socket
  `run/php-<build>.sock`, unit `php-fpm-<build>` (`yard restart php-fpm-8.4-zts`; the reserved
  `php-fpm` prefix in `services/engines/key.rs` already covers it). `8.4`'s stay exactly as they
  are.
- Builds below 7.0 are refused (`PHP_UNSUPPORTED`, tabled 2026-11-01). A user build may be a
  version brew's list does not carry (8.6-dev); doctor's `php-build:<build>` is `ok` with a note
  saying so **(my call)**.

### 10.2 Resolution

PHP's own order is unchanged (main spec §4.5: `.yard` with no `php` means the default; managed file;
isolation; `.yard-php`; default). It does not move to §7's walk **(my call: changing which PHP a
directory gets is a behaviour change for every project, and nobody asked for it)**.

### 10.3 Sources

- **Brew, plain and variants.** `8.4` → `shivammathur/php/php@8.4`, `8.4-zts` →
  `shivammathur/php/php@8.4-zts` (8.5: `php`, `php-zts`, `php-debug`, `php-debug-zts`). Keg
  `opt/<formula>`; brew's conf.d is `etc/php/<build>/conf.d` (the variant formulas' own
  `php_version` is `8.4-zts`, so the names line up). Installed builds are read from kegs, as today.
  `yard php install 8.4-zts` installs the formula, behind the existing trust gate. The 8.5
  variants (`php-zts` and the rest) are not keg-only and brew's link step fails while plain `php`
  is linked; the main spec §4.1 rule already covers it: the job succeeds, saying so, when the keg is
  there afterwards, since Yard runs PHP from the keg and never needs the link.
- **`path`.** `~/.yard/runtimes/php/<build>.toml`:

  ```toml
  schema = 1
  name = "8.4-asan"                  # the stem; a label not reserved
  description = "8.4 with AddressSanitizer"
  [source]
  kind = "path"
  prefix = "/Users/me/php-builds/8.4-asan"
  ```

  The prefix must hold `bin/php`, `sbin/php-fpm`, `bin/php-config` and `bin/phpize`, and
  `php-config --version`'s major.minor must be the name's (checked by yardd when the build is first
  seen and whenever `bin/php`'s mtime changes, recorded with the full version in
  `state/php-builds.json` for the shims to read, and reported by doctor, `php-build:<build>`, with
  the mismatch named). Allowed-path rules as §6.
  `yard php install 8.4-asan` checks it and installs nothing.
- A build file whose `kind` is anything but `path` fails to load this round, naming §10.6.

### 10.4 What differs for a build that is not brew's plain one

- **Extensions:** `yard ext add` with a brew formula is refused (`EXT_NOT_FOR_BUILD`, hint `yard
  ext add <name> --pecl`): shivammathur's extension formulas are built for the plain NTS build. pecl
  runs with the build's own `phpize`/`php-config`; bundled toggles work as today.
- **conf.d mirror:** a variant's is `etc/php/<build>/conf.d`; a path build's is
  `php-config --ini-dir` (none: nothing mirrored), mirrored as `10-build-<file>.ini`.
- Pools, Yard's `php.ini`, `99-overrides.ini`, the CA bundle, `PGGSSENCMODE`, sleep and wake, the
  shims and `php-constraint` work unchanged.

### 10.5 When brew drops a PHP

An installed keg stays when its formula leaves the tap, but brew's upgrades of its libraries
(OpenSSL, ICU) can break it. Doctor warns `php-formula-gone:<build>` when the keg is there and
`brew info` no longer knows the formula, with the hint to move to a `path` build. Yard does nothing
on its own.

### 10.6 Later: PHP builds Yard installs

Downloaded prebuilt PHPs and PHPs Yard compiles from source are their own design, parked:
`docs/superpowers/specs/2026-11-18-yard-php-builds-installed-design.md` **(maintainer)**. This design
leaves room for them: a build file's `[source]` takes a `kind`, and `installs/php/<build>/` is
where such a build would live.

## 11. Node

`node` becomes the carried recipe with source `fnm` (shims `node`, `npm`, `npx`; pin files `.nvmrc`,
`.node-version`; `uses_when = ["package.json"]`; default 22). Whether a site uses a runtime is
M4c's rule for every runtime **(my call)**: a `uses_when` file in its root, its own setting giving
a spec, or a pin file in its root (`shim::uses_node` today); a pin above the root decides the
version but alone does not make the site a user. The `fnm` source brings M4c's spec
rules (LTS names, `lts.toml`, refused specs, majors below 16) and its installs. **Nothing a user sees
changes:** `.yard` `node`, `default_node`, `yard node install|uninstall|list|default|use|unuse`,
`NODE_MISSING`, `NODE_IN_USE`, `NODE_EXTRA_CA_CERTS`. M4c's walk becomes §7's generic walk.

## 12. CLI

- `yard runtimes`: every runtime with its origin (carried, or the recipe file), its installed
  versions, the default, and any recipe that did not load. `yard runtimes check <file>` loads a
  recipe with no daemon and prints its shims, pin files and what `install` would fetch, or its
  first error (exit 1), as `yard engines check` does.
- `yard runtime install|uninstall|list|default|use|unuse <key> [<spec>] [--site <name>]`, the M4c
  verbs for any runtime. `yard node …` and `yard php …` stay and are the same RPCs.
- There is no `yard composer …` or `yard <runtime> …` command: `yard composer install` would read as
  `composer install` **(my call)**.
- `yard info` shows each runtime a site uses (spec, source, version, installed).

## 13. Protocol and Yard.app

- New `runtime.list|install|uninstall|default|use|unuse` and `runtimes.list` (with recipe errors),
  `runtimes_changed` event, `StatusInfo.runtime_errors`, and `PhpBuild` wherever a PHP is named,
  in one protocol minor version (2.14) per plan that adds them. `node.*` and `php.*` stay.
- Yard.app: plan 1 shows a site's Composer in its detail and offers its install; plan 2 lists
  builds on the PHP screen and offers the variants in its install picker; plan 3 adds a Runtimes
  screen (each runtime, its versions, install and uninstall, recipe errors).

## 14. Doctor

`runtime:<key>` (recipe broken or on its last good version), `runtime-missing:<site>:<key>` (a site
uses it and its version is not installed), `runtime-unknown:<site>:<key>`, `composer-php:<site>` (the
pinned Composer cannot run on the site's PHP), `composer-brew` (info, §9.3), `php-build:<build>`,
`php-formula-gone:<build>`, and today's Node checks moved onto the contract.

## 15. Data root

```
runtimes/<key>.toml        # USER-OWNED runtime recipes
runtimes/php/<build>.toml  # USER-OWNED PHP build files
installs/<key>/<version>/  # downloads; .yard-install.toml generated inside each
cache/<key>/               # {cache}
state/shims.json           # the bin/ links reconcile made for recipe runtimes
state/php-builds.json      # each path build's checked full version and bin/php mtime
php/<build>/…              # as php/<ver>/ today
```

`yard.toml` gains `[runtimes.<key>] default`; `sites.toml` gains `[sites.<s>.runtimes]`; a managed
`.yard` gains `[runtimes]` (edited by `use`/`unuse` with the managed-file rules).

## 16. Testing

- Unit: recipe loading and every load check, placeholder expansion, §7's walk (Node's existing
  tests must pass unchanged on it), Composer's rule and `requires` refusal, build-name parsing and
  every path that carries one, the index parser against a saved copy of `getcomposer.org/versions`.
- Integration: the shim against fake installs in a temp root; installs against a local HTTPS
  fixture or `file://` stand-in the way the engine recipes' download tests work; reconcile's links
  for recipe shims; a site on a fake `8.4-zts` keg.
- Real machine, one script per plan, each asking the maintainer first:
  - `e2e-composer.sh`: scratch projects on 7.1 and 8.4 (installs nothing but Composer); `yard
    runtime install composer` in each gets 2.2.x and 2.x; `composer -V` in each reports them;
    `composer install` of a small `composer.json` succeeds in both; `[runtimes] composer = "2.2"`
    in the 8.4 project wins; `composer self-update` is refused; brew's `composer` absent.
  - `e2e-php-builds.sh`: `yard php install 8.4-zts` (a brew install), a site isolated to it serves
    with `PHP_ZTS` 1; a `path` build whose prefix is brew's own `opt/php@8.4` (a stand-in for a
    custom build, nothing compiled) serves and its pool is apart from `8.4`'s.
  - `e2e-runtimes.sh`: a user recipe with a pinned `download` and one with a `path` (Apple's
    `python3`), installed, pinned by a pin file and by `.yard`, run through their shims, removed.

## 17. Plans

1. **The contract and Composer.** The trait, the recipe format and loader (carried recipes only),
   the `download` source with `[versions]` and the `composer` index, §7's walk with Node moved onto
   it (no behaviour change), PHP behind the contract with today's behaviour, the shims, `yard
   runtime …`/`yard runtimes`, `.yard` `[runtimes]`, Composer's recipe and §9, doctor, the app's
   Composer line, `e2e-composer.sh`.
2. **PHP builds.** `PhpBuild` everywhere, the variants, `path` builds, §10.4–§10.5,
   the PHP screen, `e2e-php-builds.sh`.
3. **User runtime recipes.** `~/.yard/runtimes/*.toml` loaded and watched, the shims reading them,
   `state/shims.json`, `path` versions, `yard runtimes check`, the Runtimes screen,
   `e2e-runtimes.sh`.

## 18. Docs that move

The main spec (§3.3, §4, §4.5, §8.1), the M4c and project-file specs (pointers), README, DEVELOPMENT.md,
AGENTS.md (the "PHP comes from Homebrew" decision, Status, Layout, e2e list), the homebrew,
config-ownership, cli-output, rpc-protocol and rust-workspace rules, `--help` text.

## 19. Left to the builder

The trait's exact shape; how the registry is published; protocol numbering within 2.x; the index
cache's file name and refresh; whether `PhpBuild` replaces `PhpVersion` in place or wraps it; the
local HTTPS fixture.

## 20. Answered by the maintainer after the builder check (2026-11-18)

1. **Composer's hash from getcomposer.org (§6.2):** yes, for the carried recipe only.
2. **Downloaded PHP builds (§10.3):** out of this design, into their own parked spec (§10.6).

## 21. What plan 1's build decided (2026-11-19)

- **The contract** is one `Runtime` trait over one `RecipeRuntime` struct; `RuntimeKind` (`Php`, `Node`,
  `Recipe`) comes from the recipe's driver or source. PHP and Node keep every code path they had, and
  `runtime.*` for them hands over to `php.*`, `sites.isolate` and `node.*`.
- **A shim's own `wake`** (`[shims.<name>] wake = false`) overrides the recipe's, so PHP's `pecl`,
  `phpize` and `php-config` never wake services.
- **The pin walk** (`runtimes::resolve::resolve_pin`) is M4c's, taking what a `.yard` and a site say for
  the runtime as closures; `resolve_node` runs on it, with Node's tests unchanged.
- **`yard.toml`** refuses `[runtimes.php]` and `[runtimes.node]` (`default_php` and `default_node` say
  those).
- **`refuse`** matches the command past options, an option's value (`-d <dir>`) and `global`, and any
  abbreviation of a refused word in any case (`self-u`, `SELF-UPDATE`), as Composer's console accepts
  them. The real guard is that an install is read-only (files 0444, directories 0555) once it is in
  place: Composer's self-update refuses a phar it cannot write. Uninstall gives write access back,
  moves the version into a stage at once and deletes it there, so a shim sees all of it or none.
- **`yard init`** moves a site's `[sites.<s>.runtimes]` into the file it writes, as it does `node`.
- **A PHP whose full version cannot be read** counts as `x.y.0`, so an unknown 7.2 gets Composer 2.2.
- **With the index unreachable**, or answering with something that is not its JSON (a captive
  portal's page), the recipe's pinned versions stand in only for their own channel:
  `2.2` installs the pinned 2.2.32, `2` fails naming the index. A major.minor that is not a channel is
  refused before any job starts.
- **A Composer too new for the project's PHP** is `INVALID_ARGUMENT` with the version and the PHP it needs
  in its data; doctor reports it as `composer-php:<site>`.
- **`PHP_SHIMS` keeps `composer`**, read as the shims that run PHP; the PHP command builder no longer
  handles it.
- **`yard info`** shows a runtime as a settings row in the Node row's style (`composer  2.2.32 (2.2)
  from default`).
- **Yard.app**: a row per runtime below Node with an Install button while it is missing, and the settle
  sheet offers it after Node.
- **`in_use` and `yard.toml`'s default**: once `[runtimes.<key>] default` is set, every unpinned site
  runs it as a plain prefix, as the shim does, so the recipe rule's `exclude` no longer decides what
  else can run a site, and uninstalling the newest 2.x while 2.2 is installed is allowed.
- **A `[runtimes]` key** that is not a bare TOML key is written quoted by `yard init`.
