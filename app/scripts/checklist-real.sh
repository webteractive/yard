#!/usr/bin/env bash
# The M6 checklist items that change nothing, against the real yardd on ~/.yard (plan Task 17
# Step 4, run only with the maintainer's approval): M3, O1, R1, D1, A1, S4 on a scratch site, R3 where Herd
# is installed, and M1 last, which stops and starts the real yardd (every site is down for a few
# seconds; the LaunchAgent's yardd comes back as the build in target/release).
#
#   app/scripts/checklist-real.sh
#
# Needs: target/release/yard (the yardd LaunchAgent installed by it), app/target/Yard.app, jq, and
# Accessibility and Screen Recording for the terminal. It refuses a running Yard.app, backs up
# yard.toml, sites.toml, ports.toml and services.toml and restores them (cp -p, cmp), and links,
# secures and unlinks one scratch site (`chk-real`, in this script's temp directory), deleting the
# override and certificate Yard made for it.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$app_dir/.." && pwd)"
real="$HOME/.yard"
out="${TMPDIR:-/tmp}/yard-check-real-$(date +%Y%m%d-%H%M%S)"
yard_bin="$repo/target/release/yard"
bundle="$app_dir/target/Yard.app"
ax_js="$app_dir/scripts/ax.js"
app_domain=dev.more.yard.app

y() { "$yard_bin" "$@"; }
ax() { osascript -l JavaScript "$ax_js" "$@"; }

passed=0
failed=0
report="$out/report.txt"
pass() { passed=$((passed + 1)); echo "PASS  $1" | tee -a "$report"; }
fail() { failed=$((failed + 1)); echo "FAIL  $1${2:+: $2}" | tee -a "$report"; }
note() { echo "      $1" | tee -a "$report"; }
die() { echo "checklist-real.sh: $1" >&2; exit 1; }

within() {
  local limit="$1"; shift
  local end=$((SECONDS + limit))
  while ((SECONDS <= end)); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.2
  done
  return 1
}
shot() {
  local frame
  frame="$(ax frame 2>/dev/null)" || return 0
  screencapture -x -R"$frame" "$out/$1.png" 2>/dev/null || true
}
now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
since() { perl -e "printf '%.2f', $1 - $2"; }
watch_pid=""
watch_start() {
  rm -f "$out/ready" "$out/watch.json"
  osascript -l JavaScript "$ax_js" watch "$out/ready" "$@" >"$out/watch.json" &
  watch_pid=$!
  within 20 test -e "$out/ready" || die "the watcher never found its element"
}
watch_end() { wait "$watch_pid" || true; at="$(jq -r '.at // empty' "$out/watch.json")"; }
menu_header() { ax menu | jq -r '.[0].name'; }
texts() { ax tree | jq -r '.[] | select(.role == "AXStaticText") | [.name, .value, .desc] | map(select(. != null and . != "")) | first // empty'; }
readable() {
  local check="$1"; shift
  local seen missing=""
  seen="$(texts)"
  for want in "$@"; do grep -qF -- "$want" <<<"$seen" || missing="$missing [$want]"; done
  if [[ -z "$missing" ]]; then pass "$check"; else fail "$check" "not exposed:$missing"; fi
}
rows() { ax tree | jq -c '[.[] | select(.role == "AXRow") | .name] | sort'; }
fact() { texts | awk -v l="$1" 'found { print; exit } $0 == l { found = 1 }'; }

# --- preflight --------------------------------------------------------------------------------

[[ -x "$yard_bin" ]] || die "$yard_bin is missing: cargo build --release"
[[ -d "$bundle" ]] || die "$bundle is missing: scripts/bundle.sh"
command -v jq >/dev/null || die "jq is missing"
osascript -l JavaScript -e 'ObjC.import("ApplicationServices"); $.AXIsProcessTrusted()' | grep -q true ||
  die "this terminal has no Accessibility permission"
pgrep -x Yard >/dev/null && die "Yard.app is already running; quit it first"
y status >/dev/null || die "the real yardd is not answering: yard start"
y sites --json | jq -e 'any(.[]; .name == "chk-real")' >/dev/null && die "a site chk-real already exists"

mkdir -p "$out"
defaults export "$app_domain" "$out/defaults.plist" 2>/dev/null || true
tomls=(yard.toml sites.toml ports.toml services.toml)
for f in "${tomls[@]}"; do
  if [[ -e "$real/$f" ]]; then cp -p "$real/$f" "$out/$f.bak"; fi
done
project="$out/projects/chk-real"
app_pid=""
linked=false

cleanup() {
  [[ -n "$app_pid" ]] && kill -TERM "$app_pid" 2>/dev/null || true
  if $linked; then
    y unsecure chk-real >/dev/null 2>&1 || true
    y unlink chk-real >/dev/null 2>&1 || true
  fi
  rm -f "$real/nginx/overrides/chk-real.conf" "$real/certs/chk-real.pem" "$real/certs/chk-real.key"
  # A yardd stopped by M1 and not yet back is started again before anything is restored.
  y status >/dev/null 2>&1 || y start >/dev/null 2>&1 || true
  for f in "${tomls[@]}"; do
    if [[ -e "$out/$f.bak" ]] && ! cmp -s "$out/$f.bak" "$real/$f"; then
      cp -p "$out/$f.bak" "$real/$f"
      echo "restored $f"
    fi
  done
  if [[ -s "$out/defaults.plist" ]]; then
    defaults import "$app_domain" "$out/defaults.plist" 2>/dev/null || true
  fi
}
trap cleanup EXIT

open -n "$bundle"
within 10 pgrep -x Yard || die "Yard.app did not start"
app_pid="$(pgrep -x Yard | head -1)"
within 60 sh -c "osascript -l JavaScript '$ax_js' menu | jq -e 'any(.[]; .name == \"Sites\")'" ||
  die "the menu bar menu never listed the sites"
echo "Yard.app pid $app_pid against $real; screenshots in $out" | tee "$report"

# --- M3: the Sites submenu --------------------------------------------------------------------

menu="$(ax menu)"
sites_cli="$(y sites --json | jq -c '[.[].name] | sort')"
sites_menu="$(jq -c '[.[] | select(.name == "Sites") | .children[].name] | sort' <<<"$menu")"
if [[ "$sites_menu" == "$sites_cli" ]]; then pass "M3 the Sites submenu lists every site ($(jq length <<<"$sites_cli"))"; else fail "M3 Sites submenu" "menu $sites_menu, cli $sites_cli"; fi

# --- O1, A1: the Overview ---------------------------------------------------------------------

ax menu-click "Open Yard" >/dev/null
within 5 ax tree || die "Open Yard did not open the main window"
ax tab Overview >/dev/null
sleep 0.5
shot overview
want="$(y status --json | jq -r '"\(.version.daemon) (\(.version.commit))"')"
got="$(fact yardd)"
if [[ "$got" == "$want" ]]; then pass "O1 the Overview shows the real yardd's version and commit ($got)"; else fail "O1 yardd" "app $got, yard status $want"; fi
got="$(fact Yard.app)"
app_commit="$(git -C "$repo" rev-parse --short HEAD)"
if [[ "$got" == *"($app_commit)" ]]; then pass "O1 and Yard.app's ($got)"; else fail "O1 Yard.app" "app $got, HEAD $app_commit"; fi
readable "A1 the Overview's facts are readable text" "yardd" "helper" "CA" "Data root" "$real"

# --- R1: the Ports table ----------------------------------------------------------------------

ax tab Ports >/dev/null
sleep 0.5
shot ports
want="$(y ports --json | jq -c '[.[].unit] | sort')"
got="$(rows)"
if [[ "$got" == "$want" ]]; then pass "R1 the Ports table lists every unit ($(jq length <<<"$want"))"; else fail "R1 Ports table" "app $got, cli $want"; fi
holders="$(y ports --json | jq -r '[.[] | select(.holder != null) | .holder.name] | unique | join(", ")')"
[[ -n "$holders" ]] && note "R1 ports held by other programs: $holders"

# --- R3: the privileged profile is refused where Herd is installed ---------------------------

if [[ -d /Applications/Herd.app || -e /etc/resolver/test ]]; then
  ax pick "Ports profile" "privileged (80, 443)" >/dev/null 2>&1 || true
  if within 10 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .subrole == \"AXApplicationAlert\" and (.name | test(\"Herd\")))'"; then
    pass "R3 the privileged profile is refused, naming Herd"
  else
    fail "R3 no refusal naming Herd" "$(y ports --json | jq -r '.[0].mode')"
  fi
  [[ "$(y status --json | jq -r .profile)" == standard ]] || die "the profile changed: put it back with yard ports profile standard"
else
  note "R3 skipped: no Herd on this Mac, where the switch would go through"
fi

# --- D1: Doctor -------------------------------------------------------------------------------

ax tab Doctor >/dev/null
ax press "Run doctor" >/dev/null
if within 60 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; (.value // \"\") | test(\"yardd is running\"))'"; then
  shot doctor
  pass "D1 doctor runs against the real yardd and its results are readable text"
else
  fail "D1 doctor" "$(texts | head -5 | tr '\n' '|')"
fi

# --- S4: yard secure in a terminal updates the open detail -----------------------------------

mkdir -p "$project"
echo "<?php echo 'chk-real';" >"$project/index.php"
(cd "$project" && y link chk-real >/dev/null)
linked=true
ax tab Sites >/dev/null
within 10 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .role == \"AXRow\" and .name == \"chk-real\")'" || fail "S4 chk-real never listed"
ax row chk-real >/dev/null
sleep 0.5
watch_start 5 "control:HTTPS for chk-real" is true
y secure chk-real >/dev/null
done_at="$(now)"
watch_end
if [[ -n "$at" ]] && perl -e "exit(!($at - $done_at <= 1))"; then
  pass "S4 yard secure turns the open detail's HTTPS switch on $(since "$at" "$done_at") s after it returns"
else
  fail "S4 HTTPS switch after yard secure" "$(cat "$out/watch.json")"
fi
y unsecure chk-real >/dev/null
y unlink chk-real >/dev/null
linked=false
rm -f "$real/nginx/overrides/chk-real.conf" "$real/certs/chk-real.pem" "$real/certs/chk-real.key"

# --- M1: yardd stops and starts (last: every site is down for a few seconds) -----------------

ok_header="$(menu_header)"
icon="$(ax icon)"
if [[ "$icon" == ok ]]; then pass "M1 the icon is the filled circle with the real yardd up"; else note "M1 icon with yardd up: $icon (a problem or a job shows another shape)"; fi
watch_start 10 menu-header not "$ok_header"
stopped_at="$(now)"
y stop >/dev/null
watch_end
if [[ -n "$at" ]] && perl -e "exit(!($at - $stopped_at <= 1))"; then
  pass "M1 the menu says \"$(jq -r .value "$out/watch.json")\" $(since "$at" "$stopped_at") s after yard stop"
else
  fail "M1 menu after yard stop" "$(cat "$out/watch.json")"
fi
icon="$(ax icon)"
if [[ "$icon" == offline ]]; then pass "M1 and the icon is the ring"; else fail "M1 icon with yardd down" "$icon"; fi
watch_start 15 menu-header not "$(menu_header)"
started_at="$(now)"
y start >/dev/null
watch_end
if [[ -n "$at" ]] && perl -e "exit(!($at - $started_at <= 6))"; then
  pass "M1 and is back $(since "$at" "$started_at") s after yard start"
else
  fail "M1 menu after yard start" "$(cat "$out/watch.json")"
fi
note "yardd now runs $(y status --json | jq -r '"\(.version.daemon) (\(.version.commit))"')"

kill -TERM "$app_pid" 2>/dev/null || true
app_pid=""

echo
echo "$passed passed, $failed failed; screenshots and report in $out" | tee -a "$report"
((failed == 0))
