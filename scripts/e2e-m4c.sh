#!/usr/bin/env bash
# M4c end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Links a scratch project (never under ~/Herd) whose .nvmrc pins 16.14.0 as hello-node,
# and checks that `yard link --yes` installs that Node with fnm into ~/.yard/node, that the
# node and npm shims run it (an npm script too), that a native module builds from source
# under it, that `yard node use 22` in the managed .yard beats the .nvmrc, that a pin that
# is not installed fails with the install hint, and that `yard node uninstall` refuses a
# version a site still needs. It needs yardd running and brew's fnm, and downloads Node
# 16.14.0, the newest 22 and bufferutil. yard.toml and sites.toml are backed up and put
# back, only the Node versions this run installed are removed, and the run fails if nvm's
# or Herd's Node versions, or Herd's resolver, changed.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
brew_prefix="$(brew --prefix)"
fnm="$brew_prefix/bin/fnm"
herd_resolver=/etc/resolver/test
site=hello-node
managed_dir="$data/sites/$site"
versions_dir="$data/node/node-versions"
nvm_dirs=("$HOME/.nvm/versions/node" "$HOME/Library/Application Support/Herd/config/nvm")

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# The installed Node versions, one per line (v16.14.0), or nothing.
installed() { if [[ -d $versions_dir ]]; then ls "$versions_dir"; fi; }

# nvm's and Herd's Node versions, as ls -la shows them, or a line saying a folder is missing.
nvm_listing() {
  local dir
  for dir in "${nvm_dirs[@]}"; do
    if [[ -d $dir ]]; then ls -la "$dir"; else echo "missing: $dir"; fi
  done
}

# Yard's shim for $1, run in the project.
shim() { (cd "$project" && "$data/bin/$1" "${@:2}"); }

# 1. What this needs, and what must come out unchanged. A managed file for the name already
# there is someone's, and a 16.20.2 already installed would spoil the missing-pin check.
[[ -x $fnm ]] || fail "fnm is not installed; run brew install fnm first"
[[ ! -e $managed_dir ]] || fail "$managed_dir already exists; move it away first"
[[ ! -e $versions_dir/v16.20.2 ]] || fail "Node 16.20.2 is installed, so a pin of it is not missing"
work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-XXXXXX")"
nvm_listing >"$work/nvm-before.txt"
installed >"$work/versions-before.txt"
herd_before="$(sum "$herd_resolver")"

# 2. The fixture with an .nvmrc and an npm script, and the configs backed up. npm's and
# node-gyp's caches go in the temp dir, so ~/.npm and ~/Library/Caches are not touched.
project="$work/$site"
cp -R "$fixtures/hello-php" "$project"
printf '16.14.0\n' >"$project/.nvmrc"
printf '{"name": "hello-node", "private": true, "scripts": {"v": "node -v"}}\n' >"$project/package.json"
export npm_config_cache="$work/npm-cache" npm_config_devdir="$work/node-gyp"
# Node 16's node-gyp imports distutils, which Python 3.12 removed; Apple's python3 is 3.9.
if [[ -x /usr/bin/python3 ]]; then export npm_config_python=/usr/bin/python3; fi
for f in yard.toml sites.toml; do
  if [[ -f $data/$f ]]; then cp -p "$data/$f" "$work/$f.bak"; fi
done

cleanup() {
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  # The override stub Yard created for the site, unless someone has edited it.
  local stub="$data/nginx/overrides/$site.conf"
  if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  rm -rf "$managed_dir"
  local f v
  for f in yard.toml sites.toml; do
    if [[ -f $work/$f.bak ]] && ! cmp -s "$work/$f.bak" "$data/$f"; then cp -p "$work/$f.bak" "$data/$f"; fi
  done
  # Only what this run installed. Yard refuses to remove the last version the default
  # matches, so that one goes through fnm itself, in Yard's FNM_DIR.
  for v in $(comm -13 "$work/versions-before.txt" <(installed)); do
    "$yard" node uninstall "${v#v}" >/dev/null 2>&1 \
      || FNM_DIR="$data/node" "$fnm" uninstall "$v" >/dev/null 2>&1 || true
  done
  installed >"$work/versions-after.txt"
}
trap 'cleanup; rm -rf "$work"' EXIT

# 3. yard link --yes installs the pinned Node, and the shims run it.
(cd "$project" && "$yard" link "$site" --yes >/dev/null)
[[ -x $versions_dir/v16.14.0/installation/bin/node ]] || fail "yard link --yes did not install Node 16.14.0"
v="$(shim node -v)"
[[ $v == v16.14.0 ]] || fail "node -v in the project is '$v', expected v16.14.0"
v="$(shim npm run -s v)"
[[ $v == v16.14.0 ]] || fail "npm run v in the project printed '$v', expected v16.14.0"
pass "yard link --yes installed Node 16.14.0, and node and npm run it"

# 4. A native module builds from source under it (node-gyp, Node 16's headers).
shim npm install --build-from-source --no-audit --no-fund bufferutil >"$work/npm-install.log" 2>&1 \
  || fail "npm install --build-from-source bufferutil failed: $(tail -n 20 "$work/npm-install.log")"
pass "bufferutil built from source under Node 16.14.0"

# 5. After yard init (the managed .yard), yard node use 22 beats the .nvmrc.
(cd "$project" && "$yard" init "$site" --yes >/dev/null)
[[ -f $managed_dir/.yard ]] || fail "yard init wrote no $managed_dir/.yard"
"$yard" node install 22 >/dev/null
"$yard" node use 22 --site "$site" >/dev/null
grep -q '^node = "22"$' "$managed_dir/.yard" || fail "yard node use 22 left: $(cat "$managed_dir/.yard")"
v="$(shim node -v)"
[[ $v == v22.* ]] || fail "after yard node use 22, node -v is '$v'"
pass "yard node use 22 set node in the managed .yard and beats the .nvmrc: $v"
"$yard" node unuse --site "$site" >/dev/null
v="$(shim node -v)"
[[ $v == v16.14.0 ]] || fail "after yard node unuse, node -v is '$v', expected the .nvmrc's v16.14.0"
pass "yard node unuse falls back to the .nvmrc"

# 6. A pin that is not installed fails with the install hint, and never downloads.
printf '16.20.2\n' >"$project/.nvmrc"
if shim node -v >"$work/missing.out" 2>"$work/missing.err"; then
  fail "node -v with .nvmrc 16.20.2 succeeded: $(cat "$work/missing.out")"
fi
grep -q 'yard node install 16.20.2' "$work/missing.err" || fail "node -v said: $(cat "$work/missing.err")"
[[ ! -e $versions_dir/v16.20.2 ]] || fail "the node shim installed 16.20.2"
printf '16.14.0\n' >"$project/.nvmrc"
pass "a pin that is not installed names yard node install 16.20.2"

# 7. yard node uninstall refuses a version the site still needs.
if "$yard" node uninstall 16.14.0 --json >"$work/uninstall.out" 2>&1; then
  fail "yard node uninstall 16.14.0 succeeded while $site pins it"
fi
grep -q 'NODE_IN_USE' "$work/uninstall.out" || fail "yard node uninstall said: $(cat "$work/uninstall.out")"
[[ -x $versions_dir/v16.14.0/installation/bin/node ]] || fail "the refused uninstall removed 16.14.0"
pass "yard node uninstall 16.14.0 is refused with NODE_IN_USE"

# 8. Clean up, then the configs, the Node versions, nvm's and Herd's Node and Herd's
# resolver, after.
trap 'rm -rf "$work"' EXIT
cleanup
for f in yard.toml sites.toml; do
  if [[ -f $work/$f.bak ]]; then cmp -s "$work/$f.bak" "$data/$f" || fail "$data/$f was not restored"; fi
done
pass "yard.toml and sites.toml are restored"
cmp -s "$work/versions-before.txt" "$work/versions-after.txt" \
  || fail "Node versions before: $(tr '\n' ' ' <"$work/versions-before.txt"), after: $(tr '\n' ' ' <"$work/versions-after.txt")"
pass "only the Node versions this run installed were removed"
nvm_listing >"$work/nvm-after.txt"
cmp -s "$work/nvm-before.txt" "$work/nvm-after.txt" || fail "nvm's or Herd's Node versions changed: $(diff "$work/nvm-before.txt" "$work/nvm-after.txt")"
pass "nvm's and Herd's Node versions are unchanged"
[[ $herd_before == "$(sum "$herd_resolver")" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
