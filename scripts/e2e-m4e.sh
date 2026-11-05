#!/usr/bin/env bash
# M4e end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Adds three scratch instances that sleep after 60s without use, `redis`, `mysql-8.0` and
# `postgres-17`, links a scratch project whose .yard lists them, and checks the whole loop on
# the real yardd with the real engines:
#   - they fall asleep and nothing listens on their ports;
#   - `yard service start` wakes each one, and it prints each engine's cold start (the M4e
#     spec's targets: MySQL 1-3 s, PostgreSQL under 1 s, Redis well under 1 s);
#   - a page wakes all three within nginx's 10 s wait: PHP asks redis PING and runs SELECT 1
#     on MySQL and PostgreSQL through PDO;
#   - `php` through the shim wakes them and says so;
#   - a held client connection keeps each awake (a redis SUBSCRIBE, a MySQL SELECT SLEEP and
#     a PostgreSQL pg_sleep, the last one held by a forked backend, not the postmaster).
# While they are awake it times 50 wake asks on run/wake.sock and prints their p50 (target
# under 1 ms). Then it removes what it added.
#
# Needs: yardd running, built from this tree (e2e-m1.sh), brew's `redis`, `mysql@8.0` and
# `postgresql@17` kegs, and jq. Installs nothing. It runs for about ten minutes, most of it
# waiting for yardd's 30 s idle samples. Ask the maintainer first.
#
# It stops before changing anything if one of the three is already an instance: it removes
# only what it added. yard.toml, services.toml, ports.toml and sites.toml are backed up and
# put back if they differ. It never touches DBngin, Postgres.app, Herd or any other instance:
# the listeners on 3306, 5432 and 6379 and Herd's resolver must come out unchanged. Ports are
# read from `yard services --json`, never assumed.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
site="yard-e2e-sleep"
instances=(redis mysql-8.0 postgres-17)
foreign_ports=(3306 5432 6379)
# yardd samples every 30 s and an instance sleeps after 60 s, so it is asleep within 90 s
# of its last use; this leaves room for the stops.
sleep_wait=150
# nginx's wake ask waits at most 10 s (M4e spec §6.3); a page slower than that has fallen
# through to the app's own connection error.
page_limit=10

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
# The file's checksum, or "absent": on a Mac without Herd its resolver must stay absent.
sum() { if [[ -e $1 ]]; then shasum -a 256 "$1" | cut -d' ' -f1; else echo absent; fi; }

# What listens on each port DBngin, Postgres.app or another database may hold: pid and
# command, one line per port, in a stable order.
listeners() {
  local port
  for port in "${foreign_ports[@]}"; do
    printf '%s: %s\n' "$port" "$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpc 2>/dev/null | grep -E '^[pc]' | sort -u | tr '\n' ' ')"
  done
}

listening() { [[ -n $(lsof -nP -iTCP:"$1" -sTCP:LISTEN 2>/dev/null || true) ]]; }

services_json() { "$yard" services --json; }

# An instance's entry in `yard services --json`, through a jq filter.
field() {
  services_json | jq -r --arg n "$1" ".[] | select(.name == \$n) | $2"
}

state() { field "$1" '.state | if type == "object" then keys[0] else . end'; }

# Each instance's port, read once it is added and kept in the work directory (macOS's bash
# 3.2 has no associative arrays).
port_of() { cat "$work/port-$1"; }

# True once yardd reports every instance asleep, has listed the site in run/sleeping (which
# it writes before a stop begins) and every process is gone, so nothing listens on the ports.
asleep() {
  local i
  grep -qx "$site" "$data/run/sleeping" 2>/dev/null || return 1
  for i in "${instances[@]}"; do
    [[ $(field "$i" '.sleeping_since // ""') != "" ]] || return 1
    ! listening "$(port_of "$i")" || return 1
  done
}

wait_asleep() {
  local deadline=$((SECONDS + sleep_wait)) i
  until asleep; do
    if ((SECONDS >= deadline)); then
      for i in "${instances[@]}"; do echo "  $i: $(state "$i"), sleeping since '$(field "$i" '.sleeping_since // ""')'" >&2; done
      fail "the instances did not all fall asleep within ${sleep_wait}s"
    fi
    sleep 2
  done
}

all_running() {
  local i
  for i in "${instances[@]}"; do
    [[ $(state "$i") == running ]] || fail "$i is $(state "$i") after $1, expected running"
  done
}

# 1. What this needs, and what must not change, before anything is touched.
[[ -x $yard ]] || fail "$yard is not built; run cargo build --release"
command -v brew >/dev/null || fail "brew is not installed"
command -v jq >/dev/null || fail "jq is not installed (brew install jq)"
"$yard" status --json >/dev/null 2>&1 || fail "yardd is not running; run yard start and scripts/e2e-m1.sh first"
prefix="$(brew --prefix)"
redis_cli="$prefix/opt/redis/bin/redis-cli"
mysql_cli="$prefix/opt/mysql@8.0/bin/mysql"
psql_cli="$prefix/opt/postgresql@17/bin/psql"
for client in "$redis_cli" "$mysql_cli" "$psql_cli"; do
  [[ -x $client ]] || fail "$client is not there; brew's redis, mysql@8.0 and postgresql@17 must be installed"
done

for i in "${instances[@]}"; do
  if services_json | jq -e --arg n "$i" '.[] | select(.name == $n)' >/dev/null; then
    fail "$i is already a service instance; this script removes what it adds, so it will not run over it. Ask the maintainer"
  fi
done
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

added=()
holders=()
cleanup_failed=0

stop_holders() {
  local h
  for h in ${holders[@]+"${holders[@]}"}; do kill "$h" 2>/dev/null || true; done
  # Reaped here, so bash prints no job notices for them.
  for h in ${holders[@]+"${holders[@]}"}; do wait "$h" 2>/dev/null || true; done
  holders=()
}

cleanup() {
  local stub f i
  stop_holders
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  for i in ${added[@]+"${added[@]}"}; do
    if services_json 2>/dev/null | jq -e --arg n "$i" '.[] | select(.name == $n)' >/dev/null 2>&1; then
      if ! "$yard" service remove "$i" --yes >/dev/null 2>&1; then
        echo "FAIL could not remove $i; remove it with yard service remove $i --yes" >&2
        cleanup_failed=1
      fi
    fi
  done
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

# 3. The three instances, each sleeping after 60s.
add() {
  local name=$1
  shift
  added+=("$name")
  "$yard" service add "$@" --yes --json >"$work/add-$name.out" 2>&1 ||
    fail "yard service add $* failed: $(tail -c 600 "$work/add-$name.out")"
  local p
  p="$(field "$name" '.ports[0].port')"
  [[ -n $p ]] || fail "yard services lists no port for $name"
  echo "$p" >"$work/port-$name"
  for foreign in "${foreign_ports[@]}"; do
    if [[ $p == "$foreign" ]] && [[ -n $(grep "^$foreign: ." <<<"$listeners_before" || true) ]]; then
      fail "$name took port $foreign, which another process held before this run"
    fi
  done
  "$yard" service sleep-after "$name" 60s >/dev/null
  [[ $(field "$name" '.sleep_after_secs') == 60 ]] || fail "yard services does not show sleep_after 60 for $name"
  pass "added $name on port $p, sleeping after 60s"
}
add redis redis
add mysql-8.0 mysql --version 8.0
add postgres-17 postgres --version 17

# 4. A scratch project, outside ~/Herd, whose .yard asks for all three and whose page uses
# each: redis PING, and SELECT 1 on MySQL and PostgreSQL with the accounts `yard env` prints.
cp -R "$fixtures/hello-php" "$work/$site"
printf '[services]\nredis = true\nmysql = "8.0"\npostgres = "17"\n' >"$work/$site/.yard"
cat >"$work/$site/public/index.php" <<PHP
<?php
\$out = [];
\$redis = @fsockopen('127.0.0.1', $(port_of redis), \$errno, \$error, 2);
\$out[] = 'redis ' . (\$redis && fwrite(\$redis, "PING\r\n") ? trim(fgets(\$redis), "+\r\n") : "down: \$error");
foreach ([
    'mysql' => ['mysql:host=127.0.0.1;port=$(port_of mysql-8.0)', 'root'],
    'postgres' => ['pgsql:host=127.0.0.1;port=$(port_of postgres-17);dbname=postgres', '$USER'],
] as \$name => [\$dsn, \$user]) {
    try {
        \$out[] = "\$name " . (new PDO(\$dsn, \$user, ''))->query('SELECT 1')->fetchColumn();
    } catch (Throwable \$e) {
        \$out[] = "\$name down: " . \$e->getMessage();
    }
}
echo implode("\n", \$out);
PHP
(cd "$work/$site" && "$yard" link "$site" >/dev/null)
"$yard" info "$site" --json >"$work/info.json" 2>&1 || true
pass "linked $site, whose .yard lists redis, mysql 8.0 and postgres 17"
expected_page=$'redis PONG\nmysql 1\npostgres 1'

# 5. They fall asleep, and nothing listens on their ports.
wait_asleep
for i in "${instances[@]}"; do
  [[ $(state "$i") == stopped ]] || fail "a sleeping $i is $(state "$i") on the wire, expected stopped"
done
pass "all three fell asleep, and nothing listens on their ports"

# 6. Each engine's cold start: yard service start waits until the instance passes its health
# check, so its time is the wake.
for i in "${instances[@]}"; do
  start=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  "$yard" service start "$i" >"$work/start-$i.out" 2>&1 || fail "yard service start $i failed: $(cat "$work/start-$i.out")"
  end=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
  listening "$(port_of "$i")" || fail "nothing listens on $(port_of "$i") after yard service start $i"
  awk -v s="$start" -v e="$end" -v i="$i" 'BEGIN {printf "ok   %s woke in %.2f s through yard service start\n", i, e - s}'
done
all_running "yard service start"

# 7. A page wakes all three: nginx asks yardd before PHP runs, and PHP reaches each.
wait_asleep
page="$(curl -s --max-time 30 -w '\n%{time_total}' "http://$site.yard:8080/")"
page_time="${page##*$'\n'}"
page="${page%$'\n'*}"
[[ $page == "$expected_page" ]] || fail "http://$site.yard:8080/ answered:
$page
expected:
$expected_page"
awk -v t="$page_time" -v l="$page_limit" 'BEGIN {exit !(t < l)}' ||
  fail "the waking page took ${page_time}s, over nginx's ${page_limit}s wait"
all_running "the page"
awk -v t="$page_time" 'BEGIN {printf "ok   a request to the site woke all three and PHP reached each, in %.2f s\n", t}'

# 8. The cost of an ask while everything is awake: the p50 of 50 asks on wake.sock.
for _ in $(seq 50); do
  curl -s -o /dev/null -w '%{time_total}\n' --unix-socket "$data/run/wake.sock" "http://yard/wake/$site?wait=10"
done | sort -n | awk '{t[NR] = $1} END {printf "ok   a wake ask while awake takes %.2f ms at p50 (target < 1 ms)\n", t[int((NR + 1) / 2)] * 1000}'

# 9. php through the shim wakes them, and says so.
wait_asleep
php_err="$work/php.err"
out="$(cd "$work/$site" && "$data/bin/php" -r 'echo 1;' 2>"$php_err")" || fail "php through the shim failed: $(cat "$php_err")"
[[ $out == 1 ]] || fail "php through the shim printed '$out', expected 1"
grep -q "waking" "$php_err" || fail "php through the shim did not say it was waking: $(cat "$php_err")"
all_running "php"
pass "php in $site woke all three and said so"

# 10. A held connection keeps each awake: a SUBSCRIBE, a SELECT SLEEP and a pg_sleep each
# hold one open for longer than the check.
"$redis_cli" -h 127.0.0.1 -p "$(port_of redis)" subscribe yard-e2e >/dev/null 2>&1 &
holders+=($!)
"$mysql_cli" -h 127.0.0.1 -P "$(port_of mysql-8.0)" -u root -e 'SELECT SLEEP(150)' >/dev/null 2>&1 &
holders+=($!)
"$psql_cli" -h 127.0.0.1 -p "$(port_of postgres-17)" -d postgres -c 'SELECT pg_sleep(150)' >/dev/null 2>&1 &
holders+=($!)
sleep 100
for i in "${instances[@]}"; do
  [[ $(state "$i") == running && $(field "$i" '.sleeping_since // ""') == "" ]] ||
    fail "$i fell asleep while a client held a connection"
done
stop_holders
pass "held client connections kept all three awake for 100 s"

# 11. Clean up: unlink, remove the instances, put the TOMLs back; then the foreign listeners
# and Herd's resolver, after.
trap - EXIT
cleanup || fail "the cleanup could not remove everything it added"
for i in "${instances[@]}"; do
  if services_json | jq -e --arg n "$i" '.[] | select(.name == $n)' >/dev/null; then
    fail "$i is still an instance after its removal"
  fi
done
pass "unlinked $site and removed the three instances"
listeners_after="$(listeners)"
[[ $listeners_before == "$listeners_after" ]] || fail "the listeners on ${foreign_ports[*]} changed: before
$listeners_before
after
$listeners_after"
pass "the listeners on ${foreign_ports[*]} are unchanged"
[[ $resolver_before == "$(sum "$herd_resolver")" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
