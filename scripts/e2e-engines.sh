#!/usr/bin/env bash
# Service recipes end-to-end check on a real Mac (recipes spec §13), after e2e-m1.sh passes.
#
# Adds the carried recipes valkey, memcached, meilisearch and typesense through `yard service
# add`, asks each one a question by absolute path (valkey-cli PING, memcached's version over TCP,
# /health on the two search engines), checks `yard env` in a scratch project whose .yard lists
# them and the meilisearch.yard console, then writes two scratch recipes into ~/.yard/engines: a
# path one (Apple's python3 serving a directory) and an npm one (http-server 14.1.1 on Node 22),
# adds and curls both, lets memcached fall asleep and wakes it with yard service start, and removes
# everything it added and both recipe files.
#
# Needs: yardd running (e2e-m1.sh), built from this tree, brew, jq and the network.
# Installs: brew's valkey, memcached and meilisearch when missing, Typesense 30.2's download into
# ~/.yard/programs, Node 22 through Yard's fnm when missing, and http-server into ~/.yard/programs.
# Ask the maintainer first.
#
# It stops before changing anything if one of its instances or recipe files already exists.
# services.toml, ports.toml and sites.toml are backed up and put back. It never touches DBngin,
# Postgres.app or Herd: the listeners on 3306, 5432 and 6379 and Herd's resolver must come out
# unchanged. Every port is read from `yard services --json`.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
site="yard-e2e-engines"
carried=(valkey memcached meilisearch typesense)
scratch=(e2e-path e2e-npm)
foreign_ports=(3306 5432 6379)

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { if [[ -e $1 ]]; then shasum -a 256 "$1" | cut -d' ' -f1; else echo absent; fi; }

listeners() {
  local port
  for port in "${foreign_ports[@]}"; do
    printf '%s: %s\n' "$port" "$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpc 2>/dev/null | grep -E '^[pc]' | sort -u | tr '\n' ' ')"
  done
}

services_json() { "$yard" services --json; }

# The first port yard services reports for instance $1.
port_of() {
  local found
  found="$(services_json | jq -r --arg n "$1" '.[] | select(.name == $n) | .ports[0].port')"
  [[ -n $found && $found != null ]] || fail "yard services lists no port for $1"
  printf '%s' "$found"
}

state_of() {
  services_json | jq -r --arg n "$1" '.[] | select(.name == $n) | .state | if type == "object" then keys[0] else . end'
}

# The output of "$@" once it equals $want, trying for $tries x 0.2 s.
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
for name in "${carried[@]}" "${scratch[@]}"; do
  if jq -e --arg n "$name" '.[] | select(.name == $n)' <<<"$existing" >/dev/null; then
    fail "$name is already a service instance; this script removes what it adds, so it will not run over it"
  fi
done
for name in "${carried[@]}" "${scratch[@]}"; do
  [[ -e $data/engines/$name.toml ]] && fail "$data/engines/$name.toml exists; this script will not replace a recipe"
done
if "$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null; then
  fail "a site named $site already exists"
fi

resolver_before="$(sum "$herd_resolver")"
listeners_before="$(listeners)"

# 2. Back up the three TOMLs.
work="$(mktemp -d /tmp/yard-e2e-XXXXXX)"
tomls=(services.toml ports.toml sites.toml)
for f in "${tomls[@]}"; do
  if [[ -f $data/$f ]]; then cp -p "$data/$f" "$work/$f.bak"; else : >"$work/$f.absent"; fi
done
added=()
recipes=()
cleanup_failed=0

cleanup() {
  local name f stub
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
  for name in "${recipes[@]}"; do rm -f "$data/engines/$name.toml"; done
  # The override stubs Yard created for the site and for meilisearch.yard, unless edited.
  for stub in "$data/nginx/overrides/$site.conf" "$data/nginx/overrides/meilisearch.conf"; do
    if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  done
  for f in "${tomls[@]}"; do
    if [[ $cleanup_failed == 1 && $f != sites.toml ]]; then continue; fi
    if [[ -f $work/$f.bak ]]; then
      cmp -s "$work/$f.bak" "$data/$f" || cp -p "$work/$f.bak" "$data/$f"
    elif [[ -f $data/$f ]] && ! grep -q '^\[' "$data/$f"; then
      rm -f "$data/$f"
    fi
  done
  rm -rf "$work"
  [[ $(sum "$herd_resolver") == "$resolver_before" ]] || { echo "FAIL Herd's resolver changed" >&2; cleanup_failed=1; }
  [[ $(listeners) == "$listeners_before" ]] || { echo "FAIL the listeners on 3306, 5432 or 6379 changed" >&2; cleanup_failed=1; }
  return "$cleanup_failed"
}
trap cleanup EXIT

add() {
  local name="$1"
  added+=("$name")
  echo "adding $name (it may install its program first)..."
  "$yard" service add "$name" --yes --json >"$work/add-$name.out" 2>&1 \
    || fail "yard service add $name failed: $(tail -c 600 "$work/add-$name.out")"
  [[ $(state_of "$name") == running ]] || fail "$name is $(state_of "$name"), expected running"
  pass "added $name"
}

# 3. The carried recipes, each asked its own question.
for name in "${carried[@]}"; do add "$name"; done

valkey_port="$(port_of valkey)"
[[ $("$brew_prefix/opt/valkey/bin/valkey-cli" -p "$valkey_port" PING) == PONG ]] || fail "valkey did not answer PING"
pass "valkey answers PING on $valkey_port"

memcached_port="$(port_of memcached)"
printf 'version\r\nquit\r\n' | /usr/bin/nc 127.0.0.1 "$memcached_port" | grep -q '^VERSION' || fail "memcached did not answer version"
pass "memcached answers version on $memcached_port"

meili_port="$(port_of meilisearch)"
want='{"status":"available"}' answer="$(tries=50 eventually curl -fsS "http://127.0.0.1:$meili_port/health")"
[[ $answer == '{"status":"available"}' ]] || fail "meilisearch /health answered '$answer'"
pass "meilisearch is available on $meili_port"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$meili_port/indexes")"
[[ $code == 401 ]] || fail "meilisearch answered $code without its key, expected 401: its master key is set"
pass "meilisearch refuses a request without its key"

ts_port="$(port_of typesense)"
want='{"ok":true}' answer="$(tries=100 eventually curl -fsS "http://127.0.0.1:$ts_port/health")"
[[ $answer == '{"ok":true}' ]] || fail "typesense /health answered '$answer'"
pass "typesense is healthy on $ts_port"
ts_listeners="$(lsof -nP -iTCP -sTCP:LISTEN -a -c typesense | awk 'NR>1 {print $9}')"
[[ -n $ts_listeners ]] || fail "lsof finds no typesense listener to check"
grep -vqE '^(127\.0\.0\.1|\[::1\]):' <<<"$ts_listeners" && fail "typesense listens beyond loopback: $ts_listeners"
pass "typesense listens on loopback only"

# 4. yard env in a scratch project whose .yard lists them, and the console through nginx.
mkdir -p "$work/$site"
printf '<?php echo "ok";\n' >"$work/$site/index.php"
printf '[services]\nvalkey = true\nmemcached = true\nmeilisearch = true\ntypesense = true\n' >"$work/$site/.yard"
(cd "$work/$site" && "$yard" link "$site" --yes >/dev/null) || fail "yard link $site failed"
env_lines="$(cd "$work/$site" && "$yard" env)"
for line in "REDIS_PORT=$valkey_port" "MEMCACHED_PORT=$memcached_port" "MEILISEARCH_HOST=http://127.0.0.1:$meili_port" "TYPESENSE_PORT=$ts_port"; do
  grep -qx "$line" <<<"$env_lines" || fail "yard env lacks $line"
done
grep -qE '^MEILISEARCH_KEY=[0-9a-f]{32}$' <<<"$env_lines" || fail "yard env has no MEILISEARCH_KEY secret"
grep -qE '^TYPESENSE_API_KEY=[0-9a-f]{32}$' <<<"$env_lines" || fail "yard env has no TYPESENSE_API_KEY secret"
pass "yard env lists each engine's port and its secrets"
want=200 code="$(tries=50 eventually curl -s -o /dev/null -w '%{http_code}' "http://meilisearch.yard:8080/")"
[[ $code == 200 ]] || fail "http://meilisearch.yard:8080/ answered $code"
pass "meilisearch.yard serves its dashboard"
"$yard" unlink "$site" >/dev/null

# 5. A path recipe and an npm recipe of the user's own.
mkdir -p "$data/engines"
recipes+=(e2e-path)
cat >"$data/engines/e2e-path.toml" <<'EOF'
schema = 1
key = "e2e-path"
name = "E2E path"

[source]
kind = "path"
program = "/usr/bin/python3"

[ports]
main = 29870

[run]
command = ["{program}", "-m", "http.server", "--bind", "127.0.0.1", "--directory", "{data}", "{port.main}"]
dirs = ["data"]

[health]
tcp = "main"
EOF
recipes+=(e2e-npm)
cat >"$data/engines/e2e-npm.toml" <<'EOF'
schema = 1
key = "e2e-npm"
name = "E2E npm"

[source]
kind = "npm"
package = "http-server"
version = "14.1.1"
node = "22"

[ports]
main = 29871

[run]
command = ["{node}", "{home}/node_modules/http-server/bin/http-server", "-a", "127.0.0.1", "-p", "{port.main}", "{data}"]
dirs = ["data"]

[health]
tcp = "main"
EOF
for name in "${scratch[@]}"; do
  "$yard" engines check "$data/engines/$name.toml" >/dev/null || fail "yard engines check $name failed"
  want=1 known="$(tries=50 eventually bash -c "\"$yard\" engines --json | jq '[.engines[] | select(.key == \"$name\")] | length'")"
  [[ $known == 1 ]] || fail "yardd did not pick up $name"
  add "$name"
  port="$(port_of "$name")"
  want=200 code="$(tries=50 eventually curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/")"
  [[ $code == 200 ]] || fail "$name answered $code on $port"
  pass "$name serves on $port"
done

# 6. memcached falls asleep, and wakes.
"$yard" service sleep-after memcached 60s >/dev/null
for _ in $(seq 120); do [[ -n $(services_json | jq -r '.[] | select(.name == "memcached") | .sleeping_since // empty') ]] && break; sleep 1; done
[[ -n $(services_json | jq -r '.[] | select(.name == "memcached") | .sleeping_since // empty') ]] || fail "memcached never fell asleep"
pass "memcached fell asleep"
# Nothing listens while it sleeps; yard service start wakes it, as a PHP request would.
"$yard" service start memcached >/dev/null || fail "yard service start memcached failed"
printf 'version\r\nquit\r\n' | /usr/bin/nc 127.0.0.1 "$memcached_port" | grep -q '^VERSION' || fail "memcached did not answer after waking"
pass "memcached woke and answers"
"$yard" service sleep-after memcached default >/dev/null

# 7. Afterwards (in cleanup): every instance and recipe removed, the TOMLs put back; yard engines
# then lists only the built-ins and carried recipes.
trap - EXIT
cleanup || fail "cleanup"
left="$("$yard" engines --json | jq -r '.engines[] | select(.origin.kind == "user") | .key')"
[[ -z $left ]] || fail "user recipes are still listed: $left"
pass "yard engines lists only built-in and carried engines again"
echo "e2e-engines: all checks passed"
