#!/usr/bin/env bash
# Checks install.sh's refusals against a fake release served over file:// (M7 plan Task 7).
# Nothing here installs anything: every case must stop before the app is placed, and the
# temp Applications directory must stay empty. The signed path is checked against a real
# release build instead (the plan's Step 4).
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failures=0

# A fake release: an unsigned Yard.app whose yard is a shell stub, zipped as release.sh does.
release="$tmp/release"
mkdir -p "$release" "$tmp/build/Yard.app/Contents/Helpers"
printf '#!/bin/sh\necho "yard 0.0.1"\n' > "$tmp/build/Yard.app/Contents/Helpers/yard"
chmod +x "$tmp/build/Yard.app/Contents/Helpers/yard"
(cd "$tmp/build" && ditto -c -k --keepParent Yard.app "$release/Yard-0.0.1-arm64.zip")
sha="$(shasum -a 256 "$release/Yard-0.0.1-arm64.zip" | awk '{print $1}')"
feed() { # feed <sha256>
  printf '{"version":"0.0.1","zip":"file://%s/Yard-0.0.1-arm64.zip","sha256":"%s","notes_url":"x"}\n' \
    "$release" "$1" > "$release/latest.json"
}

# A fake Homebrew prefix with a brew, and a directory for PATH stubs.
brew_prefix="$tmp/brew"
mkdir -p "$brew_prefix/bin" "$tmp/stubs"
printf '#!/bin/sh\nexit 0\n' > "$brew_prefix/bin/brew"
chmod +x "$brew_prefix/bin/brew"

# expect_refusal <name> <text the error must contain> [env...]
expect_refusal() {
  local name="$1" want="$2"
  shift 2
  local apps="$tmp/apps-$name" home="$tmp/home-$name" out status=0
  mkdir -p "$apps" "$home"
  out="$(env HOME="$home" YARD_APPS_DIR="$apps" YARD_BREW_PREFIX="$brew_prefix" \
    YARD_RELEASE_URL="file://$release" PATH="$tmp/stubs:$PATH" "$@" sh "$repo/install.sh" 2>&1)" || status=$?
  if [[ "$status" == 0 ]]; then
    echo "FAIL  $name: install.sh succeeded"; failures=$((failures + 1))
  elif [[ "$out" != *"$want"* ]]; then
    echo "FAIL  $name: wanted '$want' in: $out"; failures=$((failures + 1))
  elif [[ -n "$(ls -A "$apps")" || -L "$home/.local/bin/yard" ]]; then
    echo "FAIL  $name: something was installed"; failures=$((failures + 1))
  else
    echo "PASS  $name"
  fi
  rm -f "$tmp/stubs/"*
}

feed "$sha"

# shellcheck disable=SC2016 # $1 belongs to the stub
printf '#!/bin/sh\nif [ "$1" = -m ]; then echo x86_64; else echo Darwin; fi\n' > "$tmp/stubs/uname"
chmod +x "$tmp/stubs/uname"
expect_refusal refuses_intel "arm64"

expect_refusal refuses_without_brew "install Homebrew first" YARD_BREW_PREFIX="$tmp/nobrew"

mkdir -p "$brew_prefix/Caskroom/yard"
expect_refusal refuses_a_cask_install "brew upgrade --cask yard"
rmdir "$brew_prefix/Caskroom/yard"

feed 0000000000000000000000000000000000000000000000000000000000000000
expect_refusal refuses_a_bad_checksum "checksum"
feed "$sha"

expect_refusal refuses_an_unsigned_app "not signed"

mkdir -p "$tmp/home-refuses_a_file_at_the_link/.local/bin"
echo "not a link" > "$tmp/home-refuses_a_file_at_the_link/.local/bin/yard"
expect_refusal refuses_a_file_at_the_link "is a file, not a link"

echo
if [[ "$failures" -gt 0 ]]; then
  echo "$failures failed"
  exit 1
fi
echo "all passed"
