#!/usr/bin/env bash
# Builds Yard.app (M6 design §14.2, M7 spec §4): release build, bundle assembled from
# yard-app/macos/, plist stamped, arm64 checked.
#
#   app/scripts/bundle.sh                    a dev build for this Mac, sealed ad hoc
#   app/scripts/bundle.sh --release <dir>    the release layout: also copies yard, yardd,
#                                            yard-helper and yard-shim from <dir> (the root
#                                            workspace's target/release) into
#                                            Contents/Helpers/, and leaves signing to
#                                            scripts/release.sh
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$app_dir"

helpers_from=""
case "${1:-}" in
  "") ;;
  --release)
    helpers_from="${2:?usage: bundle.sh --release <root target/release dir>}"
    helpers_from="$(cd "$helpers_from" && pwd)"
    ;;
  *) echo "usage: bundle.sh [--release <dir>]" >&2; exit 2 ;;
esac

cargo build --release --locked -p yard-app

version="$(cargo metadata --no-deps --format-version 1 | sed -E 's/.*"name":"yard-app","version":"([^"]+)".*/\1/')"
commit="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
build="$(git rev-list --count HEAD 2>/dev/null || echo 0)"

bundle="$app_dir/target/Yard.app"
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "$app_dir/target/release/yard-app" "$bundle/Contents/MacOS/Yard"
cp "$app_dir/yard-app/macos/Info.plist" "$bundle/Contents/Info.plist"
cp "$app_dir/yard-app/macos/AppIcon.icns" "$bundle/Contents/Resources/AppIcon.icns"

plist="$bundle/Contents/Info.plist"
plutil -replace CFBundleIdentifier -string dev.more.yard.app "$plist"
plutil -replace LSUIElement -bool true "$plist"
plutil -replace CFBundleShortVersionString -string "$version" "$plist"
plutil -replace CFBundleVersion -string "$build" "$plist"
plutil -replace YardBuildCommit -string "$commit" "$plist"
plutil -replace LSMinimumSystemVersion -string 14.0 "$plist"
plutil -lint "$plist" >/dev/null

executables=("$bundle/Contents/MacOS/Yard")
if [[ -n "$helpers_from" ]]; then
  mkdir -p "$bundle/Contents/Helpers"
  for bin in yard yardd yard-helper yard-shim; do
    cp "$helpers_from/$bin" "$bundle/Contents/Helpers/$bin"
    executables+=("$bundle/Contents/Helpers/$bin")
  done
fi

for exe in "${executables[@]}"; do
  archs="$(lipo -archs "$exe")"
  if [[ "$archs" != "arm64" ]]; then
    echo "bundle.sh: $(basename "$exe") is built for '$archs', not arm64 alone" >&2
    exit 1
  fi
done

if [[ -n "$helpers_from" ]]; then
  echo "$bundle"
  exit 0
fi
codesign --force --sign - "$bundle"
echo "ad-hoc build for this Mac; not for shipping"
echo "$bundle"
