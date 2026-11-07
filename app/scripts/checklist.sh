#!/usr/bin/env bash
# Runs the parts of the M6 manual checklist (plan Task 17) that a script can see, against a scratch
# yardd: it drives Yard.app through macOS accessibility (scripts/ax.js), compares what the app shows
# with the yard CLI, and saves a screenshot of every screen. Nothing touches ~/.yard: the script
# fails if /etc/resolver, yardd's LaunchAgent or Yard.app's settings changed.
#
#   app/scripts/checklist.sh [--keep]
#
# Needs: target/release/{yard,yardd} at the repo root (cargo build --release), app/target/Yard.app
# (scripts/bundle.sh), jq, brew's redis keg, and Accessibility and Screen Recording for the
# terminal. The scratch root is $YARD_CHECK_ROOT (default /tmp/yard-check); its nginx and dnsmasq
# are pinned to 28080, 28443 and 28053, its instances to 28379-28381 and 28900, and its auto ports
# come from 29000-29099. MinIO is added and removed when brew has its keg (V1, V6). O4 runs when
# $YARD_CHECK_OLD_YARDD names a yardd built before protocol 2.11 (git 2c1ed9b) with yard-core's
# HELPER_SOCKET pointed at a path that does not exist (it predates the helper guard). --keep leaves the
# root, the daemon and the app running for a look afterwards.
#
# What it cannot check is still a person's: what VoiceOver says aloud (it checks what VoiceOver
# is given: every control's label, state and the screens' text). The items that install, share or
# need a password (T1, T2, B3, P2, P3's add and remove, N2, M5's submenu, the trust question) are
# checklist-installs.sh's, against a fake brew. N3 asks nodejs.org.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$app_dir/.." && pwd)"
root="${YARD_CHECK_ROOT:-/tmp/yard-check}"
out="${TMPDIR:-/tmp}/yard-check-$(date +%Y%m%d-%H%M%S)"
keep=false
[[ "${1:-}" == "--keep" ]] && keep=true

yard_bin="$repo/target/release/yard"
yardd_bin="$repo/target/release/yardd"
bundle="$app_dir/target/Yard.app"
ax_js="$app_dir/scripts/ax.js"

y() { "$yard_bin" --root "$root" "$@"; }
ax() { osascript -l JavaScript "$ax_js" "$@"; }

passed=0
failed=0
report="$out/report.txt"

pass() { passed=$((passed + 1)); echo "PASS  $1" | tee -a "$report"; }
fail() { failed=$((failed + 1)); echo "FAIL  $1${2:+: $2}" | tee -a "$report"; }
note() { echo "      $1" | tee -a "$report"; }
die() { echo "checklist.sh: $1" >&2; exit 1; }

# Waits up to $1 seconds for the command in the remaining arguments to succeed.
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

# Starts `ax.js watch` in the background and returns once it has found its element; its result
# lands in $out/watch.json. Reading the element is a 50 ms poll, so the times are close.
watch_pid=""
watch_start() {
  rm -f "$out/ready" "$out/watch.json"
  osascript -l JavaScript "$ax_js" watch "$out/ready" "$@" >"$out/watch.json" &
  watch_pid=$!
  within 20 test -e "$out/ready" || die "the watcher never found its element"
}
# Waits for the watcher; its time is then in $at (empty when it timed out). Not called in $(…): a
# subshell cannot wait for the watcher.
watch_end() { wait "$watch_pid" || true; at="$(jq -r '.at // empty' "$out/watch.json")"; }

window_count() {
  osascript -l JavaScript -e 'Application("System Events").processes.byName("Yard").windows().length'
}

menu_header() { ax menu | jq -r '.[0].name'; }

# The visible text of the main window that assistive technology can read, one line each.
texts() { ax tree | jq -r '.[] | select(.role == "AXStaticText") | [.name, .value, .desc] | map(select(. != null and . != "")) | first // empty'; }

# Whether the main window shows this text now.
texts_have() { texts | grep -qF -- "$1"; }

# A1: each of the given strings is readable text in the main window as it is now.
readable() {
  local check="$1"; shift
  local seen missing=""
  seen="$(texts)"
  for want in "$@"; do grep -qF -- "$want" <<<"$seen" || missing="$missing [$want]"; done
  if [[ -z "$missing" ]]; then pass "$check"; else fail "$check" "not exposed:$missing"; fi
}

# --- preflight --------------------------------------------------------------------------------

[[ "$root" == "$HOME/.yard" ]] && die "refusing to use the real data root"
for f in "$yard_bin" "$yardd_bin"; do [[ -x "$f" ]] || die "$f is missing: cargo build --release"; done
[[ -d "$bundle" ]] || die "$bundle is missing: scripts/bundle.sh"
command -v jq >/dev/null || die "jq is missing"
[[ -d "$(brew --prefix)/opt/redis" ]] || die "brew's redis keg is missing"
osascript -l JavaScript -e 'ObjC.import("ApplicationServices"); $.AXIsProcessTrusted()' | grep -q true ||
  die "this terminal has no Accessibility permission (System Settings → Privacy & Security)"
pgrep -x Yard >/dev/null && die "Yard.app is already running; quit it first (the script finds the app by name)"
if [[ -e "$root" ]]; then
  [[ -e "$root/.yard-check" ]] || die "$root exists and was not made by this script"
  pgrep -f "yardd --root $root" >/dev/null && die "a yardd is still running on $root"
  rm -rf "$root"
fi
for port in 28080 28443 28053 28379 28380 28381 28900; do
  lsof -nP -iTCP:"$port" -sTCP:LISTEN -iUDP:"$port" >/dev/null 2>&1 && die "port $port is in use"
done

# The scratch yardd must never touch this Mac's resolver: the helper serves ~/.yard.
resolver_sum() { shasum /etc/resolver/* 2>/dev/null | sort; }
resolver_before="$(resolver_sum)"
# Nor yardd's LaunchAgent: it serves ~/.yard, and yard start and stop for this root must leave it.
agent_plist="$HOME/Library/LaunchAgents/dev.more.yard.daemon.plist"
agent_sum() { shasum "$agent_plist" 2>/dev/null; }
agent_before="$(agent_sum)"
# Yard.app keeps its settings in its defaults, shared with the real app: the run puts them back.
app_domain=dev.more.yard.app
onb_root="$root-onboarding"

mkdir -p "$root" "$out"
defaults export "$app_domain" "$out/defaults.plist" 2>/dev/null || true
touch "$root/.yard-check"
cat >"$root/ports.toml" <<'EOF'
range = "29000-29099"
[ports]
nginx-http = { port = 28080, mode = "pinned" }
nginx-https = { port = 28443, mode = "pinned" }
dnsmasq = { port = 28053, mode = "pinned" }
EOF

yardd_pid=""
app_pid=""

start_yardd() {
  "$yardd_bin" --root "$root" >>"$out/yardd.log" 2>&1 &
  yardd_pid=$!
  within 15 y status || die "the scratch yardd did not start (see $out/yardd.log)"
}

stop_yardd() {
  [[ -n "$yardd_pid" ]] || return 0
  kill -TERM "$yardd_pid" 2>/dev/null || true
  wait "$yardd_pid" 2>/dev/null || true
  yardd_pid=""
}

cleanup() {
  if $keep; then
    echo "kept: root $root, yardd pid ${yardd_pid:-none}, app pid ${app_pid:-none}"
    echo "Yard.app's settings are the run's: put them back with defaults import $app_domain $out/defaults.plist"
    return
  fi
  [[ -n "$app_pid" ]] && kill -TERM "$app_pid" 2>/dev/null || true
  [[ -n "${holder_pid:-}" ]] && kill "$holder_pid" 2>/dev/null || true
  stop_yardd
  # Yard.app's settings as they were, or none when it had none.
  if [[ -s "$out/defaults.plist" ]]; then
    defaults import "$app_domain" "$out/defaults.plist" 2>/dev/null || true
  else
    defaults delete "$app_domain" 2>/dev/null || true
  fi
  rm -rf "$root" "$onb_root"
}
trap cleanup EXIT

start_yardd
for site in chk-a chk-b; do
  mkdir -p "$root/projects/$site"
  echo "<?php echo '$site ', PHP_VERSION;" >"$root/projects/$site/index.php"
  (cd "$root/projects/$site" && y link "$site" >/dev/null)
done
# A pinned port of its own: the real root's redis may be asleep with its port free.
y service add redis --port 28379 >"$out/service-add.log" 2>&1 || die "yard service add redis failed (see $out/service-add.log)"
# chk-c's committed .yard asks for what this root lacks (redis it has): the settle form (S2), the
# locks (S3).
mkdir -p "$root/projects/chk-c"
echo "<?php echo 'chk-c';" >"$root/projects/chk-c/index.php"
printf 'php = "7.1"\nextensions = ["xdebug"]\n\n[services]\npostgres = "17"\nredis = true\n' >"$root/projects/chk-c/.yard"
(cd "$root/projects/chk-c" && y link chk-c </dev/null 2>&1 || true) |
  awk '/needs:$/ { on = 1; next } /^[^ ]/ { on = 0 } on { sub(/^  /, ""); print }' >"$out/chk-c-needs.txt"

open -n "$bundle" --args --root "$root"
within 10 pgrep -f "Yard.app/Contents/MacOS/Yard --root $root" || die "Yard.app did not start"
app_pid="$(pgrep -f "Yard.app/Contents/MacOS/Yard --root $root" | head -1)"
# The menu fills in once the app has its first snapshot of sites and services. A freshly built
# bundle is scanned by macOS on its first launch, which can take most of a minute.
within 60 sh -c "osascript -l JavaScript '$ax_js' menu | jq -e 'any(.[]; .name == \"Sites\")'" ||
  die "the menu bar menu never listed the sites"
echo "Yard.app pid $app_pid against $root; screenshots in $out" | tee "$report"

# B4: with yardd answering and its LaunchAgent's yard found, onboarding does not open by itself.
if ! osascript -l JavaScript "$ax_js" tree other 2>/dev/null | jq -e 'any(.[]; .value == "Set up Yard")' >/dev/null; then
  pass "B4 onboarding does not open by itself when Yard is set up"
else
  fail "B4 onboarding opened at launch"
fi

# --- menu bar ---------------------------------------------------------------------------------

menu="$(ax menu)"
echo "$menu" >"$out/menu.json"
header="$(jq -r '.[0].name' <<<"$menu")"
if [[ "$header" == "Yard "*" · standard · .yard" ]]; then pass "M0 menu header names the version, profile and TLD"; else fail "M0 menu header" "$header"; fi

sites_cli="$(y sites --json | jq -c '[.[].name] | sort')"
sites_menu="$(jq -c '[.[] | select(.name == "Sites") | .children[].name] | sort' <<<"$menu")"
if [[ "$sites_menu" == "$sites_cli" ]]; then pass "M3 Sites submenu lists every site"; else fail "M3 Sites submenu" "menu $sites_menu, cli $sites_cli"; fi

{
  mark() { ax menu | jq -r '.. | objects | select((.name // "") | startswith("redis")) | .mark // "none"' | head -1; }
  redis_state() { y services --json | jq -r '.[] | select(.name == "redis") | .state'; }
  item="$(ax menu | jq -r '.. | objects | select((.name // "") | startswith("redis")) | .name' | head -1)"
  [[ -n "$item" ]] || die "the menu has no redis item: $(ax menu)"
  before="$(mark)"
  ax menu-click "$item" >/dev/null
  if within 10 sh -c "[ \"\$($yard_bin --root $root services --json | jq -r '.[] | select(.name == \"redis\") | .state')\" != running ]"; then
    after="$(mark)"
    if [[ "$before" == "✓" && "$after" == "none" ]]; then pass "M4 a service item stops it and its check mark follows"; else fail "M4 check mark" "before $before, after $after"; fi
  else
    fail "M4 clicking the redis item did not stop it" "state $(redis_state)"
  fi
  item="$(ax menu | jq -r '.. | objects | select((.name // "") | startswith("redis")) | .name' | head -1)"
  ax menu-click "$item" >/dev/null
  if within 15 sh -c "[ \"\$($yard_bin --root $root services --json | jq -r '.[] | select(.name == \"redis\") | .state')\" = running ]" &&
    within 3 sh -c "[ \"\$(osascript -l JavaScript $ax_js menu | jq -r '.. | objects | select((.name // \"\") | startswith(\"redis\")) | .mark // \"none\"' | head -1)\" = ✓ ]"; then
    pass "M4 and starts it again, the mark back"
  else
    fail "M4 restart from the menu" "state $(redis_state), mark $(mark)"
  fi
}
emailulator_mark="$(jq -r '.. | objects | select((.name // "") | startswith("emailulator")) | .mark // "none"' <<<"$menu" | head -1)"
if [[ -z "$emailulator_mark" || "$emailulator_mark" == "none" ]]; then pass "M4 emailulator has no check mark"; else fail "M4 emailulator mark" "$emailulator_mark"; fi
if ! jq -e 'any(.[]; .name == "Xdebug")' <<<"$menu" >/dev/null; then
  pass "M5 no Xdebug submenu while no PHP of this root loads xdebug (with one: by hand, it installs)"
else
  fail "M5 an Xdebug submenu with no xdebug" "$(jq -c '.[] | select(.name == "Xdebug")' <<<"$menu")"
fi

# --- windows: Settings and the main window open once ------------------------------------------

ax menu-click "Open Yard" >/dev/null
within 5 ax tree || die "Open Yard did not open the main window"
base="$(window_count)"
ax menu-click "Settings…" >/dev/null
sleep 0.7
ax menu-click "Settings…" >/dev/null
sleep 0.7
if [[ "$(window_count)" == $((base + 1)) ]]; then pass "Settings… twice opens one Settings window"; else fail "Settings… twice" "$base windows became $(window_count)"; fi
ax menu-click "Open Yard" >/dev/null
sleep 0.5
if [[ "$(window_count)" == $((base + 1)) ]]; then pass "Open Yard raises the main window rather than opening another"; else fail "Open Yard" "$(window_count) windows"; fi
# GPUI builds a window's accessibility tree only while it is in front: raise Settings again.
ax menu-click "Settings…" >/dev/null
sleep 0.7
ax type "Path of yard" "$yard_bin" >/dev/null
ax press Save @other >/dev/null
if within 5 sh -c "osascript -l JavaScript '$ax_js' tree other | jq -e 'any(.[]; (.value // \"\") == \"Saved. Yard.app now runs $yard_bin.\")'"; then
  pass "Settings saves a yard path and says it is the one in use"
else
  fail "Settings yard path" "$(ax tree other | jq -c '[.[] | select(.role == "AXStaticText") | .value]')"
fi
ax type "Path of yard" "" >/dev/null
ax press Save @other >/dev/null

# --- Overview ---------------------------------------------------------------------------------

ax tab Overview >/dev/null
shot overview
readable "A1 Overview's facts are readable text" "yardd" "Yard.app" "Data root" "$root"
# The value shown after a label in the Overview's facts.
fact() { texts | awk -v l="$1" 'found { print; exit } $0 == l { found = 1 }'; }
want="$(y status --json | jq -r '"\(.version.daemon) (\(.version.commit))"')"
got="$(fact yardd)"
if [[ "$got" == "$want" ]]; then pass "O1 Overview shows yardd's version and commit ($got)"; else fail "O1 yardd" "app $got, yard status $want"; fi
app_commit="$(git -C "$repo" rev-parse --short HEAD)"
got="$(fact Yard.app)"
if [[ "$got" == *"($app_commit)" ]]; then pass "O1 and Yard.app's ($got)"; else fail "O1 Yard.app" "app $got, HEAD $app_commit (rebuild the bundle?)"; fi
got="$(fact CA)"
if [[ "$got" =~ ^(trusted|not\ trusted),\ expires\ [0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  pass "O1 the Overview says whether this Mac trusts the CA ($got)"
else
  fail "O1 CA" "'$got'"
fi

cp -p "$root/sites.toml" "$out/sites.toml.bak"
printf '\n[broken\n' >>"$root/sites.toml"
if within 30 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; (.value // .name // \"\") | test(\"sites.toml\"))'"; then
  pass "O3 a broken sites.toml shows the banner: $(ax tree | jq -r '.[] | select(.subrole == "AXApplicationAlert") | .name')"
  textedit_was_running=false
  pgrep -x TextEdit >/dev/null && textedit_was_running=true
  ax press "Open file" >/dev/null
  if within 10 sh -c "osascript -l JavaScript '$ax_js' windows-of TextEdit | jq -e 'index(\"sites.toml\")'"; then
    pass "O3 the banner's Open file opens sites.toml in TextEdit"
  else
    fail "O3 Open file" "$(ax windows-of TextEdit)"
  fi
  ax close-window TextEdit sites.toml >/dev/null 2>&1 || true
  $textedit_was_running || pkill -x TextEdit || true
else
  fail "O3 no banner for a broken sites.toml"
fi
cp -p "$out/sites.toml.bak" "$root/sites.toml"
if within 30 sh -c "! osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; (.value // .name // \"\") | test(\"sites.toml\"))'"; then
  pass "O3 and fixing it clears the banner"
else
  fail "O3 the banner stayed after the fix"
fi
nginx_pid() { y status --json | jq -r '.units[] | select(.name == "nginx") | .pid'; }
old_pid="$(nginx_pid)"
ax press Restart nginx >/dev/null
if within 10 sh -c "p=\$($yard_bin --root $root status --json | jq -r '.units[] | select(.name == \"nginx\") | .pid'); [ -n \"\$p\" ] && [ \"\$p\" != null ] && [ \"\$p\" != $old_pid ]"; then
  pass "O2 Restart in the units table restarts nginx (pid $old_pid → $(nginx_pid))"
else
  fail "O2 restart nginx" "pid still $(nginx_pid)"
fi

# --- Sites ------------------------------------------------------------------------------------

ax tab Sites >/dev/null
sleep 0.5
rows() { ax tree | jq -c '[.[] | select(.role == "AXRow") | .name] | sort'; }
if [[ "$(rows)" == "$sites_cli" ]]; then pass "S1 the Sites table lists every site"; else fail "S1 Sites table" "app $(rows), cli $sites_cli"; fi

base="$(window_count)"
ax row chk-a >/dev/null
sleep 0.5
if [[ "$(window_count)" == "$base" ]]; then pass "S selecting a site shows it in the same window"; else fail "S selecting a site" "$base windows became $(window_count)"; fi
detail="$(ax tree)"
missing=""
for control in "PHP for chk-a" "HTTPS for chk-a" "Open in browser" "Edit nginx override" "Copy .env" "Unlink…"; do
  jq -e --arg n "$control" 'any(.[]; .name == $n)' <<<"$detail" >/dev/null || missing="$missing [$control]"
done
if [[ -z "$missing" ]]; then pass "S the site detail exposes its picker, switch and buttons"; else fail "S site detail" "missing$missing"; fi
shot site-detail
readable "A1 the Sites table's cells and the site detail are readable text" "http://chk-a.yard:28080/" "serving" "Docroot"

watch_start 5 "control:HTTPS for chk-a" is true
y secure chk-a >/dev/null
done_at="$(now)"
watch_end
if [[ -n "$at" ]] && perl -e "exit(!($at - $done_at <= 1))"; then
  pass "S4 yard secure in a terminal turns the open detail's HTTPS switch on $(since "$at" "$done_at") s after it returns"
else
  fail "S4 HTTPS switch after yard secure" "$(cat "$out/watch.json")"
fi
y unsecure chk-a >/dev/null 2>&1 || true

ax press "Unlink…" >/dev/null
sleep 0.5
dialog="$(ax dialog)"
if jq -e '.buttons | index("Cancel") and index("Remove")' <<<"$dialog" >/dev/null 2>&1; then
  pass "S8 Unlink asks first: $(jq -r '.text | join(" ")' <<<"$dialog")"
  ax answer Cancel >/dev/null
  sleep 0.5
  if y sites --json | jq -e 'any(.[]; .name == "chk-a")' >/dev/null; then pass "S8 and Cancel keeps the site"; else fail "S8 Cancel unlinked the site"; fi
else
  fail "S8 Unlink asked nothing" "$dialog"
fi

# --- Sites: S1 linking, S2, S3, S5, S6, S7 ---------------------------------------------------

ax tab Sites >/dev/null
# S1: Link a folder… through the open panel
mkdir -p "$root/projects/chk-new"
echo "<?php echo 'chk-new ok';" >"$root/projects/chk-new/index.php"
ax press "Link a folder…" >/dev/null
ax goto "$root/projects/chk-new" >/dev/null
if within 5 ax tree && within 5 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .name == \"Site name\")'"; then
  ax press Link >/dev/null
  if within 10 sh -c "'$yard_bin' --root '$root' sites --json | jq -e 'any(.[]; .name == \"chk-new\")'" &&
    [[ "$(curl -s --max-time 5 "http://chk-new.yard:28080/")" == "chk-new ok" ]]; then
    pass "S1 Link a folder… links the folder chosen in the open panel, and it serves"
  else
    fail "S1 link" "$(y sites --json | jq -c '[.[].name]')"
  fi
else
  fail "S1 the Link sheet never showed"
fi

# S2: the settle form lists what the CLI says, in its order
ax row chk-c >/dev/null
ax press "Install what it needs…" >/dev/null
sleep 0.5
got="$(texts | grep -F -x -f "$out/chk-c-needs.txt" || true)"
if [[ -s "$out/chk-c-needs.txt" && "$got" == "$(cat "$out/chk-c-needs.txt")" ]]; then
  pass "S2 the settle form lists the CLI's needs in its order: $(tr '\n' ';' <"$out/chk-c-needs.txt")"
else
  fail "S2 settle form" "app [$got], cli [$(cat "$out/chk-c-needs.txt")]"
fi
ax press Cancel >/dev/null

# S3: a committed .yard locks the PHP picker, the HTTPS switch, Detect docroot and Save settings
tree_c="$(ax tree)"
locked="$(jq -c '[.[] | select(.name == "PHP for chk-c" or .name == "HTTPS for chk-c" or .name == "Detect docroot" or .name == "Save settings to .yard") | {name, enabled, help}]' <<<"$tree_c")"
if jq -e 'length == 4 and all(.[]; .enabled == false) and any(.[]; .name == "PHP for chk-c" and (.help // "" | test("set in .*/chk-c/\\.yard")))' <<<"$locked" >/dev/null; then
  pass "S3 a committed .yard locks the PHP picker (it says $(jq -r '.[] | select(.name == "PHP for chk-c") | .help' <<<"$locked")), HTTPS, Detect docroot and Save settings"
else
  fail "S3 committed .yard" "$locked"
fi

# S5: HTTPS switch, Detect docroot, Node
ax row chk-a >/dev/null
ax press "HTTPS for chk-a" >/dev/null
if within 5 sh -c "'$yard_bin' --root '$root' sites --json | jq -e '.[] | select(.name == \"chk-a\") | .secure'"; then
  ax press "HTTPS for chk-a" >/dev/null
  if within 5 sh -c "'$yard_bin' --root '$root' sites --json | jq -e '.[] | select(.name == \"chk-a\") | .secure | not'"; then
    pass "S5 the HTTPS switch secures the site and unsecures it"
  else fail "S5 HTTPS off"; fi
else
  fail "S5 the HTTPS switch did not secure the site"
fi
mkdir -p "$root/projects/chk-a/public"
echo "<?php echo 'public';" >"$root/projects/chk-a/public/index.php"
ax press "Detect docroot" >/dev/null
if within 5 sh -c "'$yard_bin' --root '$root' sites --json | jq -e '.[] | select(.name == \"chk-a\") | .docroot | endswith(\"/public\")'"; then
  pass "S5 Detect docroot finds public/"
else
  fail "S5 Detect docroot" "$(y sites --json | jq -c '.[] | select(.name == "chk-a")')"
fi
y node use 22 --site chk-b >/dev/null 2>&1
ax row chk-b >/dev/null
if within 5 texts_have "Node 22 — not installed, from sites.toml"; then
  ax press "Use default Node" >/dev/null
  if within 5 sh -c "'$yard_bin' --root '$root' sites --json | jq -e '.[] | select(.name == \"chk-b\") | .node == null'"; then
    pass "S5 the detail says the site's Node and where it is set, and Use default Node unpins it"
  else fail "S5 Use default Node" "$(y sites --json | jq -c '.[] | select(.name == "chk-b") | .node')"; fi
else
  fail "S5 no Node line for a pinned site" "$(texts | grep -i node)"
fi

# S6: Edit nginx override opens the site's override as text, in TextEdit when no editor is set
# (accessibility only: Apple Events to TextEdit would ask for Automation permission)
textedit_was_running=false
pgrep -x TextEdit >/dev/null && textedit_was_running=true
ax row chk-a >/dev/null
ax press "Edit nginx override" >/dev/null
if within 10 sh -c "osascript -l JavaScript '$ax_js' windows-of TextEdit | jq -e 'index(\"chk-a.conf\")'"; then
  pass "S6 Edit nginx override opens chk-a.conf in TextEdit"
else
  fail "S6 TextEdit shows no chk-a.conf" "$(ax windows-of TextEdit)"
fi
ax close-window TextEdit chk-a.conf >/dev/null
$textedit_was_running || pkill -x TextEdit || true

# S7: Copy .env copies what yard env prints
clip_backup="$out/clipboard.bak"
pbpaste >"$clip_backup"
ax row chk-c >/dev/null
ax press "Copy .env" >/dev/null
sleep 1
copied="$(pbpaste)"
pbcopy <"$clip_backup"
if [[ "$copied" == "$(y env --site chk-c)" ]]; then
  pass "S7 Copy .env copies what yard env --site prints"
else
  fail "S7 Copy .env" "app [$copied], cli [$(y env --site chk-c)]"
fi

# --- Services, PHP, Ports ---------------------------------------------------------------------

ax tab Services >/dev/null
sleep 0.5
shot services
want="$(y services --json | jq -c '[.[].name] | sort')"
got="$(rows)"
if [[ "$got" == "$want" ]]; then pass "V the Services table lists every instance"; else fail "V Services table" "app $got, cli $want"; fi

svc() { y services --json | jq -c --arg n "$1" '.[] | select(.name == $n)'; }
alert_texts() { ax tree | jq -r '.[] | select(.subrole == "AXApplicationAlert") | .name'; }
ax tab Services >/dev/null

# V2: a line brew does not have turns the line field into a picker of the lines it has
ax press "Add service…" >/dev/null
ax pick Engine PostgreSQL >/dev/null
ax type "Line (optional), such as 8.4" 99 >/dev/null
ax press Add >/dev/null
if within 15 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .role == \"AXPopUpButton\" and .name == \"Line\")'"; then
  pass "V2 a line brew does not have turns into a picker of the lines it has"
else
  fail "V2 no line picker" "$(alert_texts)"
fi
ax press Cancel >/dev/null

# V1: add MinIO from the form; it runs as a job whose output Activity shows
if [[ -d "$(brew --prefix)/opt/minio" ]]; then
  ax press "Add service…" >/dev/null
  ax pick Engine MinIO >/dev/null
  ax type "Port (optional)" 28900 >/dev/null
  ax press Add >/dev/null
  if within 90 sh -c "'$yard_bin' --root '$root' services --json | jq -e 'any(.[]; .name == \"minio\" and .state == \"running\")'"; then
    ax tab Activity >/dev/null
    sleep 0.5
    shot activity-minio
    if texts | grep -qi "minio"; then pass "V1 Add service runs as a job, and Activity lists it: $(texts | grep -i -m1 minio)"; else fail "V1 Activity shows no minio job"; fi
  else
    fail "V1 minio was not added" "$(svc minio)"
  fi
  ax tab Services >/dev/null
else
  note "V1 skipped: no brew minio keg"
fi

# V3: Port… moves an instance
ax press "Port…" redis >/dev/null
ax type "Port for redis" 28380 >/dev/null
ax press Move >/dev/null
if within 15 sh -c "'$yard_bin' --root '$root' services --json | jq -e '.[] | select(.name == \"redis\") | .ports[0].port == 28380'"; then
  pass "V3 Port… moves redis to 28380"
else
  fail "V3 port" "$(svc redis)"
fi

# V4: Sleep after…
ax press "Sleep after…" redis >/dev/null
ax type "Sleep after for redis" 5m >/dev/null
ax press Set >/dev/null
if within 10 sh -c "'$yard_bin' --root '$root' services --json | jq -e '.[] | select(.name == \"redis\") | .sleep_after_secs == 300'"; then
  pass "V4 Sleep after… sets 5m"
else
  fail "V4 sleep after" "$(svc redis | jq -c '{sleep_after_secs, sleep_after_source}')"
fi

# V5: Logs shows the tail, then new lines as they come
ax press Logs redis >/dev/null
log_lines() { ax tree | jq '[.[] | select(.role == "AXStaticText" and .row == null and ((.value // .name // "") | test("Ready to accept|Redis version|Server initialized|signal")))] | length'; }
if within 10 sh -c "[ \"\$(osascript -l JavaScript '$ax_js' tree | jq '[.[] | select((.value // .name // \"\") | test(\"Ready to accept|Redis version|Server initialized\"))] | length')\" -gt 0 ]"; then
  before="$(log_lines)"
  y service restart redis >/dev/null
  if within 15 sh -c "[ \"\$(osascript -l JavaScript '$ax_js' tree | jq '[.[] | select((.value // .name // \"\") | test(\"Ready to accept|Redis version|Server initialized|signal\"))] | length')\" -gt $before ]"; then
    pass "V5 Logs shows redis's log, and new lines arrive live (a restart)"
  else
    fail "V5 no live lines after a restart" "before $before, now $(log_lines)"
  fi
else
  fail "V5 Logs showed no redis log lines"
fi
ax press "Close log" >/dev/null 2>&1 || true

# V6: Remove asks first; an instance a .yard lists is refused before asking
ax press "Remove…" redis >/dev/null
sleep 0.5
if alert_texts | grep -q "redis is used by chk-c"; then
  pass "V6 Remove… is refused for redis, which chk-c's .yard lists: $(alert_texts | grep -m1 'used by')"
else
  fail "V6 remove of a used instance" "$(alert_texts) $(ax dialog)"
  ax answer Cancel >/dev/null 2>&1 || true
fi
if svc minio | grep -q minio; then
  ax press "Remove…" minio >/dev/null
  sleep 0.5
  d="$(ax dialog)"
  if jq -e '.buttons | index("Delete") and index("Remove, keep its data") and index("Cancel")' <<<"$d" >/dev/null 2>&1; then
    ax answer Delete >/dev/null
    if within 30 sh -c "! '$yard_bin' --root '$root' services --json | jq -e 'any(.[]; .name == \"minio\")'"; then
      pass "V6 Remove… asks ($(jq -r '.text | join(" ")' <<<"$d")) and removes minio"
    else fail "V6 minio still there after Remove"; fi
  else
    fail "V6 Remove… asked nothing" "$d"
  fi
fi

# V7 and M2: a service failed on a held port names the holder, and the menu offers its fix
port="$(svc redis | jq -r '.ports[0].port')"
y service stop redis >/dev/null
python3 -c "import socket,time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('127.0.0.1', $port)); s.listen(1); time.sleep(120)" &
holder_pid=$!
sleep 0.5
y service start redis >/dev/null 2>&1 || true
if within 15 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .subrole == \"AXApplicationAlert\" and (.name | test(\"redis failed to start: port [0-9]+ is held by Python\")))'"; then
  pass "V7 redis failed on a held port, and the Services screen names the holder: $(alert_texts | grep -m1 'held by')"
else
  fail "V7 holder not named" "$(svc redis | jq -c .state) $(texts | grep -i -m2 'redis\|held')"
fi
problem="$(ax menu | jq -r '.[] | select((.name // "") | startswith("⚠︎")) | .name' | grep -i -m1 redis || true)"
if [[ -n "$problem" ]]; then
  ax menu-click "$problem" >/dev/null
  sleep 1
  if ax tree | jq -e 'any(.[]; .name == "Port for redis")' >/dev/null; then
    pass "M2 the menu lists \"$problem\", and choosing it opens the port sheet"
  else
    fail "M2 the problem item did nothing visible" "$problem"
  fi
  ax press Cancel >/dev/null 2>&1 || true
else
  fail "M2 no problem item for redis" "$(ax menu | jq -c '[.[].name]')"
fi
kill "$holder_pid" 2>/dev/null || true
wait "$holder_pid" 2>/dev/null || true
y service start redis >/dev/null 2>&1 || true

ax tab PHP >/dev/null
sleep 0.5
shot php
want="$(y php list --json | jq -c '[.[].version] | sort')"
got="$(rows | jq -c '[.[] | select(test("^[0-9]+\\.[0-9]+$"))]')"
if [[ "$got" == "$want" ]]; then pass "P1 PHP lists every supported version"; else fail "P1 PHP table" "app $got, cli $want"; fi

# --- PHP: P3, P4, P5, P6 ---
ext_enabled() { y ext list --php 8.4 --json | jq -r --arg n "$1" '.[] | select(.name == $n) | .enabled'; }
ax pick "PHP version" 8.4 >/dev/null 2>&1 || true
ax press "opcache on PHP 8.4" >/dev/null
if within 15 sh -c "[ \"\$('$yard_bin' --root '$root' ext list --php 8.4 --json | jq -r '.[] | select(.name == \"opcache\") | .enabled')\" = false ]"; then
  ax press "opcache on PHP 8.4" >/dev/null
  if within 15 sh -c "[ \"\$('$yard_bin' --root '$root' ext list --php 8.4 --json | jq -r '.[] | select(.name == \"opcache\") | .enabled')\" = true ]"; then
    pass "P3 an extension's checkbox disables it and enables it again (add and remove install with brew or pecl: by hand)"
  else fail "P3 enable" "$(ext_enabled opcache)"; fi
else
  fail "P3 disable" "$(ext_enabled opcache)"
fi
ax type "php.ini key" memory_limit >/dev/null
ax type Value 321M >/dev/null
ax press Set >/dev/null
if within 10 sh -c "'$yard_bin' --root '$root' php config get memory_limit --php 8.4 2>/dev/null | grep -q 321M"; then
  ax press Unset >/dev/null
  if within 10 sh -c "! '$yard_bin' --root '$root' php config get memory_limit --php 8.4 2>/dev/null | grep -q 321M"; then
    pass "P5 Set writes memory_limit = 321M to 8.4's overrides, and Unset removes it"
  else fail "P5 unset" "$(y php config get memory_limit --php 8.4 2>&1)"; fi
else
  fail "P5 set" "$(y php config get memory_limit --php 8.4 2>&1)"
fi
textedit_was_running=false
pgrep -x TextEdit >/dev/null && textedit_was_running=true
ax press "Open 99-overrides.ini" >/dev/null
if within 10 sh -c "osascript -l JavaScript '$ax_js' windows-of TextEdit | jq -e 'index(\"99-overrides.ini\")'"; then
  pass "P4 Open 99-overrides.ini opens it in TextEdit"
else
  fail "P4 TextEdit shows no 99-overrides.ini" "$(ax windows-of TextEdit) $(ax tree | jq -r '.[] | select(.subrole == "AXApplicationAlert") | .name')"
fi
ax close-window TextEdit 99-overrides.ini >/dev/null
$textedit_was_running || pkill -x TextEdit || true
default_php="$(y php list --json | jq -r '.[] | select(.default) | .version')"
ax press "Uninstall…" "$default_php" >/dev/null
sleep 0.5
if ax dialog | jq -e '.buttons | index("Uninstall")' >/dev/null 2>&1; then ax answer Uninstall >/dev/null; fi
if within 10 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .subrole == \"AXApplicationAlert\" and (.name | test(\"is the default version\")))'"; then
  pass "P6 uninstalling the default PHP ($default_php) is refused: $(ax tree | jq -r '[.[] | select(.subrole == "AXApplicationAlert") | .name] | map(select(test("default"))) | first')"
else
  fail "P6 no refusal for the default PHP"
fi
[[ -d "$(brew --prefix)/opt/php@$default_php" || -d "$(brew --prefix)/opt/php" ]] || die "PHP $default_php is gone: P6 must never uninstall it"

# --- Node: N1, N3 ---
ax tab Node >/dev/null
want="$(y node list --json | jq -c '[.versions[].version] | sort')"
got="$(rows | jq -c '[.[] | select(test("^[0-9]"))]')"
if [[ "$got" == "$want" ]]; then pass "N1 Node lists the installed versions ($want)"; else fail "N1 Node list" "app $got, cli $want"; fi
ax press "Show available" >/dev/null
available() { ax tree | jq -r '.[] | select(.role == "AXStaticText" and (.value // "" | test("^[0-9]+\\.[0-9]+\\.[0-9]+ .*, [0-9]+\\."))) | .value' | head -1; }
if within 30 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .role == \"AXStaticText\" and (.value // \"\" | test(\"^[0-9]+\\\\.[0-9]+\\\\.[0-9]+ .*, [0-9]+\\\\.\")))'"; then
  pass "N3 Show available lists what nodejs.org has: $(available | cut -c1-60)…"
else
  fail "N3 Show available listed nothing" "$(ax tree | jq -r '.[] | select(.subrole == "AXApplicationAlert") | .name')"
fi

# --- Tunnels: T3 ---
ax tab Tunnels >/dev/null
want="$(y tunnels domains --json | jq -c '[.[].domain] | sort')"
got="$(ax tree | jq -c '[.[] | select(.role == "AXRow" and (.name // "" | test("\\."))) | .name] | sort')"
if [[ "$got" == "$want" ]]; then pass "T3 Tunnels lists the domains yard tunnels domains has ($want)"; else fail "T3 domains" "app $got, cli $want"; fi


ax tab Ports >/dev/null
sleep 0.5
shot ports
want="$(y ports --json | jq -c '[.[].unit] | sort')"
got="$(rows)"
if [[ "$got" == "$want" ]]; then pass "R1 the Ports table lists every unit"; else fail "R1 Ports table" "app $got, cli $want"; fi

# --- Ports: R2 ---
ax press "Set…" redis >/dev/null
ax type "Port for redis" 28381 >/dev/null
ax press Move >/dev/null
if within 15 sh -c "'$yard_bin' --root '$root' ports --json | jq -e '.[] | select(.unit == \"redis\") | .port == 28381'"; then
  pass "R2 Set… moves redis to 28381"
else
  fail "R2 set" "$(y ports --json | jq -c '.[] | select(.unit == "redis")')"
fi
mode() { y ports --json | jq -r '.[] | select(.unit == "redis") | .mode'; }
before="$(mode)"
ax press "$([[ "$before" == pinned ]] && echo Unpin || echo Pin)" redis >/dev/null
if within 10 sh -c "[ \"\$('$yard_bin' --root '$root' ports --json | jq -r '.[] | select(.unit == \"redis\") | .mode')\" != $before ]"; then
  after="$(mode)"
  ax press "$([[ "$after" == pinned ]] && echo Unpin || echo Pin)" redis >/dev/null
  if within 10 sh -c "[ \"\$('$yard_bin' --root '$root' ports --json | jq -r '.[] | select(.unit == \"redis\") | .mode')\" = $before ]"; then
    pass "R2 Unpin and Pin switch redis $before → $after → $before"
  else fail "R2 pin back" "$(mode)"; fi
else
  fail "R2 pin" "$(mode)"
fi

# --- Logs: L1, L2 ---
# nginx logs no requests (access_log off) and dnsmasq is quiet, so redis's restart makes the lines.
log_count() { ax tree | jq --arg m "$1" '[.[] | select(.role == "AXStaticText" and (.value // "" | contains($m)))] | length'; }
ax tab Logs >/dev/null
ax pick Unit "All units" >/dev/null
before="$(log_count "Ready to accept")"
y service restart redis >/dev/null 2>&1
if within 10 sh -c "[ \"\$(osascript -l JavaScript '$ax_js' tree | jq '[.[] | select(.role == \"AXStaticText\" and (.value // \"\" | contains(\"Ready to accept\")))] | length')\" -gt $before ]"; then
  pass "L1 Logs shows every unit's lines as they come (redis restarting)"
  ax type "Filter the lines" "Ready to accept" >/dev/null
  sleep 0.5
  shown="$(ax tree | jq '[.[] | select(.role == "AXStaticText" and (.value // "" | test("^[a-z0-9.-]+  ")))] | length')"
  hits="$(log_count "Ready to accept")"
  if [[ "$shown" == "$hits" && "$hits" -ge 1 ]]; then pass "L1 the filter keeps only matching lines ($hits)"; else fail "L1 filter" "shown $shown, matching $hits"; fi
  ax press Pause >/dev/null
  paused_at="$(log_count "Ready to accept")"
  y service restart redis >/dev/null 2>&1
  sleep 3
  if [[ "$(log_count "Ready to accept")" == "$paused_at" ]]; then
    ax press Pause >/dev/null
    if within 5 sh -c "[ \"\$(osascript -l JavaScript '$ax_js' tree | jq '[.[] | select(.role == \"AXStaticText\" and (.value // \"\" | contains(\"Ready to accept\")))] | length')\" -gt $paused_at ]"; then
      pass "L1 Pause holds the lines, and resuming shows what came meanwhile"
    else fail "L1 resume showed nothing new"; fi
  else
    fail "L1 Pause did not hold the lines"
    ax press Pause >/dev/null
  fi
  ax type "Filter the lines" "" >/dev/null
else
  fail "L1 no live lines while redis restarted"
fi
finder_before="$(ax windows-of Finder | jq -c .)"
ax press "Show log file" >/dev/null
new_finder() { ax windows-of Finder | jq -r --argjson b "$finder_before" '.[] | select(. as $w | $b | index($w) | not)' | head -1; }
if within 10 sh -c "osascript -l JavaScript '$ax_js' windows-of Finder | jq -e --argjson b '$finder_before' 'any(.[]; . as \$w | \$b | index(\$w) | not)'"; then
  shown_in="$(new_finder)"
  pass "L2 Show log file reveals it in Finder (a window on $shown_in)"
  ax close-window Finder "$shown_in" >/dev/null
else
  fail "L2 no new Finder window" "$(ax windows-of Finder)"
fi


# --- Doctor -----------------------------------------------------------------------------------

# A managed .yard that chk-c's committed one hides: doctor's rm hint, beside the resolver's sudo one.
mkdir -p "$root/sites/chk-c"
printf 'project = "%s"\nphp = "8.4"\n' "$(cd "$root/projects/chk-c" && pwd -P)" >"$root/sites/chk-c/.yard"
ax tab Doctor >/dev/null
ax press "Run doctor" >/dev/null
sleep 4
shot doctor
readable "D1 doctor ran and its results are readable text" "yardd is running"
doc="$(ax tree)"
resolver_hint="$(jq -r '.[] | select(.role == "AXStaticText" and (.value // "" | startswith("sudo "))) | .value' <<<"$doc" | head -1)"
rm_hint="$(jq -r '.[] | select(.role == "AXStaticText" and (.value // "" | startswith("rm "))) | .value' <<<"$doc" | head -1)"
terminal="$(jq '[.[] | select(.role == "AXButton" and .name == "Open in Terminal")] | length' <<<"$doc")"
copies="$(jq '[.[] | select(.role == "AXButton" and .name == "Copy")] | length' <<<"$doc")"
fixes="$(jq '[.[] | select(.role == "AXButton" and .name == "Fix")] | length' <<<"$doc")"
if [[ -n "$resolver_hint" && -n "$rm_hint" && "$terminal" == 1 ]]; then
  pass "D2 a sudo hint offers Open in Terminal and Copy ($resolver_hint), an rm hint only Copy ($rm_hint): $terminal Terminal, $copies Copy, $fixes Fix"
else
  fail "D2 hint buttons" "sudo [$resolver_hint] rm [$rm_hint] terminal $terminal copy $copies fix $fixes"
fi
rm -rf "$root/sites/chk-c"


# --- A3: every control has a label, on every screen -------------------------------------------

unlabelled=""
for screen in Overview Sites Services PHP Node Tunnels Ports Logs Doctor Sources Activity; do
  ax tab "$screen" >/dev/null
  sleep 0.6
  [[ "$screen" == Node || "$screen" == Tunnels || "$screen" == Logs || "$screen" == Sources || "$screen" == Activity ]] && shot "$(tr '[:upper:]' '[:lower:]' <<<"$screen")"
  bad="$(ax tree | jq -r '.[] | select(.role == "AXButton" or .role == "AXCheckBox" or .role == "AXPopUpButton" or .role == "AXRadioButton" or .role == "AXTextField")
    | select((.desc // "") | test("close button|minimize button|full screen button|zoom button") | not)
    | select((.name // "") == "") | "\(.role) in row \(.row // "-")"')"
  [[ -n "$bad" ]] && unlabelled="$unlabelled $screen: $(tr '\n' ',' <<<"$bad")"
done
if [[ -z "$unlabelled" ]]; then pass "A3 every button, switch, picker and field has a label on all 11 screens"; else fail "A3 unlabelled controls" "$unlabelled"; fi

# --- M1: yardd stops and starts ---------------------------------------------------------------

ok_header="$(menu_header)"
icon="$(ax icon)"
if [[ "$icon" == ok ]]; then pass "M1 the menu bar icon is the filled circle while all is well"; else fail "M1 icon with yardd up" "$icon"; fi
watch_start 10 menu-header not "$ok_header"
stopped_at="$(now)"
stop_yardd
watch_end
if [[ -n "$at" ]] && perl -e "exit(!($at - $stopped_at <= 1))"; then
  pass "M1 the menu says \"$(jq -r .value "$out/watch.json")\" $(since "$at" "$stopped_at") s after yardd is told to stop"
else
  fail "M1 menu after yardd stopped" "$(cat "$out/watch.json")"
fi
# The icon is read from a screenshot; the status item stays highlighted while the watcher's menu
# closes, so its shape is read again for up to 3 s.
within 3 sh -c "[ \"\$(osascript -l JavaScript '$ax_js' icon)\" = offline ]" || true
icon="$(ax icon)"
if [[ "$icon" == offline ]]; then pass "M1 and the icon is the ring"; else fail "M1 icon with yardd down" "$icon"; fi
watch_start 15 menu-header is "$ok_header"
started_at="$(now)"
start_yardd
watch_end
if [[ -n "$at" ]] && perl -e "exit(!($at - $started_at <= 6))"; then
  pass "M1 and is back $(since "$at" "$started_at") s after yardd starts"
else
  fail "M1 menu after yardd started" "$(cat "$out/watch.json")"
fi
icon="$(ax icon)"
if [[ "$icon" == ok ]]; then pass "M1 and the icon is filled again"; else fail "M1 icon after yardd started" "$icon"; fi

# --- M6: an open menu changes in place --------------------------------------------------------

{
  rm -f "$out/ready"
  osascript -l JavaScript "$ax_js" menu-log "$out/ready" 8 >"$out/menu-log.json" &
  log_pid=$!
  within 20 test -e "$out/ready" || die "menu-log never opened the menu"
  sleep 1
  y service stop redis >/dev/null
  sleep 2
  y service start redis >/dev/null
  wait "$log_pid" || true
  seen="$(jq -c '[.[] | (.items // ["closed"])[] | select(. != null and test("^redis|closed"))] | unique' "$out/menu-log.json")"
  closed="$(jq '[.[] | select(.items == null)] | length' "$out/menu-log.json")"
  if [[ "$closed" == 0 ]] && jq -e 'index("redis — stopped") and index("redis — running")' <<<"$seen" >/dev/null; then
    pass "M6 the open menu shows redis stop and start without closing"
  else
    fail "M6 open menu" "seen $seen, closed $closed times"
  fi
}

# --- O4: a yardd older than the app ----------------------------------------------------------

if [[ -n "${YARD_CHECK_OLD_YARDD:-}" ]]; then
  [[ -x "$YARD_CHECK_OLD_YARDD" ]] || die "YARD_CHECK_OLD_YARDD is not an executable: $YARD_CHECK_OLD_YARDD"
  # A yardd from before 2026-11-06 has no helper guard: with the real helper socket it repoints this
  # Mac's resolver at the scratch dnsmasq (it did, on the first O4 run). It must be built with
  # yard-core's HELPER_SOCKET pointing at a path that does not exist.
  if strings "$YARD_CHECK_OLD_YARDD" | grep -q "/var/run/dev.more.yard.helper.sock"; then
    die "YARD_CHECK_OLD_YARDD can reach the root helper: build it with HELPER_SOCKET pointing nowhere"
  fi
  stop_yardd
  "$YARD_CHECK_OLD_YARDD" --root "$root" >>"$out/yardd-old.log" 2>&1 &
  yardd_pid=$!
  within 15 y status || die "the old yardd did not start (see $out/yardd-old.log)"
  ax tab Sites >/dev/null
  if within 15 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .subrole == \"AXApplicationAlert\" and (.name | test(\"older than this app\")))'"; then
    if [[ "$(rows)" == "$(y sites --json | jq -c '[.[].name] | sort')" ]]; then
      pass "O4 a yardd from before 2.11 ($("$YARD_CHECK_OLD_YARDD" --version)) shows the stale banner, and the sites are still listed"
    else
      fail "O4 the sites are not listed with an old yardd" "app $(rows)"
    fi
  else
    fail "O4 no stale banner with an old yardd" "$(ax tree | jq -c '[.[] | select(.subrole == "AXApplicationAlert") | .name]')"
  fi
  stop_yardd
  [[ "$(resolver_sum)" == "$resolver_before" ]] || fail "O4 the old yardd changed /etc/resolver" "$(resolver_sum)"
  start_yardd
  within 15 sh -c "! osascript -l JavaScript '$ax_js' tree | jq -e 'any(.[]; .subrole == \"AXApplicationAlert\" and (.name | test(\"older than this app\")))'" ||
    fail "O4 the stale banner stayed after yardd was current again"
else
  note "O4 skipped: set YARD_CHECK_OLD_YARDD to a yardd built from 2c1ed9b"
fi

# --- M6: quitting the app leaves yardd answering ----------------------------------------------

ax menu-click "Quit Yard.app" >/dev/null
if within 5 sh -c "! kill -0 $app_pid 2>/dev/null"; then
  app_pid=""
  if y status >/dev/null; then pass "M6 Quit Yard.app quits it and yard status still answers"; else fail "M6 yardd stopped with the app"; fi
else
  fail "M6 Quit Yard.app did not quit it"
fi

# --- B1, B2: onboarding for a root with no yardd and no LaunchAgent of its own ----------------

mkdir -p "$onb_root"
open -n "$bundle" --args --root "$onb_root"
within 10 pgrep -f "Yard.app/Contents/MacOS/Yard --root $onb_root" || die "Yard.app did not start on $onb_root"
app_pid="$(pgrep -f "Yard.app/Contents/MacOS/Yard --root $onb_root" | head -1)"
if within 20 sh -c "osascript -l JavaScript '$ax_js' tree other | jq -e 'any(.[]; .value == \"Set up Yard\")'"; then
  shot onboarding
  steps="$(ax tree other | jq -r '[.[] | select(.role == "AXStaticText") | .value] | map(select(. != null)) | join("|")')"
  if [[ "$steps" == *"Homebrew|done"* && "$steps" == *"yardd|to do"* && "$steps" == *"Root helper"*"|to do"* ]]; then
    pass "B1 with no yardd and no LaunchAgent for the root, onboarding opens: brew done, yardd and the rest to do"
  else
    fail "B1 onboarding's steps" "$steps"
  fi
  ax press "Start yardd" @other >/dev/null
  if within 10 sh -c "osascript -l JavaScript '$ax_js' tree other | jq -e 'any(.[]; .subrole == \"AXApplicationAlert\" and (.name | test(\"LaunchAgent .*serves\")))'"; then
    pass "B2 Start yardd for a scratch root is refused in onboarding, naming the root the LaunchAgent serves"
  else
    fail "B2 Start yardd" "$(ax tree other | jq -c '[.[] | select(.subrole == "AXApplicationAlert") | .name]')"
  fi
else
  fail "B1 onboarding did not open for a root with no yardd"
fi
kill -TERM "$app_pid" 2>/dev/null || true
within 5 sh -c "! kill -0 $app_pid 2>/dev/null" || true
app_pid=""

if [[ "$(agent_sum)" == "$agent_before" ]]; then
  pass "yardd's LaunchAgent is untouched"
else
  fail "yardd's LaunchAgent changed during the run" "$(agent_sum)"
fi
if [[ "$(resolver_sum)" == "$resolver_before" ]]; then
  pass "the scratch yardd left /etc/resolver alone"
else
  fail "/etc/resolver changed during the run" "$(resolver_sum)"
fi

echo
echo "$passed passed, $failed failed; screenshots and report in $out" | tee -a "$report"
((failed == 0))
