---
paths:
  - "crates/yard-core/src/brew.rs"
  - "crates/yard-core/src/php.rs"
  - "crates/yard-core/src/render/php.rs"
  - "crates/yard-core/src/sources.rs"
  - "crates/yard-daemon/src/sources.rs"
  - "crates/yard-daemon/src/rpc/sources.rs"
  - "crates/yard-daemon/src/node.rs"
  - "crates/yard-daemon/src/rpc/node.rs"
commands:
  - "brew "
triggers:
  - installing or locating a PHP version, extension, nginx or dnsmasq
  - launching a brew-installed binary
  - gating an install on brew's trust, or trusting a tap or formula
  - installing Node or running fnm
---

# Homebrew is a package source, never a source of truth

- Never run `brew services`, and never edit or create files under the brew prefix
  (`/opt/homebrew`). `yardd` launches every binary itself, with Yard's config.
- Installing anything with brew changes the maintainer's machine. Ask before running
  `brew install`/`reinstall`/`uninstall` outside a `yard` command he invoked.
- PHP formulas: `shivammathur/php/php@<ver>` for 7.0–8.4. In this tap **8.5 is the
  plain `php` formula** (alias `php@8.5`, keg `opt/php`). Resolve a version as
  `opt/php@<ver>` first, else `opt/php` only when its Cellar version has the same
  major.minor. The tap also ships 5.6 and pre-release 8.6/8.7; Yard supports 7.0–8.5 only.
- Fixed paths: `opt/nginx/bin/nginx`, `opt/dnsmasq/sbin/dnsmasq`,
  `opt/php@<v>/sbin/php-fpm`, `etc/php/<v>/conf.d`, `etc/nginx/{mime.types,fastcgi_params}`.
- php-fpm runs as `php-fpm --nodaemonize -y <yard>/php/<v>/fpm.conf -c <yard>/php/<v>/php.ini`
  with `PHP_INI_SCAN_DIR=<yard>/php/<v>/conf.d` only. Never put brew's `conf.d` on the
  scan path: extension formulas write their own ini there, which would load an
  extension behind `yard ext disable`. Reconcile mirrors brew's `conf.d` into Yard's as
  `10-brew-<file>.ini` (`render::php::brew_inis`), skipping files that load an
  extension `yard.toml` manages; reading brew's files is fine, writing them is not.
- shivammathur's extension formulas keep the `.so` in their own keg
  (`extension="/opt/homebrew/opt/phpredis@7.1/redis.so"`), outside `extension_dir`, so a
  `brew` extension's `50-<name>.ini` loads the path brew's own ini names. The formula's
  keg name can differ from the extension (`redis@7.1` installs `phpredis@7.1`).
  `brew uninstall` removes the keg but leaves the ini in `etc/php/<v>/conf.d`; Yard
  ignores an ini whose absolute `.so` is gone (`render::php::is_stale`).
- **Never Herd's PHP.** Herd puts its `php` and `composer` first on PATH. Run PHP tools
  by absolute keg path (`Brew::php_bin`, never a bare `php`), and give every child the
  PATH from `yard_core::shim::child_path` (keg `bin` first, `Herd` directories dropped):
  `JobCtx::run` does this, pecl gets the keg's PATH, and the shims add Yard's `bin/`
  second.
- **pecl, as `yard ext add` runs it** (`yardd/src/ext.rs`): PATH is the keg's `bin`, then
  brew's `bin` (launchd gives yardd no `/opt/homebrew/bin`, and phpize needs `autoconf`);
  `CPPFLAGS=-I<prefix>/include` and `LDFLAGS=-L<prefix>/lib` (PHP's headers include
  `pcre2.h`); and `-c <root>/php/<v>/pecl.conf`, a PEAR user config Yard writes that
  sets only `php_ini` to `<root>/php/<v>/pecl-php.ini`, a file that never exists.
  brew's `pear.conf` names brew's `php.ini` and pecl writes `extension=` into it after an
  install. `-d`/`-D php_ini=` do **not** stop that (they store the value under the
  `pecl.php.net` channel; `enableExtension` reads it for `pear.php.net`), and
  `pecl config-get` shows the overridden value anyway, so it cannot verify a fix: check
  `PEAR_Config::get('php_ini', null, 'pear.php.net')` or brew's file after a real install. pecl installs into `<prefix>/lib/php/pecl/<api>`, outside
  PHP's `extension_dir`, so Yard records the absolute `so` from `pecl config-get ext_dir`.
- `decorate_workers_output` exists only in PHP ≥ 7.3. Version-gate any FPM or ini
  directive that is newer than 7.0.
- **Trust gate** (`yardd/src/sources.rs`, spec M4b §8). Homebrew 6+ loads a formula from a
  non-official tap only when it or its tap is trusted. Before `php.install` and a brew
  `ext.add` start their job (not inside it, so the CLI gets an ordinary error), the handler
  calls `sources::require_trusted`: `brew --version` (below 6 there is no gate), then
  `brew trust --json=v1`; an untrusted formula is `FORMULA_UNTRUSTED` with the hint
  `yard source trust <formula>`, and brew is never asked to install. `yard_core::sources`
  parses brew's answer. A formula that is already installed, a `.so` and a pinned pecl
  release are not gated, and neither is an extension the tap does
  not list (the job builds it with pecl). When the gate would refuse an `ext.add`,
  `require_trusted_if_listed` looks in `<prefix>/Library/Taps/<owner>/homebrew-<repo>`: the
  formula is listed by `Formula/<name>.rb` (also one shard down), a root `<name>.rb`,
  `Aliases/<name>`, or a key of `formula_renames.json`. Never ask `brew search`: it finds
  real names only, so `redis@8.4` (a rename of `phpredis@8.4`) is invisible to it. A tap
  that is missing, an unreadable `Formula/` or renames file, a renames file over 1 MiB or
  not UTF-8 counts as listing it (the gate holds). Names are looked up lower-cased, as brew
  does (`Redis` is `redis@8.4`), and the checks run in `spawn_blocking`. The layout mirrored
  is flat or one-shard `Formula/`, `Aliases/` and `formula_renames.json`, not
  `HomebrewFormula/` or `tap_migrations.json`.
- **`services.add`** (`rpc/services.rs`, `services::resolve`) asks brew `search --formula
  "/^<base>(@|$)/"` and `info --json=v2 <name>` once per formula, never from inside a job's
  command loop. A formula whose `disabled` is true is not a line; a deprecated one is, and the job
  prints a line naming its `disable_date`, or, when brew leaves that null, `deprecation_date`
  plus a year, as brew itself computes it (`DeprecateDisable::REMOVE_DISABLED_TIME_WINDOW`,
  12 months: mysql@8.0's 2026-04-30 is disabled on 2027-04-30). The line's winning formula that brew lists but cannot
  describe is `FORMULA_MISSING`, not `SERVICE_VERSION_UNSUPPORTED`. The trust gate runs for a
  non-core formula whose keg is missing (MinIO's `minio/stable/minio`), in the handler and again in
  the job before `brew install`, since an install that waited behind another job must not meet a
  tap that was untrusted meanwhile. An existing `opt/minio` is asked about with `brew info
  --json=v2 minio`: homebrew/core's is used as it is (no install, no gate, its full name recorded),
  anything else is recorded as `minio/stable/minio`. Yard never runs `brew uninstall` for a
  service.
- **Trust is brew's, never Yard's.** Read it only with `brew trust --json=v1`; change it
  only with `brew trust` / `brew untrust` (`sources.trust|untrust` jobs, or the CLI's y/N
  prompt). Never read or write brew's `trust.json`, keep a list of Yard's own, or let
  `--yes`, `init`, `link` or an install grant trust. Never set `HOMEBREW_NO_*` (not
  `HOMEBREW_NO_REQUIRE_TAP_TRUST` or any other security variable): `Brew::job_env()`
  (`HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1`) is the only variable Yard sets, and the gate's
  reads set none.
- **Doctor only asks** (`doctor.rs`, spec M4b §10): `brew --version` and `brew trust --json=v1` (read once
  per run, shared by every check, under one 30 s bound), one `brew info --json=v2` for the instances'
  formulae (30 s) and, with no MinIO instance, `brew info --json=v2 minio` (30 s, in
  `sources::installed`, so `sources.list` has it too; a timeout means "not the tap's"). The calls run one
  after the other, so a hung brew costs up to the sum. A brew that cannot answer is a `warn`, never a doctor
  error. It never installs, trusts or runs `brew services`. The `info` call names only formulae brew would
  answer for: homebrew/core's, or a third-party tap's that is already tapped and (brew 6+) trusted. Brew 6
  refuses the whole call over one untrusted formula, and `info <tap>/<formula>` may tap the tap, which a
  read-only doctor must never do.
  `sources::installed(brew, yard, services)` is what `sources.list` and `sources:trust` both read:
  MinIO's `minio/stable/minio` counts as installed only when `opt/minio` exists and a MinIO instance
  in `services.toml` records that formula, or, with no instance, `brew info --json=v2 minio` says the
  keg is the tap's. `opt/minio` alone is not enough: homebrew/core's `minio` shares it.
- **fnm (M4c).** Run it only by absolute path (`Brew::fnm_bin`, `<prefix>/bin/fnm`) with
  `node::fnm_env` (`FNM_DIR=<root>/node`, `FNM_LOGLEVEL=info`), never a bare `fnm` and never
  fnm's own directory. brew installs fnm only inside a `node.install` job, when the binary is
  missing; it is homebrew/core's, so no trust gate. `node.list --available` runs `fnm
  ls-remote` outside a job under the 30 s `BREW_TIMEOUT`. Nothing else runs fnm: doctor and the
  shims only look at `node/node-versions`, and never nvm's or Herd's Node.
- A keg directory with no binary in it (dnsmasq on this machine until 2026-10-20)
  gets the hint `brew reinstall <formula>`, not `brew install`.
