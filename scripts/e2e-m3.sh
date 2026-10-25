#!/usr/bin/env bash
# M3 end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Links a temp PHP app as hello-tls, secures it, and checks it over HTTPS against Yard's
# CA with curl --cacert, so it needs no keychain trust and never runs `yard trust`
# (macOS asks for a password there). Unsecures and unlinks it again, removing its
# override stub. Herd's resolver must come out unchanged.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
site=hello-tls

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# The output of curl "$@" once it matches the glob in $want. nginx applies a reload a
# moment after the command that caused it returns, so the first answer can still come
# from the old config.
answer() {
  local body=""
  for _ in $(seq 50); do
    body="$(curl -fsS "$@" 2>/dev/null || true)"
    # shellcheck disable=SC2053
    [[ $body == $want ]] && break
    sleep 0.2
  done
  printf '%s' "$body"
}

# 1. Herd's resolver, before.
before="$(sum "$herd_resolver")"

# 2. The fixture in a temp dir.
work="$(mktemp -d /tmp/yard-e2e-XXXXXX)"
cp -R "$fixtures/hello-php" "$work/$site"

cleanup() {
  "$yard" unsecure "$site" >/dev/null 2>&1 || true
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  # The override stub Yard created for the site, unless someone has edited it.
  local stub="$data/nginx/overrides/$site.conf"
  if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  rm -rf "$work"
}
trap cleanup EXIT

# 3. Link and secure.
(cd "$work/$site" && "$yard" link "$site" >/dev/null)
"$yard" secure "$site" >/dev/null
pass "linked and secured $site"

# 4. HTTPS answers with a certificate from Yard's CA.
ca="$data/ca/yard-ca.pem"
[[ -f $ca ]] || fail "$ca is missing"
want="8.4.*"
version="$(answer --cacert "$ca" "https://$site.yard:8443/")"
[[ $version == 8.4.* ]] || fail "https://$site.yard:8443/ answered '$version', expected PHP 8.4.*"
pass "https://$site.yard:8443/ runs PHP $version, verified against $ca"

# 5. Its http URL redirects to the https one, keeping the path.
headers="$(curl -sI "http://$site.yard:8080/x" | tr -d '\r')"
[[ $headers == "HTTP/1.1 301"* ]] || fail "http://$site.yard:8080/x answered: ${headers%%$'\n'*}"
location="$(grep -i '^location:' <<<"$headers" | cut -d' ' -f2)"
[[ $location == "https://$site.yard:8443/x" ]] || fail "the redirect goes to '$location'"
pass "http://$site.yard:8080/x answers 301 to $location"

# 6. The site's key is readable by the user alone.
key="$data/certs/$site.key"
mode="$(stat -f '%Lp' "$key")"
[[ $mode == 600 ]] || fail "$key has mode $mode, expected 600"
pass "$key has mode 600"

# 7. Doctor has no failing check (an untrusted CA is a warning).
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

# 8. Unsecure serves plain http again and removes the certificate.
"$yard" unsecure "$site" >/dev/null
want="8.4.*"
version="$(answer "http://$site.yard:8080/")"
[[ $version == 8.4.* ]] || fail "after unsecure, http://$site.yard:8080/ answered '$version'"
[[ ! -e $data/certs/$site.pem && ! -e $key ]] || fail "unsecure left $data/certs/$site.{pem,key}"
pass "yard unsecure serves http://$site.yard:8080/ again and removed the certificate"

# 9. Clean up, then Herd's resolver, after.
trap - EXIT
cleanup
after="$(sum "$herd_resolver")"
[[ $before == "$after" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
