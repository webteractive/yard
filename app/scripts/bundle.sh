#!/usr/bin/env bash
# Builds Yard.app for this Mac (M6 design §14.2): release build, bundle assembled from
# yard-app/macos/, plist stamped, arm64 checked, sealed ad hoc. Not for shipping: M7 grows this
# into Developer ID signing and notarization.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$app_dir"

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

archs="$(lipo -archs "$bundle/Contents/MacOS/Yard")"
if [[ "$archs" != "arm64" ]]; then
  echo "bundle.sh: Yard is built for '$archs', not arm64 alone" >&2
  exit 1
fi

codesign --force --sign - "$bundle"
echo "ad-hoc build for this Mac; not for shipping"
echo "$bundle"
