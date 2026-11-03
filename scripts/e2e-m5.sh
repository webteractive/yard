#!/usr/bin/env bash
# M5 end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Shares a scratch PHP site through a quick tunnel (no Cloudflare account) and checks what a
# visitor's request looks like to PHP: HTTPS on, the public host, and the visitor's address
# rather than 127.0.0.1. The tunnel's origin listener serves only that site whatever Host it is
# sent. `yard untunnel` then leaves no unit and no port entry behind.
#
# Needs: yardd running, built from this tree (e2e-m1.sh), brew's cloudflared, jq and the
# network (trycloudflare.com). Installs nothing. Ask the maintainer first.
#
# sites.toml and ports.toml are backed up and put back if they differ; the scratch site's
# override stubs are removed. Herd's resolver must come out unchanged, and cloudflared must not
# have made ~/.cloudflared (Yard runs it with its own config, and a quick tunnel needs no login).
# A named tunnel needs a Cloudflare zone and login, so it is the maintainer's manual step (see AGENTS.md).
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
site="yard-e2e-tunnel"

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# 1. What this needs, before anything is touched.
[[ -x $yard ]] || fail "$yard is not built; run cargo build --release"
command -v jq >/dev/null || fail "jq is not installed (brew install jq)"
cloudflared="$(brew --prefix)/opt/cloudflared/bin/cloudflared"
[[ -x $cloudflared ]] || fail "brew's cloudflared is not installed (brew install cloudflared)"
"$yard" status --json >/dev/null 2>&1 || fail "yardd is not running; run yard start and scripts/e2e-m1.sh first"
if "$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null; then
  fail "a site named $site already exists"
fi
other="$("$yard" sites --json | jq -r --arg n "$site" '[.[] | select(.name != $n)][0].name // empty')"
pass "cloudflared $("$cloudflared" --version 2>&1 | head -n1 | awk '{print $3}') is installed"

resolver_before="$(sum "$herd_resolver")"
had_cloudflared_dir=no
[[ -e $HOME/.cloudflared ]] && had_cloudflared_dir=yes
work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-XXXXXX")"
for f in sites.toml ports.toml; do
  if [[ -f $data/$f ]]; then cp -p "$data/$f" "$work/$f.bak"; fi
done

cleanup() {
  local stub f
  "$yard" untunnel "$site" >/dev/null 2>&1 || true
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  # The override stubs Yard created for the site, unless someone has edited them.
  for stub in "$data/nginx/overrides/$site.conf" "$data/nginx/tunnel-overrides/$site.conf"; do
    if [[ -f $stub ]] && ! grep -qv '^#' "$stub"; then rm -f "$stub"; fi
  done
  for f in sites.toml ports.toml; do
    if [[ -f $work/$f.bak ]]; then
      cmp -s "$work/$f.bak" "$data/$f" || cp -p "$work/$f.bak" "$data/$f"
    fi
  done
  rm -rf "$work"
}
trap cleanup EXIT

# 2. A scratch project, outside ~/Herd, whose page shows what PHP was told about the request.
cp -R "$fixtures/hello-php" "$work/$site"
cat >"$work/$site/public/index.php" <<'PHP'
<?php
echo "site=yard-e2e-tunnel\n";
echo "https=", $_SERVER['HTTPS'] ?? '', "\n";
echo "host=", $_SERVER['HTTP_HOST'] ?? '', "\n";
echo "addr=", $_SERVER['REMOTE_ADDR'] ?? '', "\n";
PHP
(cd "$work/$site" && "$yard" link "$site" >/dev/null) || fail "yard link $site failed"
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "http://$site.yard:8080/")"
[[ $code == 200 ]] || fail "http://$site.yard:8080/ answered $code, expected 200"
pass "linked $site"

# 3. A quick tunnel. --json prints the job's events, then the TunnelInfo as the last line.
(cd "$work/$site" && "$yard" tunnel --quick --json) >"$work/tunnel.ndjson" || fail "yard tunnel --quick failed: $(cat "$work/tunnel.ndjson")"
info="$(tail -n1 "$work/tunnel.ndjson")"
url="$(jq -r '.url' <<<"$info")"
origin="$(jq -r '.origin_port' <<<"$info")"
[[ $(jq -r '.state' <<<"$info") == ready ]] || fail "the tunnel is not ready: $info"
[[ $url == https://*.trycloudflare.com ]] || fail "the tunnel's URL is not a trycloudflare.com one: $url"
public_host="${url#https://}"
pass "shared $site at $url (origin port $origin)"

# 4. A visitor's request. A new trycloudflare.com name can take a while to resolve.
page=""
for _ in $(seq 1 30); do
  page="$(curl -s --max-time 10 "$url/" || true)"
  [[ $page == site=yard-e2e-tunnel* ]] && break
  sleep 2
done
[[ $page == site=yard-e2e-tunnel* ]] || fail "$url did not serve $site within 60 s: $page"
grep -qx 'https=on' <<<"$page" || fail "PHP did not see HTTPS=on through the tunnel: $page"
grep -qx "host=$public_host" <<<"$page" || fail "PHP did not see the public host $public_host: $page"
addr="$(sed -n 's/^addr=//p' <<<"$page")"
[[ -n $addr && $addr != 127.0.0.1 && $addr != ::1 ]] || fail "PHP saw REMOTE_ADDR $addr, not the visitor's address: $page"
pass "through the tunnel PHP sees HTTPS=on, Host $public_host and REMOTE_ADDR $addr"

# 5. The origin listener serves only this site, whatever Host it is sent.
if [[ -n $other ]]; then
  page="$(curl -s --max-time 10 -H "Host: $other.yard" "http://127.0.0.1:$origin/")"
  [[ $page == site=yard-e2e-tunnel* ]] || fail "the origin port answered Host $other.yard with another site: $page"
  pass "the origin port answers Host $other.yard with $site"
fi

# 6. yard untunnel leaves no unit and no port entry.
"$yard" untunnel "$site" >/dev/null || fail "yard untunnel $site failed"
if "$yard" status --json | jq -e --arg u "tunnel-$site" '.units[] | select(.name == $u)' >/dev/null; then
  fail "the unit tunnel-$site is still there"
fi
entries="$("$yard" ports --json | jq -r --arg s "$site" '.[] | select(.unit == "tunnel-\($s)" or .unit == "nginx-tunnel-\($s)") | .unit')"
[[ -z $entries ]] || fail "port entries left after untunnel: $entries"
if "$yard" tunnels --json | jq -e --arg s "$site" '.[] | select(.site == $s)' >/dev/null; then
  fail "yard tunnels still lists $site"
fi
pass "untunnel removed the unit and both port entries"

# 7. Clean up, then Herd's resolver and ~/.cloudflared, after.
trap - EXIT
cleanup
if "$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null; then
  fail "$site is still linked"
fi
pass "unlinked $site"
[[ $(sum "$herd_resolver") == "$resolver_before" ]] || fail "Herd's $herd_resolver changed"
pass "Herd's resolver is unchanged"
if [[ $had_cloudflared_dir == no && -e $HOME/.cloudflared ]]; then
  fail "$HOME/.cloudflared was created during the run"
fi
pass "no ~/.cloudflared was created"
echo "M5 e2e passed."
