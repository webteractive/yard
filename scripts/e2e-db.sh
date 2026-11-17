#!/usr/bin/env bash
# `yard mysql` and `yard postgres` (once `yard db`) end-to-end check on a real Mac (db spec §12), after e2e-m1.sh passes.
#
# On the real MySQL and PostgreSQL instances (YARD_DB_MYSQL and YARD_DB_PG; each defaults to the
# engine's only instance, else mysql-8.4 and postgres-17; a missing PostgreSQL instance skips its
# half) it creates scratch
# databases named yard_e2e_<pid>_* and checks:
#   - `yard mysql create` makes one and leaves an existing one alone;
#   - `yard mysql import` of plain SQL (with strings that look like statements), of the same file
#     gzipped and named .sql, and of a dump `yard mysql dump` wrote, each gives the same rows;
#   - a `mysqldump --databases` dump stops before its USE (DB_DUMP_SWITCHES_DATABASE), and one
#     imported under the name it carries is followed;
#   - an import into a database with tables is DB_NOT_EMPTY;
#   - a flag passed after -- reaches the tool (--no-data), and --host is DB_FLAG_RESERVED;
#   - PostgreSQL: a plain dump whose OWNER TO names a role the instance lacks imports with that
#     statement skipped, and a `yard postgres dump -- -Fc` archive imports through pg_restore;
#   - when an instance is asleep at the start, `yard mysql list` wakes it.
#
# Needs: yardd running, built from this tree, the instances above, and jq. Installs nothing,
# changes no TOML. It writes to the real instances, so ask the maintainer first. It drops only
# the databases it created, through each instance's own client by absolute keg path, and fails
# if any other database appeared or vanished, if the listeners on 3306, 5432 and 6379 changed,
# or if Herd's resolver changed. It never touches DBngin, Postgres.app or Herd.
set -euo pipefail
unset YARD_ROOT

repo="$(cd "$(dirname "$0")/.." && pwd)"
yard="${YARD:-$repo/target/release/yard}"
run_dir="$HOME/.yard/run"
herd_resolver=/etc/resolver/test
prefix="yard_e2e_$$"
foreign_ports=(3306 5432 6379)

fail() { echo "FAIL $*" >&2; exit 1; }
pass() { echo "ok   $*"; }
sum() { if [[ -e $1 ]]; then shasum -a 256 "$1" | cut -d' ' -f1; else echo absent; fi; }

listeners() {
  local port
  for port in "${foreign_ports[@]}"; do
    yard_holds "$port" && continue
    printf '%s: %s\n' "$port" "$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fpc 2>/dev/null | grep -E '^[pc]' | sort -u | tr '\n' ' ')"
  done
}

# Ports Yard's own instances are given (moved there with `yard service port`, say): they sleep
# and wake on their own, so they are left out of the listeners compared.
yard_holds() { grep -qx "$1" <<<"$yard_ports"; }

field() { "$yard" services --json | jq -r --arg n "$1" ".[] | select(.name == \$n) | $2"; }
has_instance() { [[ -n $(field "$1" '.name') ]]; }
keg_of() { echo "$(brew --prefix)/opt/$(field "$1" '.formula' | awk -F/ '{print $NF}')"; }
port_of() { field "$1" '.ports[0].port'; }
asleep() { [[ $(field "$1" '.sleeping_since // empty') != "" && $(field "$1" '.state') == stopped ]]; }

command -v jq >/dev/null || fail "jq is needed"
[[ -x $yard ]] || fail "no yard at $yard (cargo build --release)"
"$yard" status >/dev/null || fail "yardd is not running"

# The instance to use for engine $1: $2 when it is set, else the engine's only instance, else $3.
pick() {
  local names
  if [[ -n $2 ]]; then echo "$2"; return; fi
  names="$("$yard" services --json | jq -c --arg e "$1" '[.[] | select(.engine == $e) | .name]')"
  if [[ $(jq length <<<"$names") == 1 ]]; then jq -r '.[0]' <<<"$names"; else echo "$3"; fi
}
mysql_instance="$(pick mysql "${YARD_DB_MYSQL:-}" mysql-8.4)"
pg_instance="$(pick postgres "${YARD_DB_PG:-}" postgres-17)"
echo "instances: $mysql_instance and $pg_instance"
has_instance "$mysql_instance" || fail "no $mysql_instance instance (yard service add mysql)"
with_pg=1
has_instance "$pg_instance" || { with_pg=0; echo "skip PostgreSQL: no $pg_instance instance"; }

my_keg="$(keg_of "$mysql_instance")"
my() { "$my_keg/bin/mysql" --no-defaults --socket="$run_dir/$mysql_instance.sock" --user=root -N -B "$@"; }
if (( with_pg )); then
  pg_keg="$(keg_of "$pg_instance")"
  pg_port="$(port_of "$pg_instance")"
  pg() { "$pg_keg/bin/psql" --no-psqlrc -h "$run_dir" -p "$pg_port" -U "$USER" -At "$@"; }
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/yard-e2e-db.XXXXXX")"
resolver_before="$(sum "$herd_resolver")"
yard_ports="$("$yard" services --json | jq -r '.[].ports[]?.port')"
for port in "${foreign_ports[@]}"; do
  if yard_holds "$port"; then echo "not compared: Yard's own instance holds $port"; fi
done
listeners_before="$(listeners)"
my_asleep_at_start=0
asleep "$mysql_instance" && my_asleep_at_start=1

cleanup() {
  local db
  for db in $(my -e "SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE '${prefix}%'" 2>/dev/null); do
    my -e "DROP DATABASE \`$db\`" || echo "could not drop $db" >&2
  done
  if (( with_pg )); then
    for db in $(pg -d postgres -c "SELECT datname FROM pg_database WHERE datname LIKE '${prefix}%'" 2>/dev/null); do
      pg -d postgres -c "DROP DATABASE \"$db\"" >/dev/null || echo "could not drop $db" >&2
    done
  fi
  rm -rf "$work"
}
trap cleanup EXIT

if (( my_asleep_at_start )); then
  "$yard" mysql list --service "$mysql_instance" >/dev/null || fail "db list on a sleeping $mysql_instance"
  [[ $(field "$mysql_instance" '.state') == running ]] || fail "db list did not wake $mysql_instance"
  pass "db list woke a sleeping $mysql_instance"
else
  echo "skip the wake check: $mysql_instance was awake"
fi
(( with_pg )) && "$yard" postgres list --service "$pg_instance" >/dev/null

my_dbs_before="$(my -e "SELECT schema_name FROM information_schema.schemata ORDER BY 1")"
(( with_pg )) && pg_dbs_before="$(pg -d postgres -c "SELECT datname FROM pg_database ORDER BY 1")"

# A database's error code from a failed --json run, or "ok".
# --json goes right after yard, since anything after -- is the tool's.
code() { local out; if out="$("$1" --json "${@:2}" 2>/dev/null)"; then echo ok; else echo "$out" | jq -r '.error.code // "?"'; fi; }

# --- MySQL ---------------------------------------------------------------------------------
cd "$work"
cat > plain.sql <<'SQL'
CREATE TABLE t (id int PRIMARY KEY, note varchar(60));
INSERT INTO t VALUES (1, 'it''s; USE elsewhere'), (2, 'a "CREATE DATABASE x"'), (3, 'back\\slash');
SQL
rows_my() { my -e "SELECT COUNT(*) FROM \`$1\`.t"; }

[[ $(code "$yard" mysql create "${prefix}_e" --service "$mysql_instance") == ok ]] || fail "db create"
[[ $("$yard" mysql create "${prefix}_e" --service "$mysql_instance" --json | jq -r .created) == false ]] || fail "a second create said created"
pass "db create makes a database once"
# yard db still works for one release, and says what replaces it (recipes spec §9.1).
"$yard" db list --service "$mysql_instance" 2>&1 >/dev/null | grep -q "yard db is going away" \
  || fail "yard db did not print its notice"
pass "yard db still works and names yard <engine>"

"$yard" mysql import "${prefix}_a" plain.sql --service "$mysql_instance" >/dev/null || fail "plain import"
[[ $(rows_my "${prefix}_a") == 3 ]] || fail "plain import rows"
[[ $(my -e "SELECT note FROM \`${prefix}_a\`.t WHERE id = 1") == "it's; USE elsewhere" ]] || fail "a string that looks like USE was changed"
gzip -c plain.sql > gzipped.sql
"$yard" mysql import "${prefix}_b" gzipped.sql --service "$mysql_instance" >/dev/null || fail "gzipped import"
[[ $(rows_my "${prefix}_b") == 3 ]] || fail "gzipped import rows"
pass "plain and gzipped imports give the same rows"

"$my_keg/bin/mysqldump" --no-defaults --socket="$run_dir/$mysql_instance.sock" --user=root --databases "${prefix}_a" > databases.sql
[[ $(code "$yard" mysql import "${prefix}_c" databases.sql --service "$mysql_instance") == DB_DUMP_SWITCHES_DATABASE ]] || fail "a --databases dump was not stopped"
pass "a mysqldump --databases dump stops before its USE"
# The same kind of dump imported under the name it carries is followed: its CREATE DATABASE is
# skipped (yard made it) and its USE runs.
sed "s/${prefix}_a/${prefix}_f/g" databases.sql > own.sql
"$yard" mysql import "${prefix}_f" own.sql --service "$mysql_instance" >/dev/null || fail "a -B dump under its own name"
[[ $(rows_my "${prefix}_f") == 3 ]] || fail "-B dump rows"
pass "a mysqldump -B dump imports under the name it carries"

[[ $(code "$yard" mysql import "${prefix}_a" plain.sql --service "$mysql_instance") == DB_NOT_EMPTY ]] || fail "import into tables was not DB_NOT_EMPTY"
pass "an import into a database with tables is refused"

"$yard" mysql dump "${prefix}_a" dumped.sql --service "$mysql_instance" -- --single-transaction >/dev/null || fail "db dump"
[[ ! -e dumped.sql.partial ]] || fail "a .partial was left"
"$yard" mysql import "${prefix}_d" dumped.sql --service "$mysql_instance" >/dev/null || fail "re-import of the dump"
[[ $(rows_my "${prefix}_d") == 3 ]] || fail "re-imported rows"
pass "a dump re-imports with the same rows"

"$yard" mysql dump "${prefix}_a" schema.sql --service "$mysql_instance" -- --no-data >/dev/null || fail "dump --no-data"
! grep -q 'INSERT INTO' schema.sql || fail "--no-data did not reach mysqldump"
[[ $(code "$yard" mysql dump "${prefix}_a" x.sql --service "$mysql_instance" -- --host=elsewhere) == DB_FLAG_RESERVED ]] || fail "--host was not refused"
pass "a flag after -- reaches mysqldump, and --host is refused"

# --- PostgreSQL ----------------------------------------------------------------------------
if (( with_pg )); then
  cat > pg.sql <<'SQL'
CREATE TABLE t (id int PRIMARY KEY, note text);
ALTER TABLE t OWNER TO yard_e2e_no_such_role;
INSERT INTO t VALUES (1, 'it''s; \connect elsewhere'), (2, $$GRANT ALL$$), (3, E'back\\slash');
SQL
  rows_pg() { pg -d "$1" -c "SELECT COUNT(*) FROM t"; }
  skipped="$("$yard" postgres import "${prefix}_pa" pg.sql --service "$pg_instance" --json | jq -r .skipped)" || fail "pg plain import"
  [[ $skipped == 1 ]] || fail "the foreign OWNER TO was not skipped ($skipped)"
  [[ $(rows_pg "${prefix}_pa") == 3 ]] || fail "pg plain import rows"
  pass "a PostgreSQL dump naming a missing role imports with OWNER TO skipped"

  "$yard" postgres dump "${prefix}_pa" archive.dump --service "$pg_instance" -- -Fc >/dev/null || fail "pg_dump -Fc"
  [[ $(head -c 5 archive.dump) == PGDMP ]] || fail "not a custom-format archive"
  "$yard" postgres import "${prefix}_pb" archive.dump --service "$pg_instance" >/dev/null || fail "archive import"
  [[ $(rows_pg "${prefix}_pb") == 3 ]] || fail "archive rows"
  pass "a pg_dump -Fc archive imports through pg_restore"
fi

# --- Nothing else changed ------------------------------------------------------------------
cd /
cleanup
trap - EXIT
[[ $(my -e "SELECT schema_name FROM information_schema.schemata ORDER BY 1") == "$my_dbs_before" ]] || fail "MySQL's databases changed"
if (( with_pg )); then
  [[ $(pg -d postgres -c "SELECT datname FROM pg_database ORDER BY 1") == "$pg_dbs_before" ]] || fail "PostgreSQL's databases changed"
fi
[[ $(listeners) == "$listeners_before" ]] || fail "the listeners on ${foreign_ports[*]} changed"
[[ $(sum "$herd_resolver") == "$resolver_before" ]] || fail "Herd's resolver changed"
pass "only the scratch databases came and went; other servers untouched"
echo "e2e-db: all checks passed"
