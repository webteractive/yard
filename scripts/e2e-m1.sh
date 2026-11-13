#!/usr/bin/env bash
# M1 end-to-end check on a real Mac, in the standard profile, beside Herd where it runs.
#
# Needs nginx, dnsmasq and php@8.4 from brew, `sudo yard helper install` and
# `yard start`. It uses the real data root, links a temp project, proxies to a
# throwaway server, and removes both again with their override stubs. Herd's resolver
# must come out unchanged (still absent on a Mac without Herd). The steps that need Herd
# running, a Herd site answering and the privileged profile refused naming Herd, are skipped
# without it.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
herd_resolver=/etc/resolver/test
herd=no
[[ -d /Applications/Herd.app || -e $herd_resolver ]] && herd=yes

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
# The file's checksum, or "absent": on a Mac without Herd its resolver must stay absent.
sum() { if [[ -e $1 ]]; then shasum -a 256 "$1" | cut -d' ' -f1; else echo absent; fi; }

# The body of $1 once it matches the glob $2. nginx applies a reload a moment after the
# command that caused it returns, so the first answer can still come from the old config.
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

# 1. Herd's resolver, before.
before="$(sum "$herd_resolver")"

# 2. Fixtures: hello-php in a temp dir, hello-proxy on a free port.
work="$(mktemp -d /tmp/yard-e2e-XXXXXX)"
cp -R "$fixtures/hello-php" "$work/"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 "$fixtures/hello-proxy/server.py" "$port" &
server=$!

cleanup() {
  "$yard" unproxy hello-proxy >/dev/null 2>&1 || true
  "$yard" unlink hello-php >/dev/null 2>&1 || true
  # The override stubs Yard created for the two sites, unless someone has edited them.
  local stub
  for stub in "$HOME/.yard/nginx/overrides/"{hello-php,hello-proxy}.conf; do
    if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  done
  kill "$server" 2>/dev/null || true
  wait "$server" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

# 3. Link and proxy.
(cd "$work/hello-php" && "$yard" link hello-php >/dev/null)
"$yard" proxy hello-proxy "http://127.0.0.1:$port" >/dev/null
pass "linked $work/hello-php and proxied hello-proxy to port $port"

# 4. Yard serves both on 8080; Herd still answers its own .test sites.
version="$(answer http://hello-php.yard:8080/ "8.4.*")"
[[ $version == 8.4.* ]] || fail "http://hello-php.yard:8080/ answered '$version', expected PHP 8.4.*"
pass "http://hello-php.yard:8080/ runs PHP $version"

proxied="$(answer http://hello-proxy.yard:8080/ "*proxied*")"
[[ $proxied == *proxied* ]] || fail "http://hello-proxy.yard:8080/ answered '${proxied:0:200}'"
pass "http://hello-proxy.yard:8080/ reaches the proxied app"

if [[ $herd == yes ]]; then
  herd_site="$(find "$HOME/Herd" -mindepth 1 -maxdepth 1 -type d ! -name '.*' | head -1 | xargs basename)"
  code="$(curl -s -o /dev/null -w '%{http_code}' "http://$herd_site.test/" || true)"
  [[ $code != 000 ]] || fail "Herd did not answer http://$herd_site.test/"
  pass "Herd still answers http://$herd_site.test/ (HTTP $code)"
else
  pass "skip: Herd is not installed, so no Herd site to check"
fi

# 5. nginx runs as the user, not root (M1.1).
nginx_pid="$("$yard" status --json | python3 -c 'import json, sys; print(next(u["pid"] for u in json.load(sys.stdin)["units"] if u["name"] == "nginx"))')"
nginx_user="$(ps -o user= -p "$nginx_pid" | tr -d ' ')"
[[ $nginx_user == "$USER" ]] || fail "nginx (pid $nginx_pid) runs as '$nginx_user', expected $USER"
pass "nginx (pid $nginx_pid) runs as $USER"

# 6. Switching to the privileged profile is refused while something holds 80 or 443, naming
# Herd when it is Herd. With both free it goes through: hello-php answers on 80, and the
# switch back to standard (nginx still holding 8080 and 8443 until it moves) serves it on 8080
# again. Both run nginx -t on the helper's sockets, which nginx cannot bind itself.
if [[ $herd == yes ]] || lsof -nP -iTCP:80 -iTCP:443 -sTCP:LISTEN >/dev/null 2>&1; then
  set +e
  refusal="$("$yard" ports profile privileged 2>&1 >/dev/null)"
  status=$?
  set -e
  [[ $status == 1 ]] || fail "yard ports profile privileged exited $status, expected 1"
  [[ $herd == no || $refusal == *Herd* ]] || fail "the refusal does not name Herd: $refusal"
  pass "yard ports profile privileged is refused: $refusal"
else
  # Back to standard whatever happens below, so the run never leaves this Mac on 80/443.
  trap '"$yard" ports profile standard >/dev/null 2>&1 || true; cleanup' EXIT
  "$yard" ports profile privileged >/dev/null || fail "yard ports profile privileged failed with 80 and 443 free"
  body="$(answer "http://hello-php.yard/" "8.4.*")"
  [[ $body == 8.4.* ]] || fail "hello-php did not answer on 80 in the privileged profile: '$body'"
  pass "the privileged profile serves http://hello-php.yard/ on 80"
  "$yard" ports profile standard >/dev/null || fail "yard ports profile standard failed after privileged"
  body="$(answer "http://hello-php.yard:8080/" "8.4.*")"
  [[ $body == 8.4.* ]] || fail "hello-php did not answer on 8080 after switching back: '$body'"
  lsof -nP -iTCP:80 -iTCP:443 -sTCP:LISTEN >/dev/null 2>&1 && fail "80 or 443 is still held after switching back"
  trap cleanup EXIT
  pass "switching back to standard serves on 8080 and lets go of 80 and 443"
fi

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
