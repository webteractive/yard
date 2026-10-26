#!/usr/bin/env bash
# M4e end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Adds a scratch `redis` instance that sleeps after 60s without use, links a scratch project
# whose .yard lists it, and checks the whole loop on the real yardd: the instance falls
# asleep and nothing listens on its port; a page wakes it (PHP asks redis PING and prints
# PONG); `php` through the shim wakes it and says so; a held redis-cli connection keeps it
# awake. While it is awake it times 50 wake asks on run/wake.sock and prints their p50 (the
# M4e spec's target is under 1 ms). Then it removes what it added.
#
# Needs: yardd running, built from this tree (e2e-m1.sh), brew's `redis` (installed for
# e2e-m4b.sh) and jq. Installs nothing. It runs for about five minutes, most of it waiting
# for yardd's 30 s idle samples. Ask the maintainer first.
#
# It stops before changing anything if `redis` is already an instance: it removes only what
# it added. yard.toml, services.toml, ports.toml and sites.toml are backed up and put back if
# they differ. It never touches DBngin, Postgres.app or Herd: the listeners on 3306, 5432 and
# 6379 and Herd's resolver must come out unchanged. The port is read from
# `yard services --json`, never assumed.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
site="yard-e2e-sleep"
instance=redis
foreign_ports=(3306 5432 6379)
# yardd samples every 30 s and the instance sleeps after 60 s, so it is asleep within 90 s
# of its last use; this leaves room for the stop.
sleep_wait=150

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# What listens on each port DBngin, Postgres.app or another database may hold: pid and
# command, one line per port, in a stable order.
listeners() {
  local port
  for port in "${foreign_ports[@]}"; do
    printf '%s: %s\n' "$port" "$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpc 2>/dev/null | grep -E '^[pc]' | sort -u | tr '\n' ' ')"
  done
}

services_json() { "$yard" services --json; }

# The instance's entry in `yard services --json`, through a jq filter.
instance_field() {
  services_json | jq -r --arg n "$instance" ".[] | select(.name == \$n) | $1"
}

state() { instance_field '.state | if type == "object" then keys[0] else . end'; }

# True once yardd reports the instance asleep and has listed the site in run/sleeping,
# which it writes only after the process is gone.
asleep() {
  [[ $(instance_field '.sleeping_since // ""') != "" ]] && grep -qx "$site" "$data/run/sleeping" 2>/dev/null
}

wait_asleep() {
  local deadline=$((SECONDS + sleep_wait))
  until asleep; do
    ((SECONDS < deadline)) || fail "$instance did not fall asleep within ${sleep_wait}s (state $(state))"
    sleep 2
  done
}

# 1. What this needs, and what must not change, before anything is touched.
[[ -x $yard ]] || fail "$yard is not built; run cargo build --release"
command -v brew >/dev/null || fail "brew is not installed"
command -v jq >/dev/null || fail "jq is not installed (brew install jq)"
"$yard" status --json >/dev/null 2>&1 || fail "yardd is not running; run yard start and scripts/e2e-m1.sh first"
redis_cli="$(brew --prefix)/opt/redis/bin/redis-cli"
[[ -x $redis_cli ]] || fail "$redis_cli is not there; brew's redis comes from e2e-m4b.sh"

if services_json | jq -e --arg n "$instance" '.[] | select(.name == $n)' >/dev/null; then
  fail "$instance is already a service instance; this script removes what it adds, so it will not run over it. Ask the maintainer"
fi
if "$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null; then
  fail "a site named $site already exists"
fi

resolver_before="$(sum "$herd_resolver")"
listeners_before="$(listeners)"
echo "listeners before:"
printf '  %s\n' "${listeners_before//$'\n'/$'\n'  }"

# 2. Back up the four TOMLs. A file that does not exist yet is noted, not backed up.
work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-XXXXXX")"
tomls=(yard.toml services.toml ports.toml sites.toml)
for f in "${tomls[@]}"; do
  if [[ -f $data/$f ]]; then cp -p "$data/$f" "$work/$f.bak"; else : >"$work/$f.absent"; fi
done

added=0
holder=""
cleanup_failed=0

cleanup() {
  local stub f
  [[ -n $holder ]] && kill "$holder" 2>/dev/null || true
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  if [[ $added == 1 ]] && services_json 2>/dev/null | jq -e --arg n "$instance" '.[] | select(.name == $n)' >/dev/null 2>&1; then
    if ! "$yard" service remove "$instance" --yes >/dev/null 2>&1; then
      echo "FAIL could not remove $instance; remove it with yard service remove $instance --yes" >&2
      cleanup_failed=1
    fi
  fi
  # The override stub Yard created for the site, unless someone has edited it.
  stub="$data/nginx/overrides/$site.conf"
  if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  for f in "${tomls[@]}"; do
    # An instance that could not be removed keeps its entries: putting the old file back
    # would orphan a running process.
    if [[ $cleanup_failed == 1 && ($f == services.toml || $f == ports.toml) ]]; then continue; fi
    if [[ -f $work/$f.bak ]]; then
      cmp -s "$work/$f.bak" "$data/$f" || cp -p "$work/$f.bak" "$data/$f"
    elif [[ -f $data/$f ]] && ! grep -q '^\[' "$data/$f"; then
      # The file was created by this run and holds no table: put it back to absent.
      rm -f "$data/$f"
    fi
  done
  rm -rf "$work"
  return "$cleanup_failed"
}
trap cleanup EXIT

# 3. A redis instance that sleeps after 60s.
added=1
"$yard" service add redis --yes --json >"$work/add.out" 2>&1 || fail "yard service add redis failed: $(tail -c 600 "$work/add.out")"
port="$(instance_field '.ports[0].port')"
[[ -n $port ]] || fail "yard services lists no port for $instance"
for foreign in "${foreign_ports[@]}"; do
  if [[ $port == "$foreign" ]] && [[ -n $(grep "^$foreign: ." <<<"$listeners_before" || true) ]]; then
    fail "$instance took port $foreign, which another process held before this run"
  fi
done
"$yard" service sleep-after "$instance" 60s >/dev/null
[[ $(instance_field '.sleep_after_secs') == 60 ]] || fail "yard services does not show sleep_after 60 for $instance"
pass "added $instance on port $port, sleeping after 60s"

# 4. A scratch project, outside ~/Herd, whose .yard asks for redis and whose page asks redis PING.
cp -R "$fixtures/hello-php" "$work/$site"
printf '[services]\nredis = true\n' >"$work/$site/.yard"
cat >"$work/$site/public/index.php" <<PHP
<?php
\$redis = @fsockopen('127.0.0.1', $port, \$errno, \$error, 2);
if (!\$redis) { http_response_code(503); echo "no redis: \$error"; exit; }
fwrite(\$redis, "PING\r\n");
echo trim(fgets(\$redis), "+\r\n");
PHP
(cd "$work/$site" && "$yard" link "$site" >/dev/null)
pass "linked $site, whose .yard lists redis"

# 5. It falls asleep, and nothing listens on its port.
wait_asleep
[[ -z $(lsof -nP -iTCP:"$port" -sTCP:LISTEN 2>/dev/null || true) ]] || fail "something still listens on $port while $instance sleeps"
[[ $(state) == stopped ]] || fail "a sleeping $instance is $(state) on the wire, expected stopped"
pass "$instance fell asleep, and nothing listens on $port"

# 6. A page wakes it: nginx asks yardd before PHP runs, and PHP's PING gets PONG.
page="$(curl -s --max-time 20 "http://$site.yard:8080/")"
[[ $page == PONG ]] || fail "http://$site.yard:8080/ answered '$page', expected PONG"
[[ $(state) == running ]] || fail "$instance is $(state) after the page, expected running"
pass "a request to http://$site.yard:8080/ woke $instance and PHP got PONG"

# 7. The cost of an ask while everything is awake: the p50 of 50 asks on wake.sock.
for _ in $(seq 50); do
  curl -s -o /dev/null -w '%{time_total}\n' --unix-socket "$data/run/wake.sock" "http://yard/wake/$site?wait=10"
done | sort -n | awk '{t[NR] = $1} END {printf "ok   a wake ask while awake takes %.2f ms at p50 (target < 1 ms)\n", t[int((NR + 1) / 2)] * 1000}'

# 8. php through the shim wakes it, and says so.
wait_asleep
php_err="$work/php.err"
out="$(cd "$work/$site" && "$data/bin/php" -r 'echo 1;' 2>"$php_err")" || fail "php through the shim failed: $(cat "$php_err")"
[[ $out == 1 ]] || fail "php through the shim printed '$out', expected 1"
grep -q "waking" "$php_err" || fail "php through the shim did not say it was waking: $(cat "$php_err")"
[[ $(state) == running ]] || fail "$instance is $(state) after php, expected running"
pass "php in $site woke $instance and said so"

# 9. A held connection keeps it awake: a SUBSCRIBE holds one open until it is killed.
"$redis_cli" -h 127.0.0.1 -p "$port" subscribe yard-e2e >/dev/null 2>&1 &
holder=$!
sleep 100
[[ $(state) == running && $(instance_field '.sleeping_since // ""') == "" ]] || fail "$instance fell asleep while a client held a connection"
kill "$holder" 2>/dev/null || true
# Reaped here, so bash prints no job notice for it.
wait "$holder" 2>/dev/null || true
holder=""
pass "a held redis-cli connection kept $instance awake for 100 s"

# 10. Clean up: unlink, remove the instance, put the TOMLs back; then the foreign listeners
# and Herd's resolver, after.
trap - EXIT
cleanup || fail "the cleanup could not remove everything it added"
if services_json | jq -e --arg n "$instance" '.[] | select(.name == $n)' >/dev/null; then
  fail "$instance is still an instance after its removal"
fi
pass "unlinked $site and removed $instance"
listeners_after="$(listeners)"
[[ $listeners_before == "$listeners_after" ]] || fail "the listeners on ${foreign_ports[*]} changed: before
$listeners_before
after
$listeners_after"
pass "the listeners on ${foreign_ports[*]} are unchanged"
[[ $resolver_before == "$(sum "$herd_resolver")" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
