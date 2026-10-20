#!/usr/bin/env bash
# M2 end-to-end check on a real Mac, beside a running Herd: two PHP versions side by
# side, extensions, and the shims.
#
# Needs what e2e-m1.sh needs, plus brew installs it makes itself (ask first):
# shivammathur/php/php@7.1 and the redis@7.1 and xdebug@8.4 extension formulas. It
# links two temp projects, adds two extensions, and removes both again; PHP 7.1 stays
# installed. Herd's resolver must come out unchanged. PHP is only ever run through
# Yard's shim by absolute path: a bare `php` here is Herd's.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
root="$HOME/Library/Application Support/Yard"
shim="$root/bin/php"
herd_resolver=/etc/resolver/test

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
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

# `php -m` through the shim, in project $1.
modules() { (cd "$1" && "$shim" -m); }

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
  for ext in "${added[@]}"; do
    # shellcheck disable=SC2086
    "$yard" ext remove $ext >/dev/null 2>&1 || true
  done
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
"$yard" ext add redis --php 7.1 >/dev/null
added+=("redis --php 7.1")
"$yard" ext add xdebug --php 8.4 >/dev/null
added+=("xdebug --php 8.4")
modules "$work/hello-php71" | grep -qx redis || fail "redis is not loaded in PHP 7.1"
pass "redis loads in PHP 7.1"
m84="$(modules "$work/hello-php")"
grep -qx Xdebug <<<"$m84" || fail "Xdebug is not loaded in PHP 8.4"
grep -qx 'Zend OPcache' <<<"$m84" || fail "Zend OPcache is not loaded in PHP 8.4"
pass "Xdebug and Zend OPcache load in PHP 8.4"

# 5a. The binaries are brew's, never Herd's.
for site in hello-php:8.4 hello-php71:7.1; do
  dir="${site%%:*}" v="${site##*:}"
  binary="$(cd "$work/$dir" && "$shim" -r 'echo PHP_BINARY;')"
  [[ $binary == /opt/homebrew/* && $binary != *Herd* ]] || fail "the shim in $dir ran $binary"
  pass "the shim in $dir runs $binary"
  pid="$(unit_pid "php-fpm-$v")"
  [[ -n $pid ]] || fail "php-fpm-$v is not running"
  command="$(ps -o command= -p "$pid")"
  [[ $command == /opt/homebrew/opt/php@$v/sbin/php-fpm* ]] || fail "php-fpm-$v is '$command'"
  pass "php-fpm-$v (pid $pid) is brew's"
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
pass "PHP 7.1 stays installed; yard php uninstall 7.1 removes it"
