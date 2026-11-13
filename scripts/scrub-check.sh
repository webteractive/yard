#!/usr/bin/env bash
# Fails when a tracked file (or, with --history, any file in any commit, or any commit
# message) contains a word from .release-scrub-words: the maintainer's names, logins and the
# like, kept out of the public repository (M7 spec §9). Both scrub files are git-ignored, so
# this script names nobody; it prints file:line (or commit) for each hit, never the text.
set -euo pipefail
cd "$(dirname "$0")/.."
words=.release-scrub-words
[[ -s "$words" ]] || { echo "scrub-check: $words is missing or empty" >&2; exit 2; }
# Each word is matched as the text it is (-F), one -e per word, so "." or "+" in a word is
# that character and not a pattern.
args=()
while IFS= read -r word || [[ -n "$word" ]]; do
  [[ -z "${word//[[:space:]]/}" ]] || args+=(-e "$word")
done < "$words"
[[ ${#args[@]} -gt 0 ]] || { echo "scrub-check: $words holds no words" >&2; exit 2; }

hits=0
if [[ "${1:-}" == --history ]]; then
  while read -r commit; do
    if git grep -q -F -i -I "${args[@]}" "$commit" -- . 2>/dev/null; then
      git grep -l -F -i -I "${args[@]}" "$commit" -- . | sed 's/^/file in /'
      hits=1
    fi
    if git log -1 --format=%B "$commit" | grep -q -F -i "${args[@]}"; then
      echo "message of $commit"
      hits=1
    fi
  done < <(git rev-list --all)
else
  if git grep -q -F -i -I "${args[@]}" -- . ':!.release-scrub*'; then
    git grep -n -F -i -I "${args[@]}" -- . ':!.release-scrub*' | cut -d: -f1,2
    hits=1
  fi
fi
if [[ "$hits" == 1 ]]; then
  echo "scrub-check: the files above still name the maintainer" >&2
  exit 1
fi
echo "scrub-check: clean"
