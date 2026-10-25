#!/usr/bin/env bash
# M4b end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Adds MySQL 8.4, PostgreSQL 17, Redis and MinIO through `yard service add`, asks each
# one a question with its own client from the brew keg, checks that `yard env` prints the
# ports `yard services` reports, that `yard info` shows a scratch project's `.yard`
# [services] entry met, and that the MinIO keg's tap is the one services.toml records.
# Then it removes every instance it added.
#
# Needs: yardd running (e2e-m1.sh), brew, jq, and `minio/stable` tapped and trusted (the
# MinIO add fails with FORMULA_UNTRUSTED otherwise; this script never grants trust).
# Installs: `mysql@8.4`, `postgresql@17`, `redis` and `minio/stable/minio` with brew when
# they are missing, and leaves them installed (Yard never runs brew uninstall). Ask the maintainer
# first.
#
# It stops before changing anything if mysql-8.4, postgres-17, redis or minio is already
# an instance: it removes only what it added. services.toml, ports.toml and sites.toml are
# backed up and put back if they differ. It never touches DBngin, Postgres.app or Herd:
# the listeners on 3306, 5432 and 6379 and Herd's resolver must come out unchanged. Every
# port is read from `yard services --json`, never assumed.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
site="yard-e2e-services"
instances=(mysql-8.4 postgres-17 redis minio)
foreign_ports=(3306 5432 6379)

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

# The services Yard reports, as JSON.
services_json() { "$yard" services --json; }

# $1 is an instance, $2 a jq filter on its ports array; prints the match.
port_where() {
  services_json | jq -r --arg n "$1" ".[] | select(.name == \$n) | .ports | $2"
}

# The output of "$@" once it equals $want, trying for $tries x 0.2 s. A service answers a
# moment after yardd reports it running, and nginx applies a reload after the command returns.
eventually() {
  local out=""
  for _ in $(seq "${tries:-50}"); do
    out="$("$@" 2>/dev/null || true)"
    [[ $out == "$want" ]] && break
    sleep 0.2
  done
  printf '%s' "$out"
}

# 1. What this needs, and what must not change, before anything is touched.
[[ -x $yard ]] || fail "$yard is not built; run cargo build --release"
command -v brew >/dev/null || fail "brew is not installed"
command -v jq >/dev/null || fail "jq is not installed (brew install jq)"
"$yard" status --json >/dev/null 2>&1 || fail "yardd is not running; run yard start and scripts/e2e-m1.sh first"
brew_prefix="$(brew --prefix)"

existing="$(services_json)" || fail "yard services failed"
for name in "${instances[@]}"; do
  if jq -e --arg n "$name" '.[] | select(.name == $n)' <<<"$existing" >/dev/null; then
    fail "$name is already a service instance; this script removes what it adds, so it will not run over it"
  fi
done
if "$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null; then
  fail "a site named $site already exists"
fi

resolver_before="$(sum "$herd_resolver")"
listeners_before="$(listeners)"
echo "listeners before:"
printf '  %s\n' "${listeners_before//$'\n'/$'\n'  }"

# 2. Back up the three TOMLs. A file that does not exist yet is noted, not backed up.
work="$(mktemp -d /tmp/yard-e2e-XXXXXX)"
tomls=(services.toml ports.toml sites.toml)
for f in "${tomls[@]}"; do
  if [[ -f $data/$f ]]; then cp -p "$data/$f" "$work/$f.bak"; else : >"$work/$f.absent"; fi
done

# Instances this script is about to add. Each was absent in step 1, so each one that exists
# when the cleanup runs is ours. Nothing else is ever removed.
added=()
cleanup_failed=0

cleanup() {
  local name stub f
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  for ((i = ${#added[@]} - 1; i >= 0; i--)); do
    name="${added[$i]}"
    if services_json 2>/dev/null | jq -e --arg n "$name" '.[] | select(.name == $n)' >/dev/null 2>&1; then
      if ! "$yard" service remove "$name" --yes >/dev/null 2>&1; then
        echo "FAIL could not remove $name; remove it with yard service remove $name --yes" >&2
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
    if [[ $cleanup_failed == 1 && $f != sites.toml ]]; then continue; fi
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

# 3. Add the four instances. --yes only skips the confirmation: it never grants trust, so a
# MinIO tap brew does not trust fails here with FORMULA_UNTRUSTED.
add() {
  local name="$1"
  shift
  added+=("$name")
  echo "adding $name (brew may install its formula first)..."
  if ! "$yard" service add "$@" --yes --json >"$work/add-$name.out" 2>&1; then
    if grep -q '"FORMULA_UNTRUSTED"' "$work/add-$name.out"; then
      fail "adding $name was refused: brew does not trust its formula. Run yard source trust minio/stable/minio yourself, then run this again"
    fi
    fail "yard service add $* failed: $(tail -c 600 "$work/add-$name.out")"
  fi
  pass "added $name"
}
add mysql-8.4 mysql --version 8.4
add postgres-17 postgres --version 17
add redis redis
add minio minio

# 4. Every instance is running, on a port yard services reports, and none took a port a
# foreign listener already held.
for name in "${instances[@]}"; do
  state="$(services_json | jq -r --arg n "$name" '.[] | select(.name == $n) | .state | if type == "object" then keys[0] else . end')"
  [[ $state == running ]] || fail "$name is $state, expected running"
done
port_of() {
  local found
  found="$(port_where "$1" "$2")"
  [[ -n $found ]] || fail "yard services lists no $2 port for $1"
  printf '%s' "$found"
}
mysql_port="$(port_of mysql-8.4 '.[0].port')"
postgres_port="$(port_of postgres-17 '.[0].port')"
redis_port="$(port_of redis '.[0].port')"
minio_api="$(port_of minio 'map(select(.name | test("console") | not)) | .[0].port')"
minio_console="$(port_of minio 'map(select(.name | test("console"))) | .[0].port')"
echo "ports: mysql-8.4 $mysql_port, postgres-17 $postgres_port, redis $redis_port, minio $minio_api (console $minio_console)"
for port in "$mysql_port" "$postgres_port" "$redis_port"; do
  for foreign in "${foreign_ports[@]}"; do
    if [[ $port == "$foreign" ]] && [[ -n $(grep "^$foreign: ." <<<"$listeners_before" || true) ]]; then
      fail "an instance took port $foreign, which another process held before this run"
    fi
  done
done
pass "every instance is running, on the ports yard services reports"

# 5. Each engine answers from its own client in the brew keg (a bare mysql or psql here
# could be DBngin's or Postgres.app's).
mysql_bin="$brew_prefix/opt/mysql@8.4/bin/mysql"
psql_bin="$brew_prefix/opt/postgresql@17/bin/psql"
redis_bin="$brew_prefix/opt/redis/bin/redis-cli"
for bin in "$mysql_bin" "$psql_bin" "$redis_bin"; do [[ -x $bin ]] || fail "$bin is not there"; done

want=1
answer="$(tries=25 eventually "$mysql_bin" --no-defaults -h 127.0.0.1 -P "$mysql_port" -u root -N -e 'select 1')"
[[ $answer == 1 ]] || fail "mysql on port $mysql_port answered '$answer' to select 1"
pass "mysql-8.4 answers select 1 on port $mysql_port as root, with no password"

answer="$(tries=25 eventually "$psql_bin" -X -w -h 127.0.0.1 -p "$postgres_port" -At -c 'select 1' postgres)"
[[ $answer == 1 ]] || fail "postgres on port $postgres_port answered '$answer' to select 1"
pass "postgres-17 answers select 1 on port $postgres_port with no password"

want=PONG
answer="$(tries=25 eventually "$redis_bin" -h 127.0.0.1 -p "$redis_port" ping)"
[[ $answer == PONG ]] || fail "redis on port $redis_port answered '$answer' to ping"
pass "redis answers PONG on port $redis_port"

want=200
answer="$(tries=25 eventually curl -fsS -o /dev/null -w '%{http_code}' "http://127.0.0.1:$minio_api/minio/health/live")"
[[ $answer == 200 ]] || fail "minio's health check on port $minio_api answered '$answer', expected 200"
pass "minio's health check answers 200 on port $minio_api"

want=200
answer="$(tries=50 eventually curl -s -o /dev/null -w '%{http_code}' "http://minio.yard:8080/")"
[[ $answer == 200 ]] || fail "http://minio.yard:8080/ answered '$answer', expected 200 from the console through nginx"
pass "http://minio.yard:8080/ answers 200 (the console, through nginx)"

# 6. yard env prints the ports yard services reports.
env_json="$(cd "$work" && "$yard" env --json)" || fail "yard env --json failed"
env_value() { jq -r --arg i "$1" --arg k "$2" '.[] | select(.instance == $i) | .lines[] | select(startswith($k + "=")) | sub("^[^=]*="; "")' <<<"$env_json"; }
[[ $(env_value mysql-8.4 DB_PORT) == "$mysql_port" ]] || fail "yard env prints DB_PORT=$(env_value mysql-8.4 DB_PORT) for mysql-8.4, yard services says $mysql_port"
[[ $(env_value postgres-17 DB_PORT) == "$postgres_port" ]] || fail "yard env prints DB_PORT=$(env_value postgres-17 DB_PORT) for postgres-17, yard services says $postgres_port"
[[ $(env_value redis REDIS_PORT) == "$redis_port" ]] || fail "yard env prints REDIS_PORT=$(env_value redis REDIS_PORT) for redis, yard services says $redis_port"
[[ $(env_value minio AWS_ENDPOINT) == "http://127.0.0.1:$minio_api" ]] || fail "yard env prints AWS_ENDPOINT=$(env_value minio AWS_ENDPOINT) for minio, yard services says port $minio_api"
pass "yard env prints the ports yard services reports"

# 7. The MinIO keg's tap is the formula services.toml records. brew's answer is read, not
# assumed: the keg may be homebrew/core's rather than minio/stable's.
brew_formula="$(brew info --json=v2 minio | jq -r '.formulae[0] | select(.installed | length > 0) | .full_name')"
[[ -n $brew_formula ]] || fail "brew info --json=v2 minio shows no installed keg"
recorded="$(python3 -I -c 'import sys, tomllib
with open(sys.argv[1], "rb") as f:
    print(tomllib.load(f)["services"]["minio"]["formula"])' "$data/services.toml")" || fail "services.toml has no minio entry"
[[ $brew_formula == "$recorded" ]] || fail "brew says the installed minio is $brew_formula, services.toml records $recorded"
pass "brew's installed minio ($brew_formula) is the formula services.toml records"

# 8. A scratch project, outside ~/Herd, whose .yard asks for MySQL 8.4: yard info shows it met.
cp -R "$fixtures/hello-php" "$work/$site"
printf '[services]\nmysql = "8.4"\n' >"$work/$site/.yard"
(cd "$work/$site" && "$yard" link "$site" >/dev/null)
info="$(cd "$work/$site" && "$yard" info --json)" || fail "yard info failed in $work/$site"
met="$(jq -r '.services[] | select(.engine == "mysql") | "\(.want) \(.service.name // "missing")"' <<<"$info")"
[[ $met == "8.4 mysql-8.4" ]] || fail "yard info shows the mysql need as '$met', expected '8.4 mysql-8.4'"
[[ $(jq -r '.site.needs // [] | length' <<<"$info") == 0 ]] || fail "yard info lists unmet needs: $(jq -c '.site.needs' <<<"$info")"
pass "yard info shows [services] mysql = \"8.4\" met by mysql-8.4, with no unmet needs"

# 9. Clean up: unlink, remove what was added, put the TOMLs back; then the foreign listeners
# and Herd's resolver, after.
trap - EXIT
cleanup || fail "the cleanup could not remove everything it added"
pass "unlinked $site and removed ${instances[*]}"
for name in "${instances[@]}"; do
  if services_json | jq -e --arg n "$name" '.[] | select(.name == $n)' >/dev/null; then
    fail "$name is still an instance after its removal"
  fi
done
listeners_after="$(listeners)"
[[ $listeners_before == "$listeners_after" ]] || fail "the listeners on ${foreign_ports[*]} changed: before
$listeners_before
after
$listeners_after"
pass "the listeners on ${foreign_ports[*]} are unchanged"
[[ $resolver_before == "$(sum "$herd_resolver")" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
