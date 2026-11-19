#!/usr/bin/env bash
# Runtimes plan 1 end-to-end check on a real Mac, after e2e-m1.sh passes: Composer per project.
#
# Links two scratch projects (never under ~/Herd), one whose .yard pins PHP 7.1 and one PHP
# 8.4, each with a composer.json, and checks that `yard link --yes` offers and installs the
# Composer each PHP runs from getcomposer.org (2.2 for 7.1, the current 2.x for 8.4), that
# Yard's composer shim runs it and `composer install` works in both, that [runtimes]
# composer = "2.2" in the 8.4 project's .yard moves it to 2.2, that self-update is refused
# even after an option, and that `yard runtime uninstall` refuses a version a site runs. It
# needs yardd running (built from this checkout), brew's php@7.1 and php@8.4 (it installs no
# PHP), and the network. yard.toml and sites.toml are backed up and put back, only the
# Composer versions this run installed are removed, and the run fails if Herd's resolver
# changed. Composer's home and cache go in a temp dir, so ~/.composer is not touched.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
brew_prefix="$(brew --prefix)"
herd_resolver=/etc/resolver/test
installs="$data/installs/composer"
old=e2e-composer-old
new=e2e-composer-new

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
# The file's checksum, or "absent": on a Mac without Herd its resolver must stay absent.
sum() { if [[ -e $1 ]]; then shasum -a 256 "$1" | cut -d' ' -f1; else echo absent; fi; }
# The installed Composer versions, one per line, or nothing.
installed() {
  local d
  for d in "$installs"/*/; do
    if [[ -d $d ]]; then d="${d%/}"; echo "${d##*/}"; fi
  done
}
# Yard's composer shim, run in project $1.
composer() { (cd "$work/$1" && "$data/bin/composer" "${@:2}"); }

# 1. What this needs, and what must come out unchanged. A Composer already installed would
# let link skip the install this checks, and is the maintainer's: it is never removed.
for v in 7.1 8.4; do
  [[ -x $brew_prefix/opt/php@$v/bin/php ]] || fail "PHP $v is not installed; run brew install shivammathur/php/php@$v first"
done
[[ -z "$(installed)" ]] || fail "Composer $(installed | tr '\n' ' ')is installed in $installs; this run checks the install, so move them away first"
for s in $old $new; do
  [[ ! -e $data/sites/$s ]] || fail "$data/sites/$s already exists; move it away first"
done
work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-XXXXXX")"
herd_before="$(sum "$herd_resolver")"
export COMPOSER_HOME="$work/composer-home" COMPOSER_CACHE_DIR="$work/composer-cache"

# 2. The two projects, and the configs backed up.
for s in $old $new; do
  cp -R "$fixtures/hello-php" "$work/$s"
  printf '{"name": "yard/%s", "require": {}}\n' "$s" >"$work/$s/composer.json"
done
printf 'php = "7.1"\n' >"$work/$old/.yard"
printf 'php = "8.4"\n' >"$work/$new/.yard"
for f in yard.toml sites.toml; do
  if [[ -f $data/$f ]]; then cp -p "$data/$f" "$work/$f.bak"; fi
done

cleanup() {
  local s f v stub
  for s in $old $new; do
    "$yard" unlink "$s" >/dev/null 2>&1 || true
    # The override stub Yard created for the site, unless someone has edited it.
    stub="$data/nginx/overrides/$s.conf"
    if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
    rm -rf "${data:?}/sites/$s"
  done
  for f in yard.toml sites.toml; do
    if [[ -f $work/$f.bak ]] && ! cmp -s "$work/$f.bak" "$data/$f"; then cp -p "$work/$f.bak" "$data/$f"; fi
  done
  # Every version is this run's: step 1 refused to start over any other.
  for v in $(installed); do
    "$yard" runtime uninstall composer "$v" >/dev/null 2>&1 || rm -rf "${installs:?}/$v"
  done
}
trap 'cleanup; rm -rf "$work"' EXIT

# 3. yard link --yes offers Composer and installs the one each PHP runs.
for s in $old $new; do
  (cd "$work/$s" && "$yard" link "$s" --yes >"$work/link-$s.out" 2>&1) \
    || fail "yard link $s --yes failed: $(cat "$work/link-$s.out")"
  grep -q 'yard runtime install composer' "$work/link-$s.out" || fail "yard link $s offered no Composer: $(cat "$work/link-$s.out")"
done
v="$(composer $old -V 2>&1)"
[[ $v == *"Composer version 2.2."* ]] || fail "composer -V on PHP 7.1 printed '$v', expected 2.2.x"
pass "PHP 7.1's project runs Composer 2.2: ${v%% 20*}"
v="$(composer $new -V 2>&1)"
[[ $v == *"Composer version 2."* && $v != *"Composer version 2.2."* ]] || fail "composer -V on PHP 8.4 printed '$v', expected 2.x after 2.2"
pass "PHP 8.4's project runs the current Composer: ${v%% 20*}"
for f in "$installs"/*/.yard-install.toml; do
  [[ -f $f ]] || fail "no .yard-install.toml in $(dirname "$f")"
done
pass "each installed version has its .yard-install.toml: $(installed | tr '\n' ' ')"

# 4. composer install works in both.
for s in $old $new; do
  composer "$s" install --no-interaction >"$work/install-$s.log" 2>&1 \
    || fail "composer install in $s failed: $(tail -n 20 "$work/install-$s.log")"
  [[ -f $work/$s/vendor/autoload.php ]] || fail "composer install in $s wrote no vendor/autoload.php"
done
pass "composer install works under PHP 7.1 and 8.4"

# 5. A pin in the 8.4 project's .yard moves it to 2.2.
printf 'php = "8.4"\n\n[runtimes]\ncomposer = "2.2"\n' >"$work/$new/.yard"
v="$(composer $new -V 2>&1)"
[[ $v == *"Composer version 2.2."* ]] || fail "with [runtimes] composer = \"2.2\", composer -V printed '$v'"
pass "[runtimes] composer = \"2.2\" runs Composer 2.2 on PHP 8.4"

# 6. self-update is refused, after an option too, and leaves the phar alone.
for args in "self-update" "-n self-update" "--no-ansi selfupdate"; do
  # shellcheck disable=SC2086 # the words are the arguments
  if composer $new $args >"$work/update.out" 2>&1; then fail "composer $args succeeded: $(cat "$work/update.out")"; fi
  grep -q 'managed by Yard' "$work/update.out" || fail "composer $args said: $(cat "$work/update.out")"
done
pass "composer self-update is refused, before and after options"

# 7. yard runtime uninstall refuses the version a site runs.
lts="$(installed | grep '^2\.2\.' | head -n 1)"
if "$yard" runtime uninstall composer "$lts" --json >"$work/uninstall.out" 2>&1; then
  fail "yard runtime uninstall composer $lts succeeded while $old runs it"
fi
grep -q 'RUNTIME_IN_USE' "$work/uninstall.out" || fail "yard runtime uninstall said: $(cat "$work/uninstall.out")"
[[ -d $installs/$lts ]] || fail "the refused uninstall removed $lts"
pass "yard runtime uninstall composer $lts is refused with RUNTIME_IN_USE"

# 8. Clean up, then the configs, the installs and Herd's resolver, after.
trap 'rm -rf "$work"' EXIT
cleanup
for f in yard.toml sites.toml; do
  if [[ -f $work/$f.bak ]]; then cmp -s "$work/$f.bak" "$data/$f" || fail "$data/$f was not restored"; fi
done
pass "yard.toml and sites.toml are restored"
[[ -z "$(installed)" ]] || fail "Composer versions left behind: $(installed | tr '\n' ' ')"
pass "the Composer versions this run installed are removed"
[[ $herd_before == "$(sum "$herd_resolver")" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
