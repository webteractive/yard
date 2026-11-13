#!/usr/bin/env bash
# Builds a Yard release (M7 spec §8.1): one Yard.app carrying yard, yardd, yard-helper and
# yard-shim, Developer ID signed inside out with the hardened runtime, notarized and stapled,
# then a DMG around it (signed, notarized, stapled), a zip of the stapled app for installs and
# updates, SHA256SUMS and latest.json, in dist/<version>/.
#
# Signing reads nothing from this repo: the identity is the one "Developer ID Application"
# certificate in the keychain, found at run time and used by its SHA-1 hash, and notarytool
# authenticates through a keychain profile. Override them with YARD_SIGN_IDENTITY (a SHA-1
# hash) and YARD_NOTARY_PROFILE (default: notary).
#
# Usage:
#   scripts/release.sh X.Y.Z               build, sign, notarize, then publish (see below)
#   scripts/release.sh X.Y.Z --no-publish  build, sign and notarize only; the version bump
#                                          is put back afterwards
#   scripts/release.sh X.Y.Z --adhoc       ad hoc sealed and not notarized, for a Mac without
#                                          the certificate; never published (implies
#                                          --no-publish), every file named -adhoc
#
# Publishing commits the version (when the workspaces did not already say it), tags vX.Y.Z,
# pushes, creates the GitHub release with the four assets, writes Casks/yard.rb in
# webteractive/homebrew-tap, installs the published release on this Mac with the tag's
# install.sh, checks `yard --version`, and removes dist/X.Y.Z. Run it only with the
# maintainer's approval.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"

die() { printf 'release.sh: %s\n' "$*" >&2; exit 1; }

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: scripts/release.sh X.Y.Z [--no-publish | --adhoc]"
shift
mode=release
publish=1
case "${1:-}" in
  "") ;;
  --no-publish) publish=0 ;;
  --adhoc) mode=adhoc; publish=0 ;;
  *) die "unknown option $1" ;;
esac

suffix=""
[[ "$mode" == adhoc ]] && suffix="-adhoc"
sign_identity="${YARD_SIGN_IDENTITY:-}"
notary_profile="${YARD_NOTARY_PROFILE:-notary}"
dist="$repo/dist/$version$suffix"
zip_name="Yard-$version-arm64$suffix.zip"
dmg_name="Yard-$version-arm64$suffix.dmg"
tap_remote=git@github.com:webteractive/homebrew-tap.git

# --- preflight: everything that can fail is checked before anything changes ---------------

# The version is stamped into these and put back from git, so they must hold no edits.
stamped=(Cargo.toml Cargo.lock app/Cargo.toml app/Cargo.lock)
git diff --quiet HEAD -- "${stamped[@]}" || die "${stamped[*]} have uncommitted changes"
if [[ "$mode" == release ]]; then
  if [[ -z "$sign_identity" ]]; then
    found="$(security find-identity -v -p codesigning | grep '"Developer ID Application: ' || true)"
    case "$(printf '%s' "$found" | grep -c . || true)" in
      0) die "no valid \"Developer ID Application\" certificate in the keychain" ;;
      1) sign_identity="$(printf '%s' "$found" | awk '{print $2}')" ;;
      *) die "several Developer ID Application certificates: pick one with YARD_SIGN_IDENTITY=<sha1>" ;;
    esac
  else
    security find-identity -v -p codesigning | grep -qF "$sign_identity" \
      || die "YARD_SIGN_IDENTITY is not a valid signing identity in the keychain"
  fi
  xcrun notarytool history --keychain-profile "$notary_profile" >/dev/null 2>&1 \
    || die "the notary keychain profile '$notary_profile' is missing or rejected: xcrun notarytool store-credentials $notary_profile"
fi
if [[ "$publish" == 1 ]]; then
  [[ -z "$(git status --porcelain)" ]] || die "the working tree is not clean"
  [[ "$(git rev-parse --abbrev-ref HEAD)" == main ]] || die "releases are cut from main"
  git fetch -q origin
  [[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || die "main is not the same as origin/main"
  git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "v$version is already tagged"
  last="$(git tag --list 'v*' --sort=-v:refname | head -1)"
  if [[ -n "$last" ]]; then
    newest="$(printf '%s\n%s\n' "${last#v}" "$version" | sort -V | tail -1)"
    [[ "$newest" == "$version" && "${last#v}" != "$version" ]] || die "$version is not newer than $last"
  fi
  gh auth status >/dev/null 2>&1 || die "gh is not signed in"
  for r in webteractive/yard webteractive/homebrew-tap; do
    [[ "$(gh repo view "$r" --json visibility -q .visibility 2>/dev/null)" == PUBLIC ]] \
      || die "$r is not public: install.sh, latest.json and the cask would all 404"
  done
  # git pushes over SSH, which gh's sign-in says nothing about.
  for remote in origin "$tap_remote"; do
    git ls-remote -q "$remote" HEAD >/dev/null 2>&1 \
      || die "git cannot reach $remote over SSH (is the agent holding a key? ssh-add -l)"
  done
  [[ -f .release-scrub ]] || die ".release-scrub is missing (the maintainer's names and paths to keep out)"
  scripts/scrub-check.sh || die "a tracked file still names the maintainer"
  (cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings \
    && cargo test --workspace --locked) >/dev/null || die "the root workspace's checks fail"
  (cd app && cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings \
    && cargo test --workspace --locked) >/dev/null || die "the app workspace's checks fail"
fi

# --- the version, put back on exit unless this run publishes it ----------------------------

# shellcheck disable=SC2329 # called from the EXIT traps below
restore() { git checkout -q -- "${stamped[@]}"; }
[[ "$publish" == 1 ]] || trap restore EXIT
for manifest in Cargo.toml app/Cargo.toml; do
  # Only the [workspace.package] version line, which is the first `version =` in each file.
  sed -i '' -E "1,/^version = \"[^\"]+\"/s/^version = \"[^\"]+\"/version = \"$version\"/" "$manifest"
done
cargo update --workspace --offline -q
(cd app && cargo update --workspace --offline -q -p yard-proto -p yard-core -p yard-client)

# --- build ----------------------------------------------------------------------------------

cargo build --release --locked -p yard-cli -p yard-daemon -p yard-helper -p yard-shim
app/scripts/bundle.sh --release "$repo/target/release" >/dev/null
for bin in yard yardd yard-helper; do
  got="$("app/target/Yard.app/Contents/Helpers/$bin" --version 2>/dev/null | head -1 || true)"
  [[ "$got" == *"$version"* ]] || die "Helpers/$bin reports '$got', not $version"
done

rm -rf "$dist"
mkdir -p "$dist"
app="$dist/Yard.app"
ditto app/target/Yard.app "$app"

# --- sign inside out, notarize and staple the app --------------------------------------------

if [[ "$mode" == release ]]; then
  sign() { codesign --force --timestamp --options runtime --sign "$sign_identity" "$1"; }
else
  sign() { codesign --force --options runtime --sign - "$1"; }
fi
for bin in "$app"/Contents/Helpers/*; do sign "$bin"; done
sign "$app/Contents/MacOS/Yard"
sign "$app"
codesign --verify --deep --strict "$app"

# notarytool exits 0 for an Invalid verdict too, so the status is read from its JSON.
notarize() {
  local out id status
  out="$(xcrun notarytool submit "$1" --keychain-profile "$notary_profile" --wait --output-format json)" \
    || die "notarytool submit failed for $1"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - -)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - -)"
  if [[ "$status" != Accepted ]]; then
    xcrun notarytool log "$id" --keychain-profile "$notary_profile" >&2 || true
    die "notarization of $1 ended '$status' (submission $id)"
  fi
  echo "notarized $(basename "$1") ($id)"
}

work="$(mktemp -d)"
trap 'rm -rf "$work"; [[ "$publish" == 1 ]] || restore' EXIT
if [[ "$mode" == release ]]; then
  ditto -c -k --keepParent "$app" "$work/Yard.zip"
  notarize "$work/Yard.zip"
  xcrun stapler staple "$app"
  spctl -a -vvv -t exec "$app"
fi

# --- the DMG around the stapled app, then the zip of it ---------------------------------------

mkdir "$work/dmg"
ditto "$app" "$work/dmg/Yard.app"
ln -s /Applications "$work/dmg/Applications"
hdiutil create -quiet -volname "Yard $version" -srcfolder "$work/dmg" -ov -format UDZO "$dist/$dmg_name"
if [[ "$mode" == release ]]; then
  codesign --force --timestamp --sign "$sign_identity" "$dist/$dmg_name"
  notarize "$dist/$dmg_name"
  xcrun stapler staple "$dist/$dmg_name"
  spctl -a -vvv -t open --context context:primary-signature "$dist/$dmg_name"
fi
(cd "$dist" && ditto -c -k --keepParent Yard.app "$zip_name")
rm -rf "$app"

# --- checksums and the feed, last: stapling rewrites what it staples --------------------------

(cd "$dist" && shasum -a 256 "$zip_name" "$dmg_name" > SHA256SUMS)
zip_sha="$(shasum -a 256 "$dist/$zip_name" | awk '{print $1}')"
base="https://github.com/webteractive/yard/releases/download/v$version"
printf '{\n  "version": "%s",\n  "zip": "%s/%s",\n  "sha256": "%s",\n  "notes_url": "https://github.com/webteractive/yard/releases/tag/v%s"\n}\n' \
  "$version" "$base" "$zip_name" "$zip_sha" "$version" > "$dist/latest.json"

if [[ "$publish" == 0 ]]; then
  label="Developer ID signed, notarized, stapled"
  [[ "$mode" == adhoc ]] && label="AD HOC, not notarized: never ship it"
  echo "built $dist ($label); the version bump was put back"
  exit 0
fi

# --- publish (§8.1 steps 7–8) -----------------------------------------------------------------

tag="v$version"
git add -- "${stamped[@]}"
if git diff --cached --quiet; then
  echo "the workspaces already say $version: tagging HEAD"
else
  git commit -q -m "release $version"
fi
git tag -a "$tag" -m "Yard $version"
git push -q --atomic origin main "refs/tags/$tag"

notes="$work/notes.md"
if [[ -n "$last" ]]; then
  git log --no-merges --format='- %s' "$last..$tag" | grep -vx -- "- release $version" > "$notes" || true
else
  printf 'The first public release of Yard.\n' > "$notes"
fi
gh release create "$tag" --repo webteractive/yard --verify-tag --title "Yard $version" \
  --notes-file "$notes" "$dist/$zip_name" "$dist/$dmg_name" "$dist/SHA256SUMS" "$dist/latest.json"
assets="$(gh release view "$tag" --repo webteractive/yard --json assets -q '.assets | length')"
[[ "$assets" == 4 ]] || die "release $tag has $assets assets, not 4"

# The cask points at the DMG, so it carries the DMG's checksum.
dmg_sha="$(shasum -a 256 "$dist/$dmg_name" | awk '{print $1}')"
tap="$work/tap"
git clone -q "$tap_remote" "$tap"
mkdir -p "$tap/Casks"
{
  echo "# Written by scripts/release.sh in webteractive/yard: change packaging/yard.rb.tmpl there."
  sed -n '/^cask "yard"/,$p' packaging/yard.rb.tmpl | sed -e "s/@VERSION@/$version/" -e "s/@SHA256@/$dmg_sha/"
} > "$tap/Casks/yard.rb"
ruby -c "$tap/Casks/yard.rb" >/dev/null || die "the rendered cask is not valid Ruby"
git -C "$tap" add Casks/yard.rb
git -C "$tap" commit -q -m "yard $version"
git -C "$tap" push -q origin HEAD
echo "published $tag and the cask at $version"

# Install what was published, through the published installer, and check it (release-install rule).
curl -fsSL "https://raw.githubusercontent.com/webteractive/yard/$tag/install.sh" | sh \
  || die "install.sh failed on the published release"
got="$("$HOME/.local/bin/yard" --version 2>/dev/null | head -1 || true)"
[[ "$got" == *"$version"* ]] || die "the installed yard reports '$got', not $version"
echo "installed: $got"
rm -rf "$dist"
echo "removed $dist"
