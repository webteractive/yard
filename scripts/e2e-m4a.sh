#!/usr/bin/env bash
# M4a end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Links a temp PHP app as hello-yard whose .yard asks for PHP 7.1, checks that an edit to
# the file is served within seconds, that a supported PHP which is not installed serves the
# setup page, and that `yard isolate` is refused for it. It installs nothing: it needs
# php@7.1 (left by e2e-m2.sh) and 8.4, and one supported PHP (7.0–8.5) left uninstalled.
# sites.toml is backed up and put back. Herd's resolver must come out unchanged.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
brew_prefix="$(brew --prefix)"
herd_resolver=/etc/resolver/test
site=hello-yard

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# The output of curl "$@" once it matches the glob in $want, trying for $tries × 0.2 s.
# yardd applies a .yard edit a moment after it is saved.
answer() {
  local body=""
  for _ in $(seq "${tries:-50}"); do
    body="$(curl -fsS "$@" 2>/dev/null || true)"
    # shellcheck disable=SC2053
    [[ $body == $want ]] && break
    sleep 0.2
  done
  printf '%s' "$body"
}

# 1. What this needs, and Herd's resolver, before.
[[ -d $brew_prefix/opt/php@7.1 ]] || fail "php@7.1 is not installed; run scripts/e2e-m2.sh first"
# The first supported PHP that is not installed, for the setup page. 8.5 is brew's plain `php`.
missing=""
for v in 7.0 7.1 7.2 7.3 7.4 8.0 8.1 8.2 8.3 8.4 8.5; do
  [[ -d $brew_prefix/opt/php@$v ]] && continue
  [[ $v == 8.5 && -d $brew_prefix/opt/php ]] && continue
  missing=$v
  break
done
[[ -n $missing ]] || fail "every supported PHP (7.0–8.5) is installed, so the setup page cannot be checked"
before="$(sum "$herd_resolver")"

# 2. The fixture in a temp dir, and sites.toml backed up.
work="$(mktemp -d /tmp/yard-e2e-XXXXXX)"
cp -R "$fixtures/hello-php" "$work/$site"
sites="$data/sites.toml"
cp -p "$sites" "$work/sites.toml.bak"

cleanup() {
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  # The override stub Yard created for the site, unless someone has edited it.
  local stub="$data/nginx/overrides/$site.conf"
  if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  if [[ -f $work/sites.toml.bak ]] && ! cmp -s "$work/sites.toml.bak" "$sites"; then
    cp -p "$work/sites.toml.bak" "$sites"
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# 3. A .yard asking for 7.1, then link.
printf 'php = "7.1"\n' >"$work/$site/.yard"
(cd "$work/$site" && "$yard" link "$site" >/dev/null)
want="7.1.*"
version="$(answer "http://$site.yard:8080/")"
[[ $version == 7.1.* ]] || fail "http://$site.yard:8080/ answered '$version', expected PHP 7.1.*"
pass "the .yard's php = \"7.1\" serves PHP $version"

# 4. An edit to the file is served within 3 s.
printf 'php = "8.4"\n' >"$work/$site/.yard"
want="8.4.*"
version="$(tries=15 answer "http://$site.yard:8080/")"
[[ $version == 8.4.* ]] || fail "3 s after the edit, http://$site.yard:8080/ answered '$version'"
pass "editing it to 8.4 serves PHP $version within 3 s"

# 5. A PHP that is not installed serves the setup page, naming the fix.
printf 'php = "%s"\n' "$missing" >"$work/$site/.yard"
code=""
for _ in $(seq 50); do
  code="$(curl -s -o "$work/body" -w '%{http_code}' "http://$site.yard:8080/" || true)"
  [[ $code == 503 ]] && break
  sleep 0.2
done
[[ $code == 503 ]] || fail "with php = \"$missing\", http://$site.yard:8080/ answered $code, expected 503"
grep -q 'yard init' "$work/body" || fail "the setup page does not name yard init: $(head -c 400 "$work/body")"
pass "php = \"$missing\" (not installed) serves the 503 setup page naming yard init"

# 6. The file owns the site's php, so yard isolate is refused.
if "$yard" isolate 8.4 --site "$site" --json >"$work/isolate.json" 2>&1; then
  fail "yard isolate 8.4 --site $site succeeded"
fi
grep -q '"SITE_HAS_PROJECT_FILE"' "$work/isolate.json" || fail "yard isolate failed with: $(cat "$work/isolate.json")"
pass "yard isolate is refused with SITE_HAS_PROJECT_FILE"

# 7. Clean up, then sites.toml and Herd's resolver, after.
trap - EXIT
cleanup
after="$(sum "$herd_resolver")"
[[ $before == "$after" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
