#!/usr/bin/env bash
# Fails when a tracked file (or, with --history, any file in any commit, or any commit
# message) contains a word from .release-scrub-words: the maintainer's names, logins and the
# like, kept out of the public repository (M7 spec §9). Both scrub files are git-ignored, so
# this script names nobody; it prints file:line (or commit) for each hit, never the text.
set -euo pipefail
cd "$(dirname "$0")/.."
words=.release-scrub-words
[[ -s "$words" ]] || { echo "scrub-check: $words is missing or empty" >&2; exit 2; }
pattern="$(grep -v '^[[:space:]]*$' "$words" | paste -sd'|' -)"

hits=0
if [[ "${1:-}" == --history ]]; then
  while read -r commit; do
    if git grep -q -i -I -E "$pattern" "$commit" -- . 2>/dev/null; then
      git grep -l -i -I -E "$pattern" "$commit" -- . | sed 's/^/file in /'
      hits=1
    fi
    if git log -1 --format=%B "$commit" | grep -q -i -E "$pattern"; then
      echo "message of $commit"
      hits=1
    fi
  done < <(git rev-list --all)
else
  if git grep -n -i -I -E "$pattern" -- . ':!.release-scrub*' >/dev/null; then
    git grep -n -i -I -E "$pattern" -- . | cut -d: -f1,2
    hits=1
  fi
fi
if [[ "$hits" == 1 ]]; then
  echo "scrub-check: the files above still name the maintainer" >&2
  exit 1
fi
echo "scrub-check: clean"
