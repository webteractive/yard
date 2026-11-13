#!/usr/bin/env bash
# Checks scrub-check.sh in a scratch repository with made-up words: a word is matched as the
# text it is, so a "." or "+" in it is that character, not a pattern.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failures=0

git -C "$tmp" init -q
mkdir "$tmp/scripts"
cp "$repo/scripts/scrub-check.sh" "$tmp/scripts/"
printf 'a.c\nx+y\n' > "$tmp/.release-scrub-words"
printf '.release-scrub*\n' > "$tmp/.gitignore"
check() { # check <name> <want exit> <file text>
  printf '%s\n' "$3" > "$tmp/notes.txt"
  git -C "$tmp" add notes.txt .gitignore
  local status=0
  "$tmp/scripts/scrub-check.sh" >/dev/null 2>&1 || status=$?
  if [[ "$status" == "$2" ]]; then echo "PASS  $1"; else echo "FAIL  $1: exit $status, wanted $2"; failures=$((failures + 1)); fi
}

check a_dot_is_a_dot 0 "abc and xxy are not the words"
check the_word_itself_is_found 1 "here is a.c"
check a_plus_is_a_plus 1 "and x+y too"
check case_does_not_matter 1 "A.C"

echo
if [[ "$failures" -gt 0 ]]; then echo "$failures failed"; exit 1; fi
echo "all passed"
