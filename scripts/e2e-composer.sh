#!/usr/bin/env bash
# Runtimes end-to-end check on a real Mac, after e2e-m1.sh passes: Composer per project, with
# the explicit defaults (explicit-defaults spec).
#
# Checks a Composer a published advisory covers (2.7.7) is refused, then installs the current 2.x
# and checks yardd writes it as the default and that linking an 8.4 project (a committed .yard)
# runs it.
# Then links a second project with no .yard, isolates it to PHP 7.1, and checks that `yard runtime
# install composer` there installs 2.2 and that yardd pins that site to it (no default moves).
# Then: composer install works in both, a [runtimes] pin in the 8.4 project's .yard wins, the
# committed .yard moved to 7.1 with no pin is refused with the line to add, self-update is refused
# in every spelling and the phar is read-only, and `yard runtime uninstall` refuses the default
# and a version a site runs. It needs yardd running (built from this checkout), brew's php@7.1 and
# php@8.4 (it installs no PHP), and the network. yard.toml and sites.toml are backed up and put
# back, only the Composer versions this run installed are removed, and the run fails if Herd's
# resolver changed. Composer's home and cache go in a temp dir, so ~/.composer is not touched.
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
# let link skip the install this checks, and is the maintainer's: it is never removed. A
# written Composer default would keep yardd from writing the one this run checks.
for v in 7.1 8.4; do
  [[ -x $brew_prefix/opt/php@$v/bin/php ]] || fail "PHP $v is not installed; run brew install shivammathur/php/php@$v first"
done
[[ -z "$(installed)" ]] || fail "Composer $(installed | tr '\n' ' ')is installed in $installs; this run checks the install, so move them away first"
if grep -q '^\[runtimes\.composer\]' "$data/yard.toml" 2>/dev/null; then
  fail "$data/yard.toml has a [runtimes.composer] default; move it away first"
fi
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
# The old project has no .yard: it is isolated after linking, so Yard can pin its Composer.
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
  # Every version is this run's: step 1 refused to start over any other. They go before the
  # configs come back: with one installed, putting back a yard.toml with no Composer default
  # would have yardd write one into it. The default's version is refused by uninstall.
  for v in $(installed); do
    "$yard" runtime uninstall composer "$v" >/dev/null 2>&1 \
      || { chmod -R u+w "${installs:?}/$v" && rm -rf "${installs:?}/$v"; }
  done
  for f in yard.toml sites.toml; do
    if [[ -f $work/$f.bak ]] && ! cmp -s "$work/$f.bak" "$data/$f"; then cp -p "$work/$f.bak" "$data/$f"; fi
  done
}
trap 'cleanup; rm -rf "$work"' EXIT

# The [runtimes.composer] default written in yard.toml, or nothing.
composer_default() { sed -n '/^\[runtimes\.composer\]/,/^\[/s/^default = "\(.*\)"/\1/p' "$data/yard.toml"; }
# Waits up to 10 s for command $@ to succeed.
within() { local _; for _ in $(seq 1 50); do if "$@"; then return 0; fi; sleep 0.2; done; return 1; }

# 3. A Composer with a published advisory is refused before anything is fetched; the current
# 2.x becomes the written default, and the 8.4 project runs it. 3b checks another install never
# moves it.
if "$yard" runtime install composer 2.7.7 --json >"$work/install-flawed.out" 2>&1; then
  fail "yard runtime install composer 2.7.7 succeeded: $(cat "$work/install-flawed.out")"
fi
grep -q 'known security flaw' "$work/install-flawed.out" || fail "the refusal said: $(cat "$work/install-flawed.out")"
[[ ! -e $installs/2.7.7 ]] || fail "the refused 2.7.7 left $installs/2.7.7"
pass "Composer 2.7.7, which a published advisory covers, is refused and nothing is installed"
"$yard" runtime install composer 2 >"$work/install-current.out" 2>&1 \
  || fail "yard runtime install composer 2 failed: $(cat "$work/install-current.out")"
dflt="$(installed | grep -v '^2\.2\.' | head -n 1)"
[[ -n $dflt ]] || fail "yard runtime install composer 2 installed nothing: $(cat "$work/install-current.out")"
grep -q "wrote \[runtimes.composer\] default = \"$dflt\"" "$work/install-current.out" \
  || fail "the install did not say it wrote the default: $(cat "$work/install-current.out")"
[[ $(composer_default) == "$dflt" ]] || fail "yard.toml's Composer default is '$(composer_default)', expected $dflt"
pass "installing Composer $dflt wrote it as the default, and said so"
(cd "$work/$new" && "$yard" link "$new" --yes >"$work/link-$new.out" 2>&1) \
  || fail "yard link $new --yes failed: $(cat "$work/link-$new.out")"
v="$(composer $new -V 2>&1)"
[[ $v == *"Composer version $dflt "* ]] || fail "composer -V on PHP 8.4 printed '$v', expected the default $dflt"
pass "the 8.4 project runs the default, Composer $dflt"

# 3b. A project on PHP 7.1 gets its own pin when its Composer is installed, and no default moves.
(cd "$work/$old" && "$yard" link "$old" --yes >"$work/link-$old.out" 2>&1) \
  || fail "yard link $old --yes failed: $(cat "$work/link-$old.out")"
"$yard" isolate 7.1 --site "$old" >/dev/null || fail "yard isolate 7.1 --site $old failed"
(cd "$work/$old" && "$yard" runtime install composer >"$work/install-lts.out" 2>&1) \
  || fail "yard runtime install composer in $old failed: $(cat "$work/install-lts.out")"
lts="$(installed | grep '^2\.2\.' | head -n 1)"
[[ -n $lts ]] || fail "yard runtime install composer under PHP 7.1 installed no 2.2: $(cat "$work/install-lts.out")"
pinned() { grep -q "composer = \"$lts\"" "$data/sites.toml"; }
within pinned || fail "sites.toml has no composer = \"$lts\" for $old: $(cat "$data/sites.toml")"
grep -q "for site $old" "$work/install-lts.out" || fail "the install did not say it pinned $old: $(cat "$work/install-lts.out")"
[[ $(composer_default) == "$dflt" ]] || fail "installing $lts moved the default to '$(composer_default)'"
v="$(composer $old -V 2>&1)"
[[ $v == *"Composer version $lts "* ]] || fail "composer -V on PHP 7.1 printed '$v', expected its pin $lts"
v="$(composer $new -V 2>&1)"
[[ $v == *"Composer version $dflt "* ]] || fail "after installing $lts, composer -V on PHP 8.4 printed '$v'"
pass "PHP 7.1's project is pinned to Composer $lts, and the default, and the 8.4 project, stayed on $dflt"
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

# 5. A pin in the 8.4 project's .yard moves it to 2.2; moved to PHP 7.1 with no pin, the
# committed .yard is refused with the line to add, and never edited.
printf 'php = "8.4"\n\n[runtimes]\ncomposer = "2.2"\n' >"$work/$new/.yard"
v="$(composer $new -V 2>&1)"
[[ $v == *"Composer version 2.2."* ]] || fail "with [runtimes] composer = \"2.2\", composer -V printed '$v'"
pass "[runtimes] composer = \"2.2\" runs Composer 2.2 on PHP 8.4"
printf 'php = "7.1"\n' >"$work/$new/.yard"
if composer $new -V >"$work/refused.out" 2>&1; then fail "composer ran Composer $dflt on PHP 7.1: $(cat "$work/refused.out")"; fi
grep -q "add \[runtimes\] composer = \"$lts\" to " "$work/refused.out" \
  || fail "the refusal named no line to add: $(cat "$work/refused.out")"
[[ $(cat "$work/$new/.yard") == 'php = "7.1"' ]] || fail "the committed .yard was edited: $(cat "$work/$new/.yard")"
pass "a committed .yard on PHP 7.1 with no pin is refused with the line to add, and left alone"
printf 'php = "8.4"\n' >"$work/$new/.yard"

# 6. self-update is refused in every spelling, and the phar is read-only besides.
for args in "self-update" "-n self-update" "--no-ansi selfupdate" "-d /tmp self-update" "self-u" "global self-update" \
  "-rd self-update" "-dd d self-update"; do
  # shellcheck disable=SC2086 # the words are the arguments
  if composer $new $args >"$work/update.out" 2>&1; then fail "composer $args succeeded: $(cat "$work/update.out")"; fi
  grep -q 'managed by Yard' "$work/update.out" || fail "composer $args said: $(cat "$work/update.out")"
done
# global runs the rest again from one string, splitting an option word at its spaces.
if composer $new global "-n self-update" >"$work/update.out" 2>&1; then fail "composer global '-n self-update' succeeded: $(cat "$work/update.out")"; fi
grep -q 'managed by Yard' "$work/update.out" || fail "composer global '-n self-update' said: $(cat "$work/update.out")"
for phar in "$installs"/*/composer.phar; do
  [[ ! -w $phar ]] || fail "$phar is writable"
done
pass "composer self-update is refused in every spelling, and the phar is read-only"

# 7. yard runtime uninstall refuses the default and the version a site runs.
if "$yard" runtime uninstall composer "$dflt" --json >"$work/uninstall-default.out" 2>&1; then
  fail "yard runtime uninstall composer $dflt succeeded while it is the default"
fi
grep -q 'RUNTIME_IN_USE' "$work/uninstall-default.out" || fail "yard runtime uninstall said: $(cat "$work/uninstall-default.out")"
pass "yard runtime uninstall composer $dflt is refused while it is the default"
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
