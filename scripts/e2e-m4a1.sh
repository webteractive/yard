#!/usr/bin/env bash
# M4a.1 end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Runs `yard init` in a scratch git repo linked as hello-yard and checks that it leaves
# `git status` clean and serves from the managed .yard in Yard's data root, that
# `yard isolate 7.1` edits that file and keeps its comments (and `yard unisolate` the
# header and the comment above php), that a committed .yard added
# afterwards wins within 5 s while `yard doctor` warns about the managed one, and that
# `yard init --local` moves the managed file into the project. It installs nothing: it
# needs php@7.1 (left by e2e-m2.sh) and 8.4. sites.toml is backed up and put back, and
# the managed directory it made is removed. Herd's resolver must come out unchanged.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
brew_prefix="$(brew --prefix)"
herd_resolver=/etc/resolver/test
site=hello-yard
managed_dir="$data/sites/$site"
managed="$managed_dir/.yard"

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

git_() { git -C "$project" -c user.name=yard-e2e -c user.email=yard-e2e@localhost "$@"; }

# 1. What this needs, and Herd's resolver, before. A managed file for the name already
# there is someone's: stop before touching it.
[[ -d $brew_prefix/opt/php@7.1 ]] || fail "php@7.1 is not installed; run scripts/e2e-m2.sh first"
[[ ! -e $managed_dir ]] || fail "$managed_dir already exists; move it away first"
before="$(sum "$herd_resolver")"

# 2. The fixture as a git repo in a temp dir (never under ~/Herd), and sites.toml backed up.
work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-XXXXXX")"
project="$work/$site"
cp -R "$fixtures/hello-php" "$project"
git_ init -q
git_ add -A
git_ commit -qm "fixture"
sites="$data/sites.toml"
cp -p "$sites" "$work/sites.toml.bak"

cleanup() {
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  # The override stub Yard created for the site, unless someone has edited it.
  local stub="$data/nginx/overrides/$site.conf"
  if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  rm -rf "$managed_dir"
  if [[ -f $work/sites.toml.bak ]] && ! cmp -s "$work/sites.toml.bak" "$sites"; then
    cp -p "$work/sites.toml.bak" "$sites"
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# 3. yard init writes the managed file, links the site, and leaves the repo clean.
(cd "$project" && "$yard" init "$site" --yes >/dev/null)
[[ -f $managed ]] || fail "yard init wrote no $managed"
[[ ! -e $project/.yard ]] || fail "yard init wrote $project/.yard"
status="$(git_ status --porcelain)"
[[ -z $status ]] || fail "git status after yard init: $status"
pass "yard init wrote $managed and git status is clean"
info="$("$yard" info "$site")"
grep -q 'config *managed (' <<<"$info" || fail "yard info does not show the managed file: $info"
pass "yard info shows config managed"

# 4. yard isolate edits the managed file, keeping its comments, and the site serves it.
header="$(head -n 1 "$managed")"
printf '# e2e: keep me\n' >>"$managed"
"$yard" isolate 7.1 --site "$site" >/dev/null
grep -q '^php = "7.1"$' "$managed" || fail "yard isolate 7.1 left: $(cat "$managed")"
[[ $(head -n 1 "$managed") == "$header" ]] || fail "yard isolate dropped the header: $(cat "$managed")"
grep -q '^# e2e: keep me$' "$managed" || fail "yard isolate dropped a comment: $(cat "$managed")"
want="7.1.*"
version="$(answer "http://$site.yard:8080/")"
[[ $version == 7.1.* ]] || fail "http://$site.yard:8080/ answered '$version', expected PHP 7.1.*"
pass "yard isolate 7.1 edited the managed file, kept its comments, and serves PHP $version"

# 4b. yard unisolate removes php but keeps the header and the comment above it, then the
# site is isolated to 7.1 again for the steps below.
awk '/^php = /{print "# e2e: above php"}1' "$managed" >"$work/managed" && cat "$work/managed" >"$managed"
"$yard" unisolate --site "$site" >/dev/null
! grep -q '^php = ' "$managed" || fail "yard unisolate left php: $(cat "$managed")"
[[ $(head -n 1 "$managed") == "$header" ]] || fail "yard unisolate dropped the header: $(cat "$managed")"
grep -q '^# e2e: above php$' "$managed" || fail "yard unisolate dropped the comment above php: $(cat "$managed")"
"$yard" isolate 7.1 --site "$site" >/dev/null
pass "yard unisolate removed php and kept the header and the comment above it"

# 5. A committed .yard wins within 5 s, and the doctor warns about the managed one.
printf 'php = "8.4"\n' >"$project/.yard"
git_ add .yard
git_ commit -qm "commit a .yard"
want="8.4.*"
version="$(tries=25 answer "http://$site.yard:8080/")"
[[ $version == 8.4.* ]] || fail "5 s after committing .yard, http://$site.yard:8080/ answered '$version'"
pass "the committed .yard wins: PHP $version"
# doctor exits 1 when any check on this machine fails; only this check matters here.
"$yard" doctor --json >"$work/doctor.json" 2>&1 || true
grep -q "\"managed-file-ignored:$site\"" "$work/doctor.json" \
  || fail "yard doctor has no managed-file-ignored:$site: $(head -c 600 "$work/doctor.json")"
pass "yard doctor warns managed-file-ignored:$site"

# 6. Without the committed file, init --local moves the managed file into the project.
git_ rm -q .yard
git_ commit -qm "drop the .yard"
want="7.1.*"
version="$(tries=25 answer "http://$site.yard:8080/")"
[[ $version == 7.1.* ]] || fail "without the committed .yard, http://$site.yard:8080/ answered '$version'"
pass "removing the committed .yard falls back to the managed file: PHP $version"
(cd "$project" && "$yard" init --local --yes >/dev/null)
[[ -f $project/.yard ]] || fail "yard init --local wrote no $project/.yard"
[[ ! -e $managed ]] || fail "yard init --local left $managed"
grep -q '^php = "7.1"$' "$project/.yard" || fail "the moved .yard is: $(cat "$project/.yard")"
grep -q '^project' "$project/.yard" && fail "the moved .yard kept its project key: $(cat "$project/.yard")"
pass "yard init --local moved the managed file into the project"

# 7. Clean up, then sites.toml and Herd's resolver, after.
trap - EXIT
cleanup
after="$(sum "$herd_resolver")"
[[ $before == "$after" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
