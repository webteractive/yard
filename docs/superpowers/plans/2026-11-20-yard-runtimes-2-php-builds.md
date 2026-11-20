# Runtimes plan 2: PHP builds — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A PHP is named by a build (`8.4`, `8.4-zts`, `8.4-debug`, `8.4-debug-zts`, `8.4-<label>`), so a site can run brew's thread-safe or debug variant or a PHP the user compiled, each with its own pool, beside the plain build.

**Architecture:** `yard_core::php::PhpBuild` wraps today's `PhpVersion` (the major.minor) with an optional label, and replaces it wherever a PHP is *named* (config, files, paths, units, RPC params); version reasoning (`require.php`, Composer's rule, extension formulas, the 7.3 `decorate_workers_output` switch) keeps using `build.version`. One new module, `yard_core::php_builds`, answers the two questions every caller asked brew before: which builds are installed, and where a build's prefix is. Plain and variant builds are brew kegs; a `path` build is a user file in `~/.yard/runtimes/php/` whose prefix yardd checks and records in `state/php-builds.json` for the shims.

**Tech Stack:** Rust 1.92 workspace (`yard-core`, `yard-daemon`, `yard-proto`, `yard-cli`, `yard-shim`), Yard.app on GPUI (Rust 1.98.1, `app/`), bash 3.2 for the e2e script.

**Spec:** `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md` §10 and §17 row 2 (approved 2026-11-18). Plan 1 (`docs/superpowers/plans/2026-11-18-yard-runtimes-1-contract-composer.md`) and the explicit defaults (`docs/superpowers/specs/2026-11-19-yard-explicit-defaults-design.md`) are built.

## Global Constraints

- Build name: `<major>.<minor>` or `<major>.<minor>-<label>`, label `[a-z][a-z0-9-]*`, at most 32 characters (spec §10.1).
- `zts`, `debug` and `debug-zts` are reserved for shivammathur's variants; any other label is a user build file (§10.1).
- A plain or variant build's major.minor is one of `SUPPORTED` (7.0–8.5). A user build may be any major.minor from 7.0 on (an `8.6-dev`); below 7.0 is refused (§10.1).
- `8.4` parses, renders, and names its paths, socket and unit exactly as today: no file changes for an existing user (§10.1).
- Paths per build: `php/<build>/` (php.ini, fpm.conf, conf.d, pecl.conf, ca-bundle.pem), socket `run/php-<build>.sock`, unit `php-fpm-<build>` (§10.1).
- PHP's resolution order is unchanged (main spec §4.5): `.yard` (no `php` means the default), managed file, isolation, `.yard-php`, default (§10.2).
- Brew formulas: `shivammathur/php/php@<build>` for every plain and variant build (the tap aliases `php@8.5`, `php@8.5-zts`, `php@8.5-debug`, `php@8.5-debug-zts`); kegs `opt/php@<build>`, and for 8.5 `opt/php` / `opt/php-<label>` when the Cellar version's major.minor matches; brew's conf.d `etc/php/<build>/conf.d` (§10.3).
- A variant install whose link step fails succeeds when the keg is there afterwards (main spec §4.1 rule, §10.3).
- Path build file: `~/.yard/runtimes/php/<build>.toml`, `schema = 1`, `name` equal to the file stem, a non-reserved label, `[source] kind = "path"`, `prefix` holding `bin/php`, `sbin/php-fpm`, `bin/php-config`, `bin/phpize`; `php-config --version`'s major.minor must be the name's; any other `kind` fails to load naming §10.6 (§10.3).
- A path build is checked when first seen and whenever `bin/php`'s mtime changes, recorded with its full version in `state/php-builds.json` (§10.3, §15).
- The `path` is refused under Herd's, DBngin's or Postgres.app's folders (`PATH_NOT_ALLOWED`, `services::engines::recipe::path_allowed`) (§6, §10.3).
- `yard php install <path build>` checks it and installs nothing (§10.3).
- Doctor: `php-build:<build>` (ok with a note for a version brew's list does not carry) and `php-formula-gone:<build>` (§10.3, §10.5, §14). Yard does nothing on its own when brew drops a PHP (§10.5).
- Pools, Yard's `php.ini`, `99-overrides.ini`, the CA bundle, `PGGSSENCMODE`, sleep and wake, the shims and `php-constraint` work unchanged for every build (§10.4).
- A non-plain build's conf.d mirror: a variant's is `etc/php/<build>/conf.d` (as `10-brew-<file>.ini`, today's rule); a path build's is `php-config --ini-dir` (none: nothing), as `10-build-<file>.ini` (§10.4).
- Protocol: one minor bump for this plan, 2.15 → 2.16 (§13).
- Never Herd's PHP: every PHP Yard runs is an absolute keg or recorded prefix path, PATH from `shim::child_path` (AGENTS.md key decisions).

## Decisions this plan makes (the spec leaves them to the builder, §19)

- **`PhpBuild` wraps `PhpVersion`** rather than replacing it: `PhpVersion` stays the major.minor (Copy, ordered); `PhpBuild { version: PhpVersion, label: Option<BuildLabel> }` is not Copy and is passed by reference.
- **No `--pecl` flag, no `EXT_NOT_FOR_BUILD`.** Spec §10.4 assumed `yard ext add <name> --pecl`; `ext add` has no such flag, because it already falls back to pecl when brew has no formula. For a non-plain build, `ext add` skips brew and builds with pecl directly (the build's own `phpize`/`php-config`), so nothing needs refusing. A `.so` path and bundled toggles work as today.
- **No `PHP_UNSUPPORTED` code.** Below 7.0 stays `INVALID_ARGUMENT`, as `PhpVersion` answers today, with a message naming the 7.0 floor.
- **`best_version` (composer.json's `require.php`) picks among plain builds only:** a project asking for `^8.1` gets brew's plain build, never a variant or a path build it did not name.
- **`php.list` gains `kind` and `variants`** (protocol 2.16): `kind` is `"brew"`, `"variant"` or `"path"`; with `available`, a plain line's row lists its variant builds that are not installed, which the app's install picker offers.
- **Uninstalling a path build is refused** (`INVALID_ARGUMENT`, "Yard installed nothing for 8.4-asan", hint: remove `~/.yard/runtimes/php/8.4-asan.toml`) after the same in-use check every build gets.
- **A missing path build is a manual need** on the setup page and in `yard link`: `Need::Manual { what: "PHP 8.4-asan", command: "write ~/.yard/runtimes/php/8.4-asan.toml" }`; a missing variant is `Need::Php`, installable.
- **`php-formula-gone` reads the tap's files** (`Formula/<name>.rb` or `Aliases/<name>` under `$(brew --repository)/Library/Taps/shivammathur/homebrew-php`), as the trust gate does, never `brew info` per build; with the tap missing it says nothing.

## Review Focus

1. `default_php = "8.4-zts"` (or `.yard` `php = "8.4-zts"`) written by hand while the variant is not installed: sites go to the setup page with an installable `Need::Php { php: "8.4-zts" }`, the `php` shim fails `PHP_NOT_INSTALLED` with hint `yard php install 8.4-zts`, and nothing falls back to plain 8.4. Test in Task 3.
2. 8.5's plain and variant kegs side by side (`opt/php` → 8.5.0, `opt/php-zts` → 8.5.0): `8.5` finds `opt/php` only, `8.5-zts` finds `opt/php-zts` only, and with plain 8.5 uninstalled `8.5` is not installed even though `opt/php-zts` is. Test in Task 3.
3. A path build rebuilt in place as another line (its `bin/php` mtime changes and `php-config --version` now says 8.3 for `8.4-asan`): it stops counting as installed, its pool is not started, the shim refuses it, and doctor fails `php-build:8.4-asan` naming both versions. Test in Task 4.
4. A build file that would shadow something: `8.4-zts.toml` (a reserved label), `name` not equal to the stem, `kind = "download"`, a prefix under `~/Herd`: each fails to load with its reason in doctor, and never changes what `8.4-zts` or anything else runs. Test in Task 4.
5. `yard ext add redis --php 8.4-zts` while plain 8.4 has redis from brew: the variant's extension is built with pecl into its own keg and recorded under `[php."8.4-zts"]`; plain 8.4's entry, conf.d and pool are untouched. Test in Task 5.

---

### Task 1: `PhpBuild`, the build name

**Files:**
- Modify: `crates/yard-core/src/php.rs`
- Modify: `crates/yard-core/src/lib.rs` (re-export `PhpBuild`)

**Interfaces:**
- Produces:
  - `pub struct BuildLabel(String)` — `FromStr` (`[a-z][a-z0-9-]*`, ≤ 32), `as_str()`, `is_variant() -> bool`.
  - `pub const VARIANTS: [&str; 3] = ["zts", "debug", "debug-zts"];`
  - `#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)] pub struct PhpBuild { pub version: PhpVersion, pub label: Option<BuildLabel> }` with `Display` (`8.4`, `8.4-zts`), `FromStr<Err = YardError>`, string `Serialize`/`Deserialize`, `From<PhpVersion>`.
  - `pub enum BuildKind { Plain, Variant, Path }` and `PhpBuild::kind(&self) -> BuildKind`, `PhpBuild::is_plain(&self) -> bool`, `PhpBuild::variants(v: PhpVersion) -> [PhpBuild; 3]`.
  - `pub fn formula(b: &PhpBuild) -> Option<String>` (`None` for a path build), replacing `formula(PhpVersion)`.
  - `PhpVersion::parse_line(s: &str) -> Result<PhpVersion, YardError>`: any `major.minor` with `7 <= major <= 9`, used only for user labels. `PhpVersion`'s own `FromStr` is unchanged.

- [ ] **Step 1: Write the failing tests** in `php.rs`'s test module:

```rust
#[test]
fn build_names_parse_and_print() {
    for s in ["8.4", "7.0", "8.5-zts", "8.4-debug", "8.4-debug-zts", "8.4-asan", "8.6-dev", "9.0-x1"] {
        assert_eq!(s.parse::<PhpBuild>().unwrap().to_string(), s);
    }
    assert_eq!("8.4".parse::<PhpBuild>().unwrap(), PhpBuild::from("8.4".parse::<PhpVersion>().unwrap()));
    assert_eq!("8.4-zts".parse::<PhpBuild>().unwrap().kind(), BuildKind::Variant);
    assert_eq!("8.6-dev".parse::<PhpBuild>().unwrap().kind(), BuildKind::Path);
    for bad in ["8.6", "8.6-zts", "6.9-asan", "5.6", "8.4-", "8.4-Zts", "8.4-1x", "8.4 zts", "8.4-a_b", "8"] {
        assert_eq!(bad.parse::<PhpBuild>().unwrap_err().code, ErrorCode::InvalidArgument, "{bad}");
    }
    assert!("6.9-asan".parse::<PhpBuild>().unwrap_err().message.contains("7.0"));
    let long = format!("8.4-{}", "a".repeat(33));
    assert!(long.parse::<PhpBuild>().is_err());
}

#[test]
fn build_formulas() {
    let f = |s: &str| formula(&s.parse().unwrap());
    assert_eq!(f("8.4").as_deref(), Some("shivammathur/php/php@8.4"));
    assert_eq!(f("8.5-debug-zts").as_deref(), Some("shivammathur/php/php@8.5-debug-zts"));
    assert_eq!(f("8.4-asan"), None);
}

#[test]
fn builds_order_by_version_then_label() {
    let mut v: Vec<PhpBuild> = ["8.4-zts", "7.4", "8.4", "8.4-debug"].iter().map(|s| s.parse().unwrap()).collect();
    v.sort();
    let names: Vec<String> = v.iter().map(ToString::to_string).collect();
    assert_eq!(names, ["7.4", "8.4", "8.4-debug", "8.4-zts"]);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p yard-core --lib php::tests`
Expected: FAIL to compile (`PhpBuild` not found).

- [ ] **Step 3: Implement the interfaces above in `php.rs`.** Split at the first `-`. With no label or a variant label the version goes through `PhpVersion::from_str` (SUPPORTED); a user label goes through `parse_line`, whose error says `PHP builds below 7.0 are not supported`. Change the existing `formula_names` test to the new signature.

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p yard-core --lib php::tests`
Expected: PASS.

- [ ] **Step 5: Commit** (`git add crates/yard-core/src/php.rs crates/yard-core/src/lib.rs`, message `php builds have names`).

### Task 2: `PhpBuild` wherever a PHP is named

Mechanical: every place that names a PHP takes `PhpBuild`; everything that reasons by version reads `.version`. Behaviour for plain builds is unchanged, which the existing suite proves.

**Files:**
- Create: `crates/yard-core/src/php_builds.rs` (registered in `lib.rs`)
- Modify: `crates/yard-core/src/{paths.rs, brew.rs, config/yard.rs, config/sites.rs, project.rs, shim.rs, sites.rs, render/mod.rs, render/php.rs, render/nginx.rs, constraint.rs, runtimes/select.rs}`
- Modify: `crates/yard-daemon/src/{reconcile.rs, defaults.rs, doctor.rs, ext.rs, project.rs, runtimes.rs, sources.rs, modules.rs, rpc/php.rs, rpc/ext.rs, rpc/info.rs, rpc/project.rs, rpc/runtimes.rs, bin/yard-fake-unit.rs}`, `crates/yard-shim/src/main.rs`, and the tests that name a `PhpVersion` where a build is meant.

**Interfaces:**
- Consumes: Task 1's `PhpBuild`.
- Produces:
  - `Paths::{php_dir, php_ini, fpm_conf, php_conf_d, php_ca_bundle, php_overrides, fpm_socket}(&self, b: &PhpBuild) -> PathBuf`
  - `Brew::php_conf_d(&self, b: &PhpBuild) -> PathBuf`, `Brew::php_opt(&self, b: &PhpBuild, exists, read_link) -> Result<PathBuf, YardError>` (plain only until Task 3)
  - `reconcile::fpm_unit(b: &PhpBuild) -> String`
  - `YardConfig { default_php: Option<PhpBuild>, extensions: BTreeMap<PhpBuild, …> }`, `YardConfig::php_default(&self) -> Result<PhpBuild, YardError>`; `SiteEntry.php`, `ProjectFile.php` and the managed file's `php`: `Option<PhpBuild>`
  - `shim::resolve_version(…) -> Result<(PhpBuild, Source), YardError>` (`.yard-php` parses a build)
  - `php_builds::installed(paths: &Paths, brew: &Brew, exists: &dyn Fn(&Path) -> bool, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> Vec<PhpBuild>` — every installed build, oldest first; until Task 3 the plain kegs `Brew::installed_php` finds.
  - `php_builds::prefix(paths: &Paths, brew: &Brew, b: &PhpBuild, exists, read_link) -> Result<PathBuf, YardError>` — the build's prefix (keg); `PHP_NOT_INSTALLED` with hint `yard php install <build>`.
  - Every caller of `Brew::installed_php` and `Brew::php_opt` outside `brew.rs` and `php_builds.rs` goes through these two.

- [ ] **Step 1: Write the failing test** in `crates/yard-core/src/php_builds.rs` and `paths.rs`:

```rust
// paths.rs
#[test]
fn a_builds_paths_carry_its_name_and_plain_ones_are_unchanged() {
    let p = Paths::new("/r");
    let b = |s: &str| s.parse::<PhpBuild>().unwrap();
    assert_eq!(p.php_ini(&b("8.4")), Path::new("/r/php/8.4/php.ini"));
    assert_eq!(p.fpm_socket(&b("8.4")), Path::new("/r/run/php-8.4.sock"));
    assert_eq!(p.php_conf_d(&b("8.4-zts")), Path::new("/r/php/8.4-zts/conf.d"));
    assert_eq!(p.fpm_socket(&b("8.4-asan")), Path::new("/r/run/php-8.4-asan.sock"));
}

// config/yard.rs
#[test]
fn a_build_is_a_default_and_an_extensions_table() {
    let c = parse(Some("default_php = \"8.4-zts\"\n[php.\"8.4-zts\".extensions]\nredis = { source = \"pecl\" }\n")).unwrap();
    assert_eq!(c.default_php.unwrap().to_string(), "8.4-zts");
    assert!(c.extensions[&"8.4-zts".parse().unwrap()].contains_key("redis"));
}
```

plus `fpm_unit(&"8.4-zts".parse().unwrap()) == "php-fpm-8.4-zts"` in `reconcile.rs`'s tests and `.yard-php` holding `8.4-debug` resolving to that build in `shim.rs`'s tests. Use the extension entry shape `yard.toml` already uses; copy it from an existing test in `config/yard.rs`.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p yard-core --lib`
Expected: FAIL to compile.

- [ ] **Step 3: Change the types** as the Interfaces block lists. Where a function reasons by version (constraint checks, `php_full_or_zero`, Composer's `requires`, `ext_formula`, `render::php`'s 7.3 switch, doctor's `>= 7.4` check), pass `b.version`. `constraint::best_version` takes the installed builds and considers plain ones only (plan decision). `render/mod.rs`'s generated-file lookup parses the directory name as a `PhpBuild`. The fake brew fixture (`tests/support/mod.rs` `add_php`) already takes a string and needs no change.

- [ ] **Step 4: Run the whole suite**

Run: `cargo test --workspace` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, the same count plus the new tests; no existing assertion changed except type annotations.

- [ ] **Step 5: Commit** (message `a php is named by its build everywhere`).

### Task 3: brew's variants

**Files:**
- Modify: `crates/yard-core/src/brew.rs`, `crates/yard-core/src/php_builds.rs`, `crates/yard-core/src/project.rs` (needs), `crates/yard-daemon/src/rpc/php.rs`
- Test: `crates/yard-core/src/brew.rs` tests, `crates/yard-daemon/tests/php_builds.rs` (new)

**Interfaces:**
- Consumes: Task 2's `php_builds::installed`, `php_builds::prefix`.
- Produces: `Brew::php_opt` finds variants; `Brew::installed_php_builds(exists, read_link) -> Vec<PhpBuild>` (every SUPPORTED version × plain and the three variants whose keg exists); `php_builds::installed` returns them.

- [ ] **Step 1: Write the failing tests.**

`brew.rs`:

```rust
#[test]
fn variant_kegs_are_found_and_8_5_reads_its_cellar() {
    // opt/php@8.4-zts exists; opt/php -> ../Cellar/php/8.5.0; opt/php-zts -> ../Cellar/php-zts/8.5.0
    let (brew, exists, read_link) = fake_kegs(&["opt/php@8.4", "opt/php@8.4-zts"], &[("opt/php", "8.5.0"), ("opt/php-zts", "8.5.0")]);
    let b = |s: &str| s.parse::<PhpBuild>().unwrap();
    assert!(brew.php_opt(&b("8.4-zts"), &exists, &read_link).unwrap().ends_with("opt/php@8.4-zts"));
    assert!(brew.php_opt(&b("8.5"), &exists, &read_link).unwrap().ends_with("opt/php"));
    assert!(brew.php_opt(&b("8.5-zts"), &exists, &read_link).unwrap().ends_with("opt/php-zts"));
    assert!(brew.php_opt(&b("8.4-debug"), &exists, &read_link).is_err());
    let names: Vec<String> = brew.installed_php_builds(&exists, &read_link).iter().map(ToString::to_string).collect();
    assert_eq!(names, ["8.4", "8.4-zts", "8.5", "8.5-zts"]);
}

#[test]
fn plain_8_5_is_not_its_zts_keg() {
    let (brew, exists, read_link) = fake_kegs(&[], &[("opt/php-zts", "8.5.0")]);
    assert!(brew.php_opt(&"8.5".parse().unwrap(), &exists, &read_link).is_err());
}
```

(`fake_kegs` is a test helper you write over a `BTreeSet` of paths and a map of link targets, as the existing `brew.rs` tests stub `exists`/`read_link`.)

`tests/php_builds.rs` (daemon, fake brew):
- `a_site_isolated_to_a_variant_gets_its_own_pool`: `add_php("8.4")`, `add_php("8.4-zts")`, link two sites, isolate one to `8.4-zts` (RPC `sites.isolate`); after reconcile both `php-fpm-8.4` and `php-fpm-8.4-zts` units run, the isolated site's nginx file names `run/php-8.4-zts.sock`, and `php/8.4-zts/php.ini` exists.
- `installing_a_variant_asks_brew_for_its_formula`: `php.install { php: "8.4-zts" }` runs the fake brew with `install shivammathur/php/php@8.4-zts` (read the fake brew's argument log as the existing `php.install` test does), behind the trust gate exactly as plain.
- `a_default_variant_that_is_not_installed_is_a_need` (Review Focus 1): `default_php = "8.4-zts"` with only `8.4` installed; a linked site's `needs` is `[Need::Php { php: "8.4-zts" }]`, `installable()`; the `php` shim run in it exits 1 with `PHP 8.4-zts is not installed` and hint `yard php install 8.4-zts`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p yard-core --lib brew && cargo test -p yard-daemon --test php_builds`
Expected: FAIL (variants not found; `8.4-zts` not installed).

- [ ] **Step 3: Implement.** `php_opt`: try `opt/php@<build>`; else the unversioned keg `opt/php` (plain) or `opt/php-<label>` (variant) when its link's Cellar version has the build's major.minor. A path-kind build is never a keg. `install` keeps the link-failure rule, with `formula(&b)`; `uninstall` of a variant runs `brew uninstall` on its formula.

- [ ] **Step 4: Run them to see them pass**, then `cargo test --workspace`.

- [ ] **Step 5: Commit** (message `brew's zts and debug php builds`).

### Task 4: path builds

**Files:**
- Modify: `crates/yard-core/src/php_builds.rs` (build files, the state record), `crates/yard-core/src/paths.rs` (`php_build_files_dir() -> runtimes/php`, `php_builds_state() -> state/php-builds.json`)
- Create: `crates/yard-daemon/src/php_builds.rs` (the check, run at reconcile)
- Modify: `crates/yard-daemon/src/reconcile.rs` (step 0b, before `defaults::fill`), `crates/yard-daemon/src/rpc/php.rs` (install/uninstall), `crates/yard-core/src/project.rs` (needs), the conf.d mirror in `reconcile.rs` and `render/mod.rs`'s generated-file lookup
- Test: `php_builds.rs` unit tests, `crates/yard-daemon/tests/php_builds.rs`, `crates/yard-shim/tests/shim.rs`

**Interfaces:**
- Consumes: Tasks 2–3.
- Produces:
  - `php_builds::BuildFile { name: PhpBuild, description: String, prefix: PathBuf }`, `php_builds::parse_build_file(text: &str, file: &Path, home: &Path) -> Result<BuildFile, YardError>` (`CONFIG_INVALID` with the file and line, as recipes report).
  - `php_builds::load_build_files(paths: &Paths, home: &Path) -> (Vec<BuildFile>, Vec<YardError>)`
  - `#[derive(Serialize, Deserialize)] php_builds::Checked { prefix: PathBuf, mtime: i64, full: Option<String>, ini_dir: Option<PathBuf>, error: Option<String> }` and `php_builds::read_state(paths) -> BTreeMap<String, Checked>`; written only by yardd, atomically.
  - `php_builds::installed` adds every build in the state record whose `Checked` has no `error`, whose build file still exists, and whose `mtime` equals `<prefix>/bin/php`'s now. It does not parse the files: yardd's check already applied the load rules and the allowed-path rule, so the signature stays Task 2's (no home). `php_builds::prefix` answers the recorded prefix on the same terms, else `PHP_NOT_INSTALLED` with hint `yard doctor` (file present) or `write ~/.yard/runtimes/php/<build>.toml` (absent).
  - `yard_daemon::php_builds::check(paths: &Paths, home: &Path) -> BTreeMap<String, Checked>`: for each loaded file, reuse the record when prefix and mtime match, else run `<prefix>/bin/php-config --version` and `--ini-dir` by absolute path with a 5 s timeout and record the result; an error names what failed (`bin/php-config says 8.3.14, but 8.4-asan is a PHP 8.4 build`).

- [ ] **Step 1: Write the failing tests.**

Unit (`yard-core` `php_builds.rs`):
- `a_build_file_parses`: the spec's example parses to `8.4-asan` with its prefix.
- `build_files_that_would_shadow_are_refused` (Review Focus 4), each asserting the message names the reason: file `8.4-zts.toml` (`zts is reserved for brew's variant`), `name = "8.4-other"` in `8.4-asan.toml` (`name must be the file's name`), `[source] kind = "download"` (`§10.6`), `prefix` under `<home>/Herd` (`PATH_NOT_ALLOWED`), a relative prefix, `schema = 2`, a prefix missing `sbin/php-fpm` (names it), and `name = "6.9-x"` (below 7.0).

Daemon (`tests/php_builds.rs`), with a fake prefix whose `bin/php`, `sbin/php-fpm`, `bin/phpize` are the fake unit and whose `bin/php-config` is a script printing `8.4.3` for `--version` and a scratch dir for `--ini-dir`:
- `a_path_build_serves_and_is_recorded`: build file `8.4-asan.toml`, a site isolated to `8.4-asan` (isolate refuses until it is installed, so write the file and wait for the reconcile first); the unit `php-fpm-8.4-asan` runs from the prefix's `sbin/php-fpm`, `state/php-builds.json` has `full = "8.4.3"`, an ini in the `--ini-dir` is mirrored as `php/8.4-asan/conf.d/10-build-<file>.ini`.
- `a_rebuilt_path_build_of_another_line_stops_counting` (Review Focus 3): after it serves, rewrite `php-config` to print `8.3.14` and touch `bin/php` (new mtime); within the scan the build is not in `php.list`'s installed, `php-fpm-8.4-asan` stops, and the site has `Need::Manual`.
- `installing_a_path_build_checks_and_installs_nothing`: `php.install { php: "8.4-asan" }` runs no brew and its job says the build is ready; with the file missing it fails `PHP_NOT_INSTALLED` naming the file to write.
- `uninstalling_a_path_build_is_refused`: `php.uninstall` answers `INVALID_ARGUMENT` naming the file to remove (plan decision), after the in-use refusal when a site runs it.

Shim (`crates/yard-shim/tests/shim.rs`): `the_php_shim_runs_a_path_builds_binary` reads the build file and the state record and execs `<prefix>/bin/php`; with the record's mtime stale it fails `PHP_NOT_INSTALLED`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p yard-core --lib php_builds && cargo test -p yard-daemon --test php_builds && cargo test -p yard-shim`
Expected: FAIL.

- [ ] **Step 3: Implement.** The check runs at reconcile step 0b, before `defaults::installed_now` and `fill`, so a path build counts toward everything that follows in that reconcile; `defaults::Installed.php` lists builds, so the 30 s scan notices a build file added or rebuilt. The shim reads only the files and the record and never runs `php-config` (it has no daemon and must stay fast). `best_version` already ignores path builds (Task 2).

- [ ] **Step 4: Run them to see them pass**, then `cargo test --workspace`.

- [ ] **Step 5: Commit** (message `php builds from a prefix the user compiled`).

### Task 5: extensions on a build

**Files:**
- Modify: `crates/yard-daemon/src/rpc/ext.rs`, `crates/yard-daemon/src/ext.rs`
- Test: `crates/yard-daemon/tests/ext.rs` (or the existing extension test file; find it with `grep -l ext.add crates/yard-daemon/tests`)

**Interfaces:**
- Consumes: `PhpBuild::is_plain`, `php_builds::prefix`.
- Produces: `ext.add` for a non-plain build never calls `require_trusted_if_listed` or brew; it runs pecl with the build's `bin/phpize` and `bin/php-config` (through the `Add` job's existing pecl path with `opt` = the build's prefix).

- [ ] **Step 1: Write the failing test** `an_extension_for_a_variant_is_built_with_pecl` (Review Focus 5): `add_php("8.4")` with redis recorded from brew under `[php."8.4"]`, `add_php("8.4-zts")`; `ext.add { name: "redis", php: "8.4-zts" }` runs no brew (the fake brew log has no `install`), runs the variant keg's pecl, records `redis` under `[php."8.4-zts"]` with `source = "pecl"`, and `[php."8.4"]`, `php/8.4/conf.d` and the `php-fpm-8.4` unit's start count are unchanged.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p yard-daemon --test <that file> an_extension_for_a_variant`
Expected: FAIL (brew asked for `shivammathur/extensions/redis@8.4-zts`).

- [ ] **Step 3: Implement**: branch on `v.is_plain()` before the trust gate and the job's brew attempt.

- [ ] **Step 4: Run it to see it pass**, then `cargo test --workspace`.

- [ ] **Step 5: Commit** (message `extensions for a php build come from pecl`).

### Task 6: doctor

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs`
- Test: `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Consumes: `php_builds::load_build_files`, `php_builds::read_state`, `formula`.
- Produces: checks `php-build:<build>` and `php-formula-gone:<build>`.

- [ ] **Step 1: Write the failing tests:**
- `a_path_build_is_ok_and_says_so_when_brew_has_no_such_line`: a checked `8.6-dev` build reads `ok`, message `8.6-dev runs PHP 8.6.0 from <prefix>; brew's tap carries no 8.6, so Yard checks only what is here`.
- `a_path_build_that_fails_its_check_is_named`: the rebuilt build of Task 4 fails with the record's error; a build file that did not load fails naming the file and line; hint for both `edit ~/.yard/runtimes/php/<file>`.
- `a_keg_whose_formula_left_the_tap_warns`: fake tap dir without `Formula/php@7.0.rb` or `Aliases/php@7.0` and an installed `opt/php@7.0` keg → `warn php-formula-gone:7.0`, `brew's shivammathur/php no longer has php@7.0; its keg stays, but brew's library upgrades can break it`, hint `build it yourself and add ~/.yard/runtimes/php/7.0-<label>.toml`. With the tap directory absent, no such check.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p yard-daemon --test doctor php_build`
Expected: FAIL.

- [ ] **Step 3: Implement** the two checks. The tap directory is `$(brew prefix)/Library/Taps/shivammathur/homebrew-php` on Apple Silicon (`Brew` already knows its prefix; the repository is the prefix there).

- [ ] **Step 4: Run them to see them pass**, then `cargo test --workspace`.

- [ ] **Step 5: Commit** (message `doctor checks php builds`).

### Task 7: protocol 2.16 and the CLI

**Files:**
- Modify: `crates/yard-proto/src/{lib.rs, types.rs}` (`PROTOCOL_VERSION = "2.16"`, `PhpInfo.kind: Option<String>`, `PhpInfo.variants: Vec<String>` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`)
- Modify: `crates/yard-daemon/src/rpc/php.rs` (`list`), `crates/yard-cli/src/commands/php.rs` (find with `grep -l PHP_LIST crates/yard-cli/src`), `crates/yard-cli/src/cli.rs` help text
- Test: `crates/yard-proto/src/types.rs` tests, `crates/yard-daemon/tests/php_builds.rs`, `crates/yard-cli/tests/cli.rs` (MockDaemon)

**Interfaces:**
- Consumes: everything above.
- Produces: `php.list` rows for every installed build with `kind`; with `available`, plain rows carry `variants` (the line's variant builds not installed). `yard php list` gains a KIND column; `--available` prints, under the table, `Variants: yard php install <line>-zts | -debug | -debug-zts` for each line shown. `--help` for `php install|uninstall|default`, `isolate` and `.yard`'s `php` names build names (`8.4`, `8.4-zts`, `8.4-<label>`).

- [ ] **Step 1: Write the failing tests:** a `PhpInfo` without `kind`/`variants` deserializes (an older yardd); `php_list_shows_builds_and_offers_variants` (daemon: `8.4` and `8.4-zts` installed, `available: true` → the `8.4` row's `variants == ["8.4-debug", "8.4-debug-zts"]`, kinds `brew` and `variant`); a CLI snapshot of `yard php list --available` against MockDaemon.

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p yard-proto && cargo test -p yard-daemon --test php_builds php_list && cargo test -p yard-cli`
Expected: FAIL.

- [ ] **Step 3: Implement**, and add 2.16 to `.hydra/rules/rpc-protocol.md`'s protocol list.

- [ ] **Step 4: Run them to see them pass** (`INSTA_UPDATE=always` once for the new snapshot; read it before keeping it), then `cargo test --workspace`.

- [ ] **Step 5: Commit** (message `php.list names builds and offers variants`).

### Task 8: Yard.app's PHP screen

**Files:**
- Modify: `app/yard-app/src/screens/php.rs`, and a pure helper in `app/yard-app-model/src/` if the choice needs one (the app rule: decisions in `yard-app-model`)
- Test: `app/yard-app-model` unit test; `app/yard-app/src/screens` test if the screen has them

**Interfaces:**
- Consumes: `PhpInfo.kind`, `PhpInfo.variants` (empty from an older yardd).
- Produces: builds appear as rows (they already do: `version` is the build name); a plain installed or not-installed row with `variants` shows an **Install variant…** picker (the existing `widgets::picker::Picker`) listing them, which runs `php.install` with the chosen build as an Activity job, as the row's Install button does. The kind shows in the row (`zts`, `debug`, `path`), and a path build has no Install or Uninstall action.

- [ ] **Step 1: Write the failing test** in `yard-app-model`: `variant_choices(info: &PhpInfo) -> Vec<String>` returns `info.variants` for a `kind = "brew"` row and nothing for others or an older yardd's row.
- [ ] **Step 2: Run it to see it fail** (`cd app && cargo test -p yard-app-model`).
- [ ] **Step 3: Implement** the helper and the picker on the screen.
- [ ] **Step 4: Run** `cd app && cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`. Expected: PASS.
- [ ] **Step 5: Commit** (message `yard.app installs php variants`). Then remove `app/target`'s debug build (`cargo clean --profile dev` in `app/`).

### Task 9: the real-machine script and the docs

**Files:**
- Create: `scripts/e2e-php-builds.sh`
- Modify: `docs/superpowers/specs/2026-10-19-yard-design.md` (§3.3, §4, §4.5, §8.1), `docs/superpowers/specs/2026-11-18-yard-runtimes-design.md` (a §22 "What plan 2's build decided", listing this plan's decisions), README, DEVELOPMENT.md, AGENTS.md (the "PHP comes from Homebrew" decision, Status, Layout, the e2e list), `.hydra/rules/{homebrew,config-ownership,cli-output,rust-workspace}.md`

- [ ] **Step 1: Write `scripts/e2e-php-builds.sh`** (spec §16), shaped like `scripts/e2e-composer.sh` (bash 3.2, `set -euo pipefail`, a trap that restores, Herd resolver checksum before and after):
  - refuses to start while `php@8.4-zts` is installed or `~/.yard/runtimes/php/` holds a file, so it removes only what it added;
  - backs up `yard.toml` and `sites.toml`;
  - `yard php install 8.4-zts` (a brew install of `shivammathur/php/php@8.4-zts`);
  - links a scratch project under `$TMPDIR`, isolates it to `8.4-zts`, curls a page printing `PHP_ZTS` and expects `1`;
  - writes `~/.yard/runtimes/php/8.4-e2e.toml` with `prefix` brew's own `opt/php@8.4` (a stand-in for a custom build, nothing compiled), isolates a second scratch project to it, curls it, and checks `yard php list --json` shows both builds with their kinds and that `php-fpm-8.4-e2e` is a unit apart from `php-fpm-8.4`;
  - `yard doctor --json` has `php-build:8.4-e2e` ok;
  - cleans up: unlink both, remove the build file, `yard php uninstall 8.4-zts`, restore the TOMLs with `cp -p` and `cmp`.
  - `shellcheck scripts/e2e-php-builds.sh` stays clean. Do not run it: it installs from brew, so the maintainer approves the run.

- [ ] **Step 2: Update the docs** listed above. Grep for every name this plan touched (`PhpBuild`, `php-builds.json`, `runtimes/php`, `php-build:`, `php-formula-gone`, `10-build-`, `2.16`, `variants`) and fix what no longer matches.

- [ ] **Step 3: Verify**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && shellcheck scripts/e2e-php-builds.sh`
Expected: all pass.

- [ ] **Step 4: Commit** (message `e2e-php-builds.sh and the php builds docs`). Then `cargo clean --profile dev`.
