#!/usr/bin/env bash
# Checks what release.sh's exit handler takes back after a failed publishing run, in a scratch
# repository: release.sh's own restore and on_exit functions, read out of it, run at each point
# a publish can stop. No release is built and nothing is pushed.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failures=0
functions="$tmp/functions.sh"
awk '/^restore\(\) /{print; next} /^on_exit\(\) \{/{f=1} f{print} f&&/^\}/{f=0}' scripts/release.sh > "$functions"
grep -q '^on_exit()' "$functions" || { echo "FAIL  on_exit not found in release.sh"; exit 1; }

# scenario <name> <committed> <tagged> <pushed> <status>: runs on_exit as a run that got that far.
scenario() {
  local dir="$tmp/$1"
  git init -q "$dir"
  git -C "$dir" -c user.name=t -c user.email=t@t commit -q --allow-empty -m base
  printf 'version = "0.1.0"\n' > "$dir/Cargo.toml"
  git -C "$dir" add Cargo.toml
  git -C "$dir" -c user.name=t -c user.email=t@t commit -q -m "0.1.0"
  # shellcheck disable=SC2034 # read by the expectations
  base="$(git -C "$dir" rev-parse HEAD)"
  printf 'version = "0.2.0"\n' > "$dir/Cargo.toml"
  if [[ "$2" == 1 ]]; then
    git -C "$dir" add Cargo.toml
    git -C "$dir" -c user.name=t -c user.email=t@t commit -q -m "release 0.2.0"
  fi
  [[ "$3" == 0 ]] || git -C "$dir" -c user.name=t -c user.email=t@t tag -a v0.2.0 -m x
  (
    cd "$dir"
    # shellcheck disable=SC2034 # what release.sh has set when on_exit runs
    stamped=(Cargo.toml) publish=1 version=0.2.0 work="" committed="$2" tagged="$3" pushed="$4"
    # shellcheck source=/dev/null
    source "$functions"
    (exit "$5") || on_exit
  ) 2>/dev/null || true
}
expect() { # expect <name> <condition> <message>
  if (cd "$tmp/$1" && eval "$2"); then echo "PASS  $1"; else echo "FAIL  $1: $3"; failures=$((failures + 1)); fi
}

scenario fails_before_committing 0 0 0 1
# shellcheck disable=SC2016 # expect evaluates it
expect fails_before_committing '[[ $(git rev-parse HEAD) == "$base" && -z $(git status --porcelain) ]]' "the bump was left"

scenario fails_after_tagging 1 1 0 1
# shellcheck disable=SC2016 # expect evaluates it
expect fails_after_tagging '[[ $(git rev-parse HEAD) == "$base" && -z $(git status --porcelain) && -z $(git tag) ]]' "the commit, tag or bump was left"

scenario fails_after_pushing 1 1 1 1
# shellcheck disable=SC2016 # expect evaluates it
expect fails_after_pushing '[[ $(git rev-parse HEAD) != "$base" && -n $(git tag) ]]' "a pushed release was undone locally"

scenario succeeds 1 1 1 0
# shellcheck disable=SC2016 # expect evaluates it
expect succeeds '[[ $(git rev-parse HEAD) != "$base" && -n $(git tag) ]]' "a published release was undone"

echo
if [[ "$failures" -gt 0 ]]; then echo "$failures failed"; exit 1; fi
echo "all passed"
