#!/usr/bin/env bash
# M2 end-to-end check on a real Mac, beside a running Herd: two PHP versions side by
# side, extensions, and the shims.
#
# Needs what e2e-m1.sh needs, plus brew installs it makes itself (ask first):
# shivammathur/php/php@7.1 and the redis@7.1 and xdebug@8.4 extension formulas, plus
# a pecl build of apcu 5.1.24 into brew's PHP 8.4 extension dir. It links two temp
# projects, adds three extensions, and removes them again; PHP 7.1 stays
# installed. Herd's resolver must come out unchanged. PHP is only ever run through
# Yard's shim by absolute path: a bare `php` here is Herd's.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
root="$HOME/.yard"
shim="$root/bin/php"
herd_resolver=/etc/resolver/test

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
# Runs a command quietly, but shows its output when it fails.
quiet() { local out; out="$("$@" 2>&1)" || { printf '%s\n' "$out" >&2; fail "$*"; }; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# The body of $1 once it matches the glob $2 (nginx reloads a moment after a command).
answer() {
  local body=""
  for _ in $(seq 50); do
    body="$(curl -fsS "$1" 2>/dev/null || true)"
    # shellcheck disable=SC2053
    [[ $body == $2 ]] && break
    sleep 0.2
  done
  printf '%s' "$body"
}

# The pid of yard unit $1.
unit_pid() {
  "$yard" status --json | python3 -c 'import json, sys; print(next((u["pid"] for u in json.load(sys.stdin)["units"] if u["name"] == sys.argv[1]), ""))' "$1"
}

# brew's conf.d for the two versions this run adds extensions to. brew uninstall removes an
# extension's keg but leaves its ini there, still loading the .so it removed, so brew's own
# php warns on every run; cleanup removes the ones this run added.
brew_confd=(/opt/homebrew/etc/php/7.1/conf.d /opt/homebrew/etc/php/8.4/conf.d)
brew_inis() { local d; for d in "${brew_confd[@]}"; do find "$d" -maxdepth 1 -name '*.ini' 2>/dev/null; done | sort; }
# The absolute .so ini $1 loads; nothing for a bare name, which is PHP's to find.
ini_so() { sed -n 's/^[[:space:]]*\(zend_\)\{0,1\}extension[[:space:]]*=[[:space:]]*"\{0,1\}\(\/[^"]*\)"\{0,1\}.*$/\2/p' "$1" | head -1; }
inis_taken=false
inis_before=""

# `php -m` through the shim, in project $1.
modules() { (cd "$1" && "$shim" -m); }
# has_module <dir> <module> <what>: fails showing all of php -m's output when it is missing.
has_module() {
  local m
  m="$(modules "$1" 2>&1)"
  grep -qx "$2" <<<"$m" || { printf '%s\n' "$m" >&2; fail "$2 is not loaded in $3"; }
}

# 1. Herd's resolver, before.
before="$(sum "$herd_resolver")"

# 2. PHP 7.1, then the two fixtures in a temp dir.
if "$yard" php list --json | python3 -c 'import json, sys; sys.exit(0 if any(p["version"] == "7.1" and p["installed"] for p in json.load(sys.stdin)) else 1)'; then
  pass "PHP 7.1 is already installed"
else
  "$yard" php install 7.1 >/dev/null
  pass "installed PHP 7.1"
fi
work="$(mktemp -d /tmp/yard-e2e-XXXXXX)"
cp -R "$fixtures/hello-php" "$fixtures/hello-php71" "$work/"

added=()
cleanup() {
  local ext
  # The +-form expands to nothing for an empty array; macOS bash 3.2 calls a bare one unbound.
  for ext in ${added[@]+"${added[@]}"}; do
    # shellcheck disable=SC2086
    "$yard" ext remove $ext >/dev/null 2>&1 || true
  done
  if [[ $inis_taken == true ]]; then
    local ini so
    while IFS= read -r ini; do
      so="$(ini_so "$ini")"
      if [[ -n $so && ! -e $so ]]; then rm -f "$ini"; fi
    done < <(comm -13 <(printf '%s\n' "$inis_before") <(brew_inis))
  fi
  "$yard" unlink hello-php >/dev/null 2>&1 || true
  "$yard" unlink hello-php71 >/dev/null 2>&1 || true
  local stub
  for stub in "$root/nginx/overrides/"{hello-php,hello-php71}.conf; do
    if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  done
  rm -rf "$work"
}
trap cleanup EXIT

# 3. Link both; isolate the second to 7.1.
(cd "$work/hello-php" && "$yard" link hello-php >/dev/null)
(cd "$work/hello-php71" && "$yard" link hello-php71 >/dev/null && "$yard" isolate 7.1 >/dev/null)
pass "linked hello-php and hello-php71, isolated to 7.1"

# 4. Each site runs its own version.
v84="$(answer http://hello-php.yard:8080/ "8.4.*")"
[[ $v84 == 8.4.* ]] || fail "http://hello-php.yard:8080/ answered '$v84', expected 8.4.*"
pass "http://hello-php.yard:8080/ runs PHP $v84"
v71="$(answer http://hello-php71.yard:8080/ "7.1.*")"
[[ $v71 == 7.1.* ]] || fail "http://hello-php71.yard:8080/ answered '$v71', expected 7.1.*"
pass "http://hello-php71.yard:8080/ runs PHP $v71"

# 5. Extensions load, seen through the shim in each project.
[[ -x $shim ]] || fail "$shim is not linked; is yardd the release build next to yard-shim?"
inis_before="$(brew_inis)"
inis_taken=true
# Registered before the add, so cleanup removes even an add that failed halfway.
added+=("redis --php 7.1")
quiet "$yard" ext add redis --php 7.1
added+=("xdebug --php 8.4")
quiet "$yard" ext add xdebug --php 8.4
has_module "$work/hello-php71" redis "PHP 7.1"
pass "redis loads in PHP 7.1"
has_module "$work/hello-php" Xdebug "PHP 8.4"
has_module "$work/hello-php" 'Zend OPcache' "PHP 8.4"
pass "Xdebug and Zend OPcache load in PHP 8.4"

# A pinned release skips brew and builds with pecl, which must leave brew's php.ini alone.
brew_ini=/opt/homebrew/etc/php/8.4/php.ini
ini_before="$(shasum -a 256 "$brew_ini")"
added+=("apcu --php 8.4")
quiet "$yard" ext add apcu-5.1.24 --php 8.4
has_module "$work/hello-php" apcu "PHP 8.4"
[[ $(shasum -a 256 "$brew_ini") == "$ini_before" ]] || fail "pecl edited $brew_ini"
pass "apcu builds with pecl and loads in PHP 8.4; brew's php.ini is unchanged"

# 5a. The binaries are brew's, never Herd's.
for site in hello-php:8.4 hello-php71:7.1; do
  dir="${site%%:*}" v="${site##*:}"
  binary="$(cd "$work/$dir" && "$shim" -r 'echo PHP_BINARY;')"
  [[ $binary == /opt/homebrew/* && $binary != *Herd* ]] || fail "the shim in $dir ran $binary"
  pass "the shim in $dir runs $binary"
  pid="$(unit_pid "php-fpm-$v")"
  [[ -n $pid ]] || fail "php-fpm-$v is not running"
  # php-fpm rewrites its process title, so ask for the file it executes.
  exe="$(lsof -a -p "$pid" -d txt -Fn | sed -n 's/^n//p' | head -1)"
  [[ $exe == /opt/homebrew/Cellar/php@$v/*/sbin/php-fpm ]] || fail "php-fpm-$v runs '$exe'"
  pass "php-fpm-$v (pid $pid) runs $exe"
done

# 6. Xdebug off.
"$yard" xdebug off --php 8.4 >/dev/null
! modules "$work/hello-php" | grep -qx Xdebug || fail "Xdebug still loads after yard xdebug off"
pass "yard xdebug off unloads it"

# 7. Doctor has no failing check.
report="$work/doctor.json"
"$yard" doctor --json >"$report" || true
python3 - "$report" <<'PY' || fail "yard doctor reports failures"
import json, sys
failed = [c for c in json.load(open(sys.argv[1]))["checks"] if c["status"] == "fail"]
for c in failed:
    print(f"  {c['id']}: {c['message']} -> {c.get('hint')}", file=sys.stderr)
sys.exit(1 if failed else 0)
PY
pass "yard doctor has no failing check"

# 8. Clean up, then Herd's resolver, after.
trap - EXIT
cleanup
after="$(sum "$herd_resolver")"
[[ $before == "$after" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
while IFS= read -r ini; do
  so="$(ini_so "$ini")"
  if [[ -n $so && ! -e $so ]]; then fail "$ini still loads $so, which is gone"; fi
done < <(comm -13 <(printf '%s\n' "$inis_before") <(brew_inis))
pass "brew's conf.d keeps no ini this run added for a removed extension"
pass "PHP 7.1 stays installed; yard php uninstall 7.1 removes it"
