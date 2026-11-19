# Explicit runtime defaults

Status: draft, 2026-11-19 (builder check done; §2.1 and §2.2 decided by the maintainer). Amends the runtimes design
(`2026-11-18-yard-runtimes-design.md` §7, §9.2, §21) and the main spec's PHP default (§4.5), and
M4c's `default_node`.

## 1. Why

The maintainer wants Yard to be explicit about what it runs: "defaults like the pinned versions".
Today three defaults are not written anywhere a person can read:

- PHP: with no `default_php` in `yard.toml`, every unpinned site gets 8.4, compiled into Yard
  (`PhpVersion::DEFAULT`).
- Node: with no `default_node`, `node::DEFAULT_SPEC` (22) applies, a major that runs whatever newest
  22.x is installed.
- Composer: the carried recipe's `[[default]]` rules pick at run time: `2.2` below PHP 7.2.5, else
  `2` excluding 2.2. "2" is a prefix, so installing 2.12 moves every unpinned project onto it
  without anyone choosing that. `yard runtime list composer` prints "default: yes" for a version
  that was computed, not chosen.

## 2. What changes

A runtime's default is an **exact version, written in `yard.toml`**, exactly like a pin. It moves
only through `yard runtime default <key> <version>` (and its aliases `yard php default`, `yard node
default`). Installing a newer version never moves it. Nothing compiled into Yard and no recipe rule
decides at run time what an unpinned project gets.

"Exact" is the finest version Yard controls:

| Runtime  | Written as                | Example    | Why that grain |
| -------- | ------------------------- | ---------- | -------------- |
| PHP      | `default_php`             | `"8.4"`    | brew keeps one keg per line; Yard cannot hold a patch release |
| Node     | `default_node`            | `"22.23.5"`| fnm holds each full version side by side |
| Composer | `[runtimes.composer] default` | `"2.11.2"` | each version is its own install |

The keys stay where they are today (`default_php`, `default_node`, `[runtimes.<key>] default`);
`yard.toml` still refuses `[runtimes.php]` and `[runtimes.node]` **(my call: no key migration for
what is a change of meaning, not of place)**.

### 2.1 Old PHP and Composer (option B, the maintainer's pick)

There is one Composer default. A project whose PHP cannot run it carries **its own pin, written on
the site**:

- `yard link` and `yard init` (and Yard.app's settle sheet, which offers Composer after Node), for a
  project that uses Composer (`composer.json`) and whose PHP is below what the default needs
  (the recipe's `[[requires]]`: 2.3.0 needs PHP 7.2.5), write an exact pin, the newest installed or
  indexed version whose requirement that PHP meets (2.2.32 today), into the site's managed `.yard`
  or `sites.toml`, the same place `yard runtime use` writes.
- With no Composer default written yet, link and init still write that pin for an old-PHP project
  (the pin does not depend on the default; it depends on what the PHP can run) **(my call)**.
- Moving a site's PHP below the cutoff through Yard (`yard isolate`, `yard php use` on a managed
  `.yard`) writes the same pin when the site uses Composer and has none **(my call: the same rule as
  link, wherever Yard is the one changing the PHP)**. A PHP changed in a file the user edits
  (`.yard-php`, a committed `.yard`) gets the shim's refusal and doctor's hint instead.
- A project with a committed `.yard` is the user's file and is never edited: link and init say which
  line to add (`[runtimes] composer = "2.2.32"`), and doctor keeps saying it.
- An unpinned project whose PHP cannot run the default fails in the shim, before exec, as today's
  "Composer 2.11.2 needs PHP 7.2.5 or later" refusal does, with the hint `yard runtime use composer
  2.2.32` (or the `.yard` line for a committed one). It never quietly gets another version.

Since nothing picks a version by rule any more, the recipe format's `[[default]]` rules and their
`exclude` go away (§21's `in_use` note with them). `[[requires]]` stays: it is what the shim's
refusal, doctor's `composer-php` and the pin link writes are computed from. User runtime recipes
(plan 3) are not built yet, so no user recipe loses anything.

### 2.2 When a default is written (option A, the maintainer's pick, 2026-11-19)

Yard fills in a default that is not written, and only one that is not written. A written default
never moves except by `yard runtime default`. Removing the line from `yard.toml` means Yard fills it
in again at the next check.

- **When it checks.** At the start of every reconcile, before the config is loaded: yardd's start,
  every change to the TOMLs (so `yard link`, `init` and `isolate` too), and the reconcile each
  install job ends with. An upgrade, a fresh Mac whose PHP came from `brew install`, and the first
  `yard node install` are therefore all the same case **(decided while planning: one place for the
  check, and an isolate's pin needs no code of its own)**.
- **What it writes,** for a runtime with no default and at least one version installed:
  - PHP: `8.4` when 8.4 is installed (today's compiled-in value, so an upgrade changes nothing),
    else the newest installed line.
  - Node: when `default_node` is unset, the newest installed 22.x (today's compiled-in spec), else
    the newest installed version. A `default_node` that is written but not exact (`22`,
    `lts/iron`) is a written value and is left alone, as every written value is; doctor warns about
    it and names the exact version that runs today (§2.4) **(decided while planning: the check runs
    on every reconcile, so it cannot tell an upgrade's old value from a hand edit)**.
  - Composer, and any recipe runtime: the newest installed version at or above the recipe's newest
    `[[requires]]` boundary (2.3.0 for Composer). With only 2.2.32 installed, which is there for an
    old project's pin, nothing is written. Linking a PHP 7.1 project first therefore does not make
    2.2 every project's Composer **(my call)**. A recipe with no `[[requires]]` uses the newest
    installed version.
  - On the same check, for each linked site that uses Composer, has no Composer pin and no
    committed `.yard`, and whose PHP cannot run the new default: the pin §2.1 describes, when a
    version that fits is installed.
- **How it is written and reported.** Through `TomlFile::edit`, keeping comments. yardd logs each
  value it writes. `yard doctor` shows an `info` line per value written since yardd started
  (`runtime-default-written:<key>`: "wrote default_php = 8.4 to yard.toml; yard runtime default php
  <v> changes it"). The install command that caused the write says so in its output.
- **Install jobs** made for a pin (§2.1: link, init, or `yard runtime install composer` in an old-PHP
  project) write that pin, not the default. The fill-in above may still write a default after them
  when a version qualifies.

### 2.3 With no default written

This happens only while nothing qualifying is installed. A runtime with no default and no pin is
then `RUNTIME_MISSING` for that project (for PHP, the setup page, as for a PHP that is not
installed), with the hint naming the install command. The brew Composer fallback (runtimes spec
§9.3) keeps its one release unchanged. It applies only while Yard has no qualifying Composer
installed, and so no default.

### 2.4 Guarding the default

- `yard runtime uninstall` refuses the default's exact version (`RUNTIME_IN_USE`, naming
  `yard.toml`) until the default moves.
- `yard runtime default <key> <version>` requires that exact version installed. Given a prefix
  (`2`, `22`, `8`), it is refused with the matching installed versions listed **(my call: a prefix
  default is the floating behaviour this removes)**. PHP takes a line, which is already exact for it.
- Moving the Composer default to a version some unpinned site's PHP cannot run is allowed, and the
  command lists those sites with the `yard runtime use` line for each.
- A default in `yard.toml` that is not exact (a hand-written `default_node = "20"`, or
  `[runtimes.composer] default = "2"`) is not a config error, which would stop serving: it is read
  as the prefix it is, as a project pin is, and doctor warns `runtime-default-not-exact:<key>` with
  the exact version it resolves to now and the command that writes it **(my call: `yard.toml` is
  the user's file too; nothing writes a prefix there any more, so only a hand edit can)**.
- When `yard.toml` does not parse (a hand edit in progress), the shims use the defaults from the
  last version that did: yardd keeps them in `run/defaults.toml` after every good read, and a shim
  that falls back to it says so on stderr, as today's shim warns before using 8.4. With no such
  file, an unpinned project fails with `CONFIG_INVALID` **(my call: the last written value, never
  a compiled-in one)**.

### 2.5 Seeing it

Everywhere a runtime's version is shown, its source is shown with it: `pin` (`.yard`, managed
`.yard`, `sites.toml`, a pin file such as `.nvmrc`) or `default` (`yard.toml`):

- `yard info`: one row per runtime the site uses, as Composer's row does today
  (`composer  2.11.2  default (yard.toml)`).
- `yard sites`: the PHP column marks a default with a trailing `*` and a footnote line; `--json`
  carries the source per runtime.
- `yard runtimes` and `yard runtime list`: the DEFAULT column is the written exact version, or
  `none`; no rule text.
- Yard.app: the site detail's PHP, Node and Composer rows show the same source.

## 3. Goals, non-goals, anti-goals

- **Goal:** for any project, the version that runs is one written somewhere a person can read, and
  `yard info` says where.
- **Non-goal:** project pins stay as they are. `.yard`'s `composer = "2.2"`, an `.nvmrc` of `22` or
  `lts/jod`, and `~/.nvmrc` are files the user wrote; prefixes there keep their meaning.
- **Non-goal:** PHP patch releases. Brew upgrades a keg in place; doctor already names the version.
- **Anti-goal:** Yard choosing a version on its own at run time, or an install changing what an
  existing project runs.
- **Anti-goal:** editing a committed `.yard`.

## 4. Approaches considered

- **A. Two written Composer defaults** (`default` and one for PHP below 7.2.5). Keeps the PHP
  cutoff as a rule inside Yard, only with written outcomes. Not picked.
- **B. One default per runtime; old projects carry their own pin** (picked, 2026-11-19). Every
  exception is written on the site it applies to.
- **Filling in a missing default** (A, picked) against **only the user's commands writing one** (B,
  with onboarding asking): A keeps a fresh Mac and a deleted line working with no extra step, and
  stays explicit because what it fills in is written down and reported.
- **C. Snapshot every runtime into every site at link time**, so the default only seeds new sites.
  Most explicit, but moving all sites means editing each one. Not offered: the maintainer asked for
  defaults that behave like pins, not for defaults to disappear.

## 5. What it touches

- `yard-core`: `config/yard.rs` (`default_php` optional, no `PhpVersion::DEFAULT` fallback;
  `default_node` exact), `node.rs` (`DEFAULT_SPEC` goes), `shim.rs` (no fallback; default source
  reported), `runtimes/` (`[[default]]` and `exclude` removed from the format and from
  `composer.toml`; `pick_composer` reads the written default; the "newest fitting version" helper
  for §2.1), `sites.rs`.
- `yard-daemon`: the fill-in check (§2.2) at start and after install jobs, `in_use`,
  `runtime.default` validation, link/init's pin, doctor (`info` lines, `composer-php` hint, a
  missing default), `rpc/runtimes.rs`.
- `yard-proto`: protocol 2.15 if `runtimes.list`'s default field changes shape (rule text to exact
  version or none) and for the per-runtime source in `sites.list`.
- `yard-cli`, Yard.app: the display in §2.5.
- Docs: the runtimes spec (§7, §9.2, §21), the main spec §4.5, the M4c spec's default, README, the
  config-ownership rule (yardd's fill-in write to `yard.toml`), AGENTS.md.
- `scripts/e2e-composer.sh`: its 8.4 project now gets the written default and its 7.1 project the
  pin link writes; add a check that installing a newer 2.x leaves the 8.4 project where it was.

## 6. Left to the builder

The exact doctor check names, the `yard sites` marker, the wording of the link and upgrade notices,
and whether the start check runs in yardd's startup before the first reconcile or in the first
reconcile itself, as long as no shim can run against a `yard.toml` that is half written.
