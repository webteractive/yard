---
paths:
  - "crates/yard-core/src/brew.rs"
  - "crates/yard-core/src/php.rs"
  - "crates/yard-core/src/render/php.rs"
commands:
  - "brew "
triggers:
  - installing or locating a PHP version, extension, nginx or dnsmasq
  - launching a brew-installed binary
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
- `decorate_workers_output` exists only in PHP ≥ 7.3. Version-gate any FPM or ini
  directive that is newer than 7.0.
- A keg directory with no binary in it (dnsmasq on this machine until 2026-10-20)
  gets the hint `brew reinstall <formula>`, not `brew install`.
