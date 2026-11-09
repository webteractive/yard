#!/bin/sh
# Installs Yard: Yard.app, which carries the yard CLI, yardd, the root helper and the shims,
# into /Applications (or ~/Applications when /Applications is not writable), and links
# ~/.local/bin/yard into it. Run it again to reinstall; a Homebrew install updates with
# `brew upgrade --cask yard` instead.
#
#   curl -fsSL https://raw.githubusercontent.com/webteractive/yard/main/install.sh | sh
#
# YARD_VERSION=X.Y.Z installs that release instead of the latest.
set -eu

die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

if [ -n "${YARD_VERSION:-}" ]; then
  base="https://github.com/webteractive/yard/releases/download/v$YARD_VERSION"
else
  base="${YARD_RELEASE_URL:-https://github.com/webteractive/yard/releases/latest/download}"
fi
brew_prefix="${YARD_BREW_PREFIX:-/opt/homebrew}"

# 1. This Mac.
[ "$(uname -s)" = Darwin ] || die "Yard runs on macOS only"
arch="$(uname -m)"
[ "$arch" = arm64 ] || die "Yard needs an Apple Silicon Mac (arm64); this one is $arch"
macos="$(sw_vers -productVersion)"
[ "${macos%%.*}" -ge 14 ] || die "Yard needs macOS 14 or later; this is $macos"

# 2. Homebrew, which Yard installs PHP, nginx and services from.
[ -x "$brew_prefix/bin/brew" ] || die "install Homebrew first: https://brew.sh"

# 3. Not over a Homebrew install of Yard, which brew owns.
[ -d "$brew_prefix/Caskroom/yard" ] && die "Yard was installed with Homebrew: brew upgrade --cask yard"

if [ -n "${YARD_APPS_DIR:-}" ]; then
  apps="$YARD_APPS_DIR"
elif [ -w /Applications ]; then
  apps=/Applications
else
  apps="$HOME/Applications"
  mkdir -p "$apps"
  echo "/Applications is not writable for you, so Yard goes in $apps."
fi
target="$apps/Yard.app"
link="$HOME/.local/bin/yard"
[ -e "$link" ] && [ ! -L "$link" ] && die "$link is a file, not a link: remove it, then run this again"

# 4. The release: its checksum, its signature, and Gatekeeper's word on it.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fetch() { curl -fsSL --proto '=https,file' "$1" -o "$2" || die "cannot download $1"; }
fetch "$base/latest.json" "$tmp/latest.json"
field() { plutil -extract "$1" raw -o - "$tmp/latest.json" 2>/dev/null || die "the release feed has no $1"; }
version="$(field version)"
zip_url="$(field zip)"
sha="$(field sha256)"
echo "Installing Yard $version..."
fetch "$zip_url" "$tmp/Yard.zip"
got="$(shasum -a 256 "$tmp/Yard.zip" | awk '{print $1}')"
[ "$got" = "$sha" ] || die "the download does not match its checksum ($got, not $sha)"
ditto -x -k "$tmp/Yard.zip" "$tmp/unpacked"
app="$tmp/unpacked/Yard.app"
[ -x "$app/Contents/Helpers/yard" ] || die "the download holds no Yard.app"
codesign --verify --deep --strict "$app" 2>/dev/null || die "Yard $version is not signed properly; not installing it"
spctl --assess --type execute "$app" 2>/dev/null || die "Gatekeeper does not accept Yard $version; not installing it"

# 5. Into place: staged beside the target, so the swap is two renames on one volume. A yardd
# the LaunchAgent runs from the old app is restarted on the new one.
stage="$apps/.Yard.app.new-$$"
rm -rf "$stage"
ditto "$app" "$stage"
if [ -e "$target" ]; then
  old="$apps/.Yard.app.old-$$"
  mv "$target" "$old"
  mv "$stage" "$target" || { mv "$old" "$target"; die "cannot move Yard.app into $apps"; }
  rm -rf "$old"
else
  mv "$stage" "$target"
fi
agent="gui/$(id -u)/dev.more.yard.daemon"
if launchctl print "$agent" 2>/dev/null | grep -qF "$target/Contents/Helpers/yardd"; then
  launchctl kickstart -k "$agent" && echo "yardd restarted on Yard $version."
fi

# 6. The yard command.
mkdir -p "$(dirname "$link")"
if [ -L "$link" ]; then
  was="$(readlink "$link")"
  [ "$was" = "$target/Contents/Helpers/yard" ] || echo "Replacing $link (it pointed to $was)."
fi
ln -sfn "$target/Contents/Helpers/yard" "$link"

# 7. What comes next.
echo "Yard $version is installed: $target, with yard linked at $link."
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Add ~/.local/bin to your PATH, e.g.: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc" ;;
esac
cat <<'NEXT'
Next:
  yard start                  # installs nginx and dnsmasq if needed, then starts yardd
  sudo yard helper install    # once, in a terminal: resolver files and ports 80/443
  yard doctor                 # every check should say ok
NEXT
