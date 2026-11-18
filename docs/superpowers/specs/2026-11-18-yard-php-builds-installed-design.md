# PHP builds Yard installs: downloaded and compiled — design (parked)

Status: **parked (maintainer, 2026-11-18: "add a separate spec for it so we can focus on it
later").** Not designed yet: this file holds what is known, so the design can start from it. It
follows the runtimes design (`docs/superpowers/specs/2026-11-18-yard-runtimes-design.md`), whose
§10 gives a PHP a build name (`8.4-asan`) and lets a build come from brew (plain and
shivammathur's ZTS/debug variants) or from a `path` the user compiled. This one is about the two
sources that design leaves out: a PHP Yard **downloads** prebuilt, and a PHP Yard **compiles**.

## 1. Why it exists

- **An old PHP after brew drops it.** shivammathur's tap carries 5.6–8.7 today, but each old line
  builds only with patches against current OpenSSL and ICU (`php@7.0` depends on `openssl@4` and
  `icu4c@78` and carries a `configure.in`/`acinclude.m4` patch). When the tap drops a line, an
  installed keg keeps working only until brew upgrades a library under it. The runtimes design
  covers this with a `path` build the user compiles himself; this design would make it Yard's job.
- **Builds the team shares.** A build with the maintainer's flags or extensions (or an ASan or
  debug build) made once and installed on every teammate's Mac, rather than compiled on each.
- **FrankenPHP** (tabled) brings its own PHP build; a downloaded ZTS PHP may be how it arrives.

## 2. What the runtimes builder check found (2026-11-18)

- **A PHP prefix is not relocatable.** `php-config --prefix`, `--extension-dir` and `--ini-dir`,
  PEAR's paths and the compiled-in `extension_dir` and config-file path all name where it was built.
  Unpacked under `~/.yard/installs/php/<build>/` on another Mac, each is wrong. Yard already passes
  `-c` and `PHP_INI_SCAN_DIR` and writes `extension_dir` only for pecl, so some of this is
  overridable from Yard's ini; `phpize` and `php-config` are not.
- **It links against the build Mac's libraries.** A prefix compiled against brew's OpenSSL, ICU,
  libxml2 and the rest needs the same dylibs at the same paths (or `install_name_tool` rewrites)
  on the Mac that runs it.
- **Static builds** (static-php-cli) avoid the dylib problem, but cannot load shared extensions, and
  may not ship `php-config` and `phpize`, which the runtimes design's prefix check requires and pecl
  needs.

## 3. Questions to answer when this is picked up

1. Download, compile, or both? Compiling on each Mac avoids relocation and dylib drift but takes
   minutes and needs the toolchain (Xcode's command line tools, autoconf, bison, re2c, pkg-config).
2. For a compile: does a recipe carry configure flags, brew dependencies and patches, and who keeps
   the patches for the old lines working as OpenSSL and ICU move? Can shivammathur's formula
   patches be reused, and under what licence (the tap is MIT)?
3. For a download: static only (no shared extensions, pecl impossible), or a relocatable dynamic
   build with its dylibs bundled and rewritten? Where is it hosted, and is its hash pinned in the
   recipe (the engine recipes' rule) or fetched?
4. Which OpenSSL does an old build link: the bundled one (and then whose security updates), or
   brew's current one with patches?
5. Extensions for such a build: pecl only, as for a `path` build, or built alongside it?
6. Does a build Yard installed get upgraded (a new 7.4 patch release is impossible; a new 8.4 patch
   is not), and how does a site move from one to the next?

## 4. What the runtimes design already gives it

The build name and its paths (`php/<build>/`, `run/php-<build>.sock`, `php-fpm-<build>`), the build
file in `~/.yard/runtimes/php/<build>.toml` with a `[source] kind` that today accepts only `path`,
`installs/<key>/<version>/` as the place an installed program lives, the download source's
curl-stage-hash-rename install, `state/php-builds.json`'s checked version, and the
`EXT_NOT_FOR_BUILD` rule for brew's extension formulas.
