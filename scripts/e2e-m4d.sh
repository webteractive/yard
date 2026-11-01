#!/usr/bin/env bash
# M4d end-to-end check on a real Mac, in the standard profile, after e2e-m1.sh passes.
#
# Checks Yard against the emailulator LaunchAgent already on this machine: `yard doctor`
# names the agent, `yard ports` has its SMTP port as an external entry, mail.yard:8080 serves
# its web UI while the agent's own port refuses that Host (so the rewrite is what makes it
# work), and a scratch project whose .yard says `emailulator = true` serves and prints the
# agent's MAIL_PORT in `yard env`.
#
# Needs: yardd running, built from this tree (e2e-m1.sh), emailulator 0.1.3 or later with its
# LaunchAgent answering, and jq. Installs nothing. Ask the maintainer first.
#
# Read-only towards emailulator: it only runs `emailulator service status --json`, and never
# installs, uninstalls, starts or stops the agent (option B: the agent runs itself). It stops
# before changing anything when no agent answers. sites.toml is backed up and put back if it
# differs, and the scratch override is removed. Herd's resolver must come out unchanged.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
fixtures="$repo/tests/fixtures/e2e"
data="$HOME/.yard"
herd_resolver=/etc/resolver/test
emailulator="${EMAILULATOR:-$HOME/.local/bin/emailulator}"
site="yard-e2e-mail"

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { shasum -a 256 "$1" | cut -d' ' -f1; }

# 1. What this needs, before anything is touched.
[[ -x $yard ]] || fail "$yard is not built; run cargo build --release"
command -v jq >/dev/null || fail "jq is not installed (brew install jq)"
"$yard" status --json >/dev/null 2>&1 || fail "yardd is not running; run yard start and scripts/e2e-m1.sh first"
[[ -x $emailulator ]] || fail "$emailulator is not there; install emailulator first (the maintainer's step)"
agent="$("$emailulator" service status --json)" || fail "emailulator's LaunchAgent does not answer (exit $?); this script never starts it"
label="$(jq -r '.label' <<<"$agent")"
smtp="$(jq -r '.smtp_addr' <<<"$agent")"
http="$(jq -r '.http_addr' <<<"$agent")"
smtp_port="${smtp##*:}"
[[ $(jq -r '.answering' <<<"$agent") == true ]] || fail "$label is not answering"
if "$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null; then
  fail "a site named $site already exists"
fi
if "$yard" sites --json | jq -e '.[] | select(.name == "mail")' >/dev/null; then
  fail "a site named mail takes mail.yard; unlink it first"
fi
pass "$label answers: SMTP $smtp, HTTP $http"

resolver_before="$(sum "$herd_resolver")"
work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-XXXXXX")"
if [[ -f $data/sites.toml ]]; then cp -p "$data/sites.toml" "$work/sites.toml.bak"; fi

cleanup() {
  local stub
  "$yard" unlink "$site" >/dev/null 2>&1 || true
  # The override stub Yard created for the site, unless someone has edited it.
  stub="$data/nginx/overrides/$site.conf"
  if [[ -f $stub && $(wc -l <"$stub") -le 1 ]] && grep -q '^# USER-OWNED' "$stub"; then rm -f "$stub"; fi
  if [[ -f $work/sites.toml.bak ]]; then
    cmp -s "$work/sites.toml.bak" "$data/sites.toml" || cp -p "$work/sites.toml.bak" "$data/sites.toml"
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# 2. yard doctor names the agent.
"$yard" doctor --json >"$work/doctor.json" || true
check="$(jq -c '.checks[] | select(.id == "emailulator")' "$work/doctor.json")"
[[ -n $check ]] || fail "yard doctor has no emailulator check"
[[ $(jq -r '.status' <<<"$check") == ok ]] || fail "yard doctor's emailulator check is not ok: $check"
[[ $(jq -r '.message' <<<"$check") == *"LaunchAgent $label"* ]] || fail "yard doctor does not name $label: $check"
pass "yard doctor: $(jq -r '.message' <<<"$check")"

# 3. The agent's SMTP port is an external entry.
entry="$("$yard" ports --json | jq -c '.[] | select(.unit == "emailulator-smtp")')"
[[ -n $entry ]] || fail "yard ports has no emailulator-smtp"
[[ $(jq -r '.mode' <<<"$entry") == external ]] || fail "emailulator-smtp is not external: $entry"
[[ $(jq -r '.port' <<<"$entry") == "$smtp_port" ]] || fail "emailulator-smtp is not on $smtp_port: $entry"
pass "yard ports: emailulator-smtp external on $smtp_port"

# 4. mail.yard:8080 serves the web UI; the agent's own port refuses that Host.
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 http://mail.yard:8080/)"
[[ $code == 200 ]] || fail "http://mail.yard:8080/ answered $code, expected 200"
direct="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 -H 'Host: mail.yard:8080' "http://$http/")"
[[ $direct == 403 ]] || fail "$http answered $direct to Host mail.yard:8080, expected 403 (emailulator's host check)"
pass "mail.yard:8080 answers 200, and $http answers 403 to that Host without Yard's rewrite"

# 5. A scratch project, outside ~/Herd, whose .yard asks for emailulator.
cp -R "$fixtures/hello-php" "$work/$site"
printf '[services]\nemailulator = true\n' >"$work/$site/.yard"
(cd "$work/$site" && "$yard" link "$site" >/dev/null) || fail "yard link $site failed"
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "http://$site.yard:8080/")"
[[ $code == 200 ]] || fail "http://$site.yard:8080/ answered $code, expected 200 (503 is the setup page)"
env_out="$(cd "$work/$site" && "$yard" env)"
grep -qx "MAIL_PORT=$smtp_port" <<<"$env_out" || fail "yard env in $site does not print MAIL_PORT=$smtp_port: $env_out"
grep -qx "MAIL_HOST=${smtp%:*}" <<<"$env_out" || fail "yard env in $site does not print MAIL_HOST=${smtp%:*}: $env_out"
pass "$site serves, and yard env prints MAIL_PORT=$smtp_port"

# 6. Clean up, then the agent and Herd's resolver, after.
trap - EXIT
cleanup
"$yard" sites --json | jq -e --arg n "$site" '.[] | select(.name == $n)' >/dev/null && fail "$site is still linked"
pass "unlinked $site"
after="$("$emailulator" service status --json)" || fail "emailulator's LaunchAgent stopped answering during the run"
[[ $(jq -r '.pid' <<<"$after") == $(jq -r '.pid' <<<"$agent") ]] || fail "$label's pid changed during the run"
pass "$label is untouched (same pid)"
[[ $resolver_before == "$(sum "$herd_resolver")" ]] || fail "$herd_resolver changed"
pass "$herd_resolver is unchanged"
