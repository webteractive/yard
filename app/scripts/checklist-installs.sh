#!/usr/bin/env bash
# The M6 checklist items (plan Task 17) that install something, share a site or need a password,
# run against a scratch yardd whose Homebrew is the test suite's fake (yard-fake-unit as brew,
# nginx, dnsmasq, php-fpm, fnm and cloudflared): nothing is installed on this Mac, no tunnel is
# public and no password is typed. checklist.sh covers the rest against the real kegs.
#
#   app/scripts/checklist-installs.sh [--keep]
#
# Checks: B3 (the helper step opens Terminal with the quoted command; the sudo it starts is
# stopped before it gets a password), P2 (an install's progress, a second one waiting behind it and
# cancelled), P3's add and remove, M5 (the Xdebug submenu follows xdebug), the trust question (a
# formula from an untrusted tap asks, and Trust installs it), N2 (a Node install as a job), T1 and T2
# (a quick tunnel's URL, then Stop sharing).
#
# Needs: target/release/{yard,yardd,yard-fake-unit} (cargo build --release), app/target/Yard.app
# (scripts/bundle.sh), jq, and Accessibility and Screen Recording for the terminal. The scratch root
# is $YARD_CHECK_ROOT (default /tmp/yard-check-installs), its fake brew beside it (`-brew`); nginx
# and dnsmasq are pinned to 28080, 28443 and 28053 and MinIO to 28900. It refuses ~/.yard, a running
# Yard.app or a used port, fails if /etc/resolver or yardd's LaunchAgent changed, puts Yard.app's
# settings back, and removes what it made unless --keep. About 3 minutes.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$app_dir/.." && pwd)"
root="${YARD_CHECK_ROOT:-/tmp/yard-check-installs}"
brew_dir="$root-brew"
out="${TMPDIR:-/tmp}/yard-check-installs-$(date +%Y%m%d-%H%M%S)"
keep=false
[[ "${1:-}" == "--keep" ]] && keep=true

yard_bin="$repo/target/release/yard"
yardd_bin="$repo/target/release/yardd"
fake_bin="$repo/target/release/yard-fake-unit"
bundle="$app_dir/target/Yard.app"
ax_js="$app_dir/scripts/ax.js"
app_domain=dev.more.yard.app

y() { "$yard_bin" --root "$root" "$@"; }
ax() { osascript -l JavaScript "$ax_js" "$@"; }

passed=0
failed=0
report="$out/report.txt"
pass() { passed=$((passed + 1)); echo "PASS  $1" | tee -a "$report"; }
fail() { failed=$((failed + 1)); echo "FAIL  $1${2:+: $2}" | tee -a "$report"; }
die() { echo "checklist-installs.sh: $1" >&2; exit 1; }

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

texts() { ax tree | jq -r '.[] | select(.role == "AXStaticText") | [.name, .value, .desc] | map(select(. != null and . != "")) | first // empty'; }
alerts() { ax tree | jq -c '[.[] | select(.subrole == "AXApplicationAlert") | .name]'; }

# --- preflight --------------------------------------------------------------------------------

[[ "$root" == "$HOME/.yard" ]] && die "refusing to use the real data root"
for f in "$yard_bin" "$yardd_bin" "$fake_bin"; do [[ -x "$f" ]] || die "$f is missing: cargo build --release"; done
[[ -d "$bundle" ]] || die "$bundle is missing: scripts/bundle.sh"
command -v jq >/dev/null || die "jq is missing"
osascript -l JavaScript -e 'ObjC.import("ApplicationServices"); $.AXIsProcessTrusted()' | grep -q true ||
  die "this terminal has no Accessibility permission (System Settings → Privacy & Security)"
pgrep -x Yard >/dev/null && die "Yard.app is already running; quit it first (the script finds the app by name)"
for dir in "$root" "$brew_dir"; do
  if [[ -e "$dir" ]]; then
    [[ -e "$dir/.yard-check" ]] || die "$dir exists and was not made by this script"
    pgrep -f "yardd --root $root" >/dev/null && die "a yardd is still running on $root"
    rm -rf "$dir"
  fi
done
for port in 28080 28443 28053 28900; do
  lsof -nP -iTCP:"$port" -sTCP:LISTEN -iUDP:"$port" >/dev/null 2>&1 && die "port $port is in use"
done

resolver_sum() { shasum /etc/resolver/* 2>/dev/null | sort; }
resolver_before="$(resolver_sum)"
agent_plist="$HOME/Library/LaunchAgents/dev.more.yard.daemon.plist"
agent_sum() { shasum "$agent_plist" 2>/dev/null; }
agent_before="$(agent_sum)"
terminal_was_running=false
pgrep -x Terminal >/dev/null && terminal_was_running=true

mkdir -p "$root" "$brew_dir" "$out"
touch "$root/.yard-check" "$brew_dir/.yard-check"
defaults export "$app_domain" "$out/defaults.plist" 2>/dev/null || true
cat >"$root/ports.toml" <<'EOF'
range = "29000-29099"
[ports]
nginx-http = { port = 28080, mode = "pinned" }
nginx-https = { port = 28443, mode = "pinned" }
dnsmasq = { port = 28053, mode = "pinned" }
EOF

# The fake brew, as crates/yard-daemon/tests/support builds it: every binary is yard-fake-unit,
# which acts by the name it is run as. PHP 8.4 is installed; the shivammathur taps are trusted and
# minio/stable is not; xdebug is an extension formula; each install takes 2 s (20 s for P2).
place() { mkdir -p "$(dirname "$brew_dir/$1")" && ln -sf "$fake_bin" "$brew_dir/$1"; }
for bin in bin/brew bin/fnm opt/nginx/bin/nginx opt/dnsmasq/sbin/dnsmasq opt/cloudflared/bin/cloudflared \
  opt/php@8.4/sbin/php-fpm opt/php@8.4/bin/php opt/php@8.4/bin/pecl opt/php@8.4/bin/phpize opt/php@8.4/bin/php-config; do
  place "$bin"
done
mkdir -p "$brew_dir/etc/nginx" "$brew_dir/etc/php/8.4/conf.d" "$brew_dir/Library/Taps/minio/homebrew-stable"
: >"$brew_dir/etc/nginx/mime.types"
: >"$brew_dir/etc/nginx/fastcgi_params"
echo '{"taps":["shivammathur/php","shivammathur/extensions"],"formulae":[],"casks":[],"commands":[]}' >"$brew_dir/fake-trust.json"
echo xdebug >"$brew_dir/fake-ext-formulas"
echo 2000 >"$brew_dir/fake-brew-delay"
echo '[{"name":"minio","full_name":"minio/stable/minio","tap":"minio/stable","aliases":[],"versions":{"stable":"2025.9.7"},"deprecated":false,"disabled":false,"deprecation_date":null,"disable_date":null}]' >"$brew_dir/fake-formulae.json"

yardd_pid=""
app_pid=""
start_yardd() {
  YARD_BREW_PREFIX="$brew_dir" "$yardd_bin" --root "$root" >>"$out/yardd.log" 2>&1 &
  yardd_pid=$!
  within 15 y status || die "the scratch yardd did not start (see $out/yardd.log)"
}
stop_app() {
  [[ -n "$app_pid" ]] || return 0
  kill -TERM "$app_pid" 2>/dev/null || true
  within 5 sh -c "! kill -0 $app_pid" || true
  app_pid=""
}
launch_app() {
  open -n "$bundle" --env "YARD_BREW_PREFIX=$brew_dir" --args --root "$root"
  within 10 pgrep -f "Yard.app/Contents/MacOS/Yard --root $root" || die "Yard.app did not start"
  app_pid="$(pgrep -f "Yard.app/Contents/MacOS/Yard --root $root" | head -1)"
}
# The sudo a B3 Terminal runs: never given a password, stopped as soon as it is seen.
helper_sudo() { pgrep -f "sudo .*--root $root helper install"; }
cleanup() {
  helper_sudo >/dev/null && pkill -INT -f "sudo .*--root $root helper install" || true
  $terminal_was_running || pkill -x Terminal || true
  if $keep; then
    echo "kept: root $root, brew $brew_dir, yardd pid ${yardd_pid:-none}, app pid ${app_pid:-none}"
    echo "Yard.app's settings are the run's: put them back with defaults import $app_domain $out/defaults.plist"
    return
  fi
  stop_app
  if [[ -n "$yardd_pid" ]]; then
    kill -TERM "$yardd_pid" 2>/dev/null || true
    wait "$yardd_pid" 2>/dev/null || true
  fi
  # Yard.app's settings as they were, or none when it had none.
  if [[ -s "$out/defaults.plist" ]]; then
    defaults import "$app_domain" "$out/defaults.plist" 2>/dev/null || true
  else
    defaults delete "$app_domain" 2>/dev/null || true
  fi
  rm -rf "$root" "$brew_dir"
}
trap cleanup EXIT

# Settings unset, so Terminal.app is the terminal and no editor is set.
defaults delete "$app_domain" 2>/dev/null || true
defaults write "$app_domain" DidOnboard -bool true

# --- B3: with no yardd for the root, onboarding's helper step opens Terminal ------------------

launch_app
echo "Yard.app pid $app_pid against $root (fake brew $brew_dir); report in $out" | tee "$report"
within 30 sh -c "osascript -l JavaScript '$ax_js' tree other | jq -e 'any(.[]; .value == \"Set up Yard\")'" ||
  die "onboarding did not open for a root with no yardd"
tmp_dir="$(getconf DARWIN_USER_TEMP_DIR)"
ax press "Open in Terminal" @other >/dev/null
if within 15 helper_sudo; then
  script="$(find "$tmp_dir" -maxdepth 1 -name 'yard-*.command' -newer "$report" -exec cat {} + 2>/dev/null)"
  pkill -INT -f "sudo .*--root $root helper install" || true
  if [[ "$script" == *"'sudo' '$yard_bin' '--root' '$root' 'helper' 'install'"* ]]; then
    pass "B3 the helper step opens Terminal, which runs sudo yard --root $root helper install (stopped at its password)"
  else
    fail "B3 the command file" "$script"
  fi
else
  fail "B3 no sudo helper install started" "$(ax windows-of Terminal 2>&1) $(ax tree other | jq -c '[.[] | select(.subrole == "AXApplicationAlert") | .name]')"
fi
sleep 1
$terminal_was_running || pkill -x Terminal || true

# The menu's Set up Yard… reopens a closed checklist.
ax close-window Yard "Set up Yard" >/dev/null 2>&1 || osascript -l JavaScript -e 'Application("System Events").processes.byName("Yard").windows[0].buttons.whose({subrole: "AXCloseButton"})[0].click()' >/dev/null 2>&1 || true
sleep 0.5
ax menu-click "Set up Yard…" >/dev/null
if within 10 sh -c "osascript -l JavaScript '$ax_js' tree other | jq -e 'any(.[]; .value == \"Set up Yard\")'"; then
  pass "the menu's Set up Yard… opens the onboarding checklist"
else
  fail "Set up Yard… from the menu" "$(ax windows 2>&1)"
fi
stop_app

# --- the app against the scratch yardd on the fake brew ---------------------------------------

start_yardd
mkdir -p "$root/projects/chk-t"
echo "<?php echo 'chk-t';" >"$root/projects/chk-t/index.php"
(cd "$root/projects/chk-t" && y link chk-t >/dev/null)
launch_app
within 60 sh -c "osascript -l JavaScript '$ax_js' menu | jq -e 'any(.[]; .name == \"Sites\")'" ||
  die "the menu bar menu never listed the sites"
ax menu-click "Open Yard" >/dev/null
within 5 ax tree || die "Open Yard did not open the main window"

# The Overview's Set up Yard… opens the checklist while yardd answers too.
ax tab Overview >/dev/null
ax press "Set up Yard…" >/dev/null
if within 10 sh -c "osascript -l JavaScript '$ax_js' tree other | jq -e 'any(.[]; .value == \"Set up Yard\")'"; then
  pass "the Overview's Set up Yard… opens the onboarding checklist"
else
  fail "Set up Yard… from the Overview" "$(ax windows 2>&1)"
fi
ax menu-click "Open Yard" >/dev/null

# --- P2: an install shows its progress; a second waits behind it and is cancelled -------------

php_installed() { [[ "$(y php list --json | jq -r --arg v "$1" '.[] | select(.version == $v) | .installed')" == true ]]; }
job_phase() { texts | grep -qx -- "$1"; }
# Long enough installs that the second is still waiting when Activity is read.
echo 20000 >"$brew_dir/fake-brew-delay"
ax tab PHP >/dev/null
sleep 0.5
ax press Install 7.1 >/dev/null
ax press Install 7.2 >/dev/null
ax tab Activity >/dev/null
if within 10 job_phase "waiting for Install PHP 7.1"; then
  shot activity-waiting
  ax press "Cancel Install PHP 7.2" >/dev/null
  if within 10 job_phase cancelled && within 40 php_installed 7.1 && ! php_installed 7.2; then
    # brew's output is on screen (see the screenshots), but the accessibility tree gives every job
    # card the first card's log lines (an open finding), so it is not read from the tree here.
    pass "P2 a second install waits for the first ('waiting for Install PHP 7.1') and Cancel cancels it (7.1 installed, 7.2 not)"
  else
    fail "P2 cancel or install" "$(texts | tr '\n' '|')"
  fi
else
  fail "P2 no install waiting behind the first" "$(texts | tr '\n' '|') $(alerts)"
fi
shot activity-p2
echo 2000 >"$brew_dir/fake-brew-delay"

# --- P3 add and remove, M5: the Xdebug submenu follows xdebug ----------------------------------

has_ext() { y ext list --php 8.4 --json | jq -e --arg n "$1" 'any(.[]; .name == $n)'; }
xdebug_menu() { ax menu | jq -e 'any(.[]; .name == "Xdebug")'; }
no_ext() { ! has_ext "$1"; }
no_xdebug_menu() { ! xdebug_menu; }
ax tab PHP >/dev/null
ax pick "PHP version" 8.4 >/dev/null 2>&1 || true
ax type "Extension to add" xdebug >/dev/null
ax press Add >/dev/null
if within 30 has_ext xdebug; then
  pass "P3 Add installs xdebug for PHP 8.4 as a job"
  if within 10 xdebug_menu; then
    pass "M5 the menu has an Xdebug submenu once a PHP has xdebug: $(ax menu | jq -c '.[] | select(.name == "Xdebug") | [.children[].name]')"
  else
    fail "M5 no Xdebug submenu with xdebug installed" "$(ax menu | jq -c '[.[].name]')"
  fi
  ax tab PHP >/dev/null
  ax press Remove xdebug >/dev/null
  sleep 0.5
  if ax dialog | jq -e '.buttons | index("Remove")' >/dev/null 2>&1; then ax answer Remove >/dev/null; fi
  if within 30 no_ext xdebug; then
    pass "P3 Remove removes it"
    if within 10 no_xdebug_menu; then pass "M5 and the Xdebug submenu goes with it"; else fail "M5 the Xdebug submenu stayed"; fi
  else
    fail "P3 remove" "$(y ext list --php 8.4 --json | jq -c '[.[].name]') $(alerts)"
  fi
else
  fail "P3 add" "$(alerts)"
fi

# --- the trust question: a formula from an untrusted tap asks, and Trust installs it -----------

ax tab Services >/dev/null
ax press "Add service…" >/dev/null
ax pick Engine MinIO >/dev/null
ax type "Port (optional)" 28900 >/dev/null
ax press Add >/dev/null
asked() { ax dialog | jq -e '.text | any(test("Trust minio"))'; }
added() { y services --json | jq -e 'any(.[]; .name == "minio")'; }
if within 20 asked; then
  question="$(ax dialog | jq -c .)"
  ax answer Trust >/dev/null
  if within 30 added && jq -e '.formulae | index("minio/stable/minio")' "$brew_dir/fake-trust.json" >/dev/null; then
    pass "trust question: adding MinIO from the untrusted minio/stable asks ($question), and Trust trusts the formula and adds it"
  else
    fail "trust question: Trust did not add minio" "$(cat "$brew_dir/fake-trust.json") $(alerts)"
  fi
else
  fail "trust question: never asked" "$(alerts)"
fi

# --- N2: a Node install runs as a job ---------------------------------------------------------

node_22() { y node list --json | jq -e 'any(.versions[]; .version | startswith("22."))'; }
ax tab Node >/dev/null
ax type "Node version to install" 22 >/dev/null
ax press Install >/dev/null
if within 30 node_22; then
  ax tab Activity >/dev/null
  if texts | grep -qx "Install Node 22"; then
    pass "N2 Install 22 runs as a job and installs $(y node list --json | jq -r '[.versions[].version] | join(", ")')"
  else
    fail "N2 Activity has no Install Node job" "$(texts | tr '\n' '|')"
  fi
else
  fail "N2 node 22 was not installed" "$(alerts)"
fi

# --- T1, T2: a quick tunnel shows its URL, and Stop sharing ends it ---------------------------

tunnel_url() { y tunnels --json | jq -r '.[] | select(.site == "chk-t") | .url // empty'; }
shared() { [[ -n "$(tunnel_url)" ]]; }
unshared() { [[ -z "$(tunnel_url)" ]]; }
ax tab Tunnels >/dev/null
ax type "Site to share" chk-t >/dev/null
ax press "Share (quick tunnel)" >/dev/null
if within 30 shared; then
  url="$(tunnel_url)"
  if within 10 sh -c "osascript -l JavaScript '$ax_js' tree | jq -e --arg u '$url' 'any(.[]; (.value // .name // \"\") | contains(\$u))'"; then
    shot tunnels
    pass "T1 Share (quick tunnel) shows its URL ($url), as yard tunnels does"
  else
    fail "T1 the Tunnels screen does not show $url" "$(texts | tr '\n' '|')"
  fi
  ax press "Stop sharing" chk-t >/dev/null
  if within 20 unshared; then pass "T2 Stop sharing ends it"; else fail "T2 stop" "$(y tunnels --json)"; fi
else
  fail "T1 no tunnel" "$(y tunnels --json) $(alerts)"
fi

stop_app

if [[ "$(agent_sum)" == "$agent_before" ]]; then pass "yardd's LaunchAgent is untouched"; else fail "yardd's LaunchAgent changed" "$(agent_sum)"; fi
if [[ "$(resolver_sum)" == "$resolver_before" ]]; then pass "/etc/resolver is untouched"; else fail "/etc/resolver changed" "$(resolver_sum)"; fi

echo
echo "$passed passed, $failed failed; report in $out" | tee -a "$report"
((failed == 0))
