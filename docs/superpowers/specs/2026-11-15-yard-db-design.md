# Yard databases: `yard db` and connection details — design

Status: **approved by the maintainer on 2026-11-15.** A builder read it cold the same day; its
questions are answered below (§15 lists what they changed). It changes one M4b decision ("Yard never
creates databases", M4b spec §2) as §2 says, and builds on the M4b service model
(`docs/superpowers/specs/2026-10-23-yard-m4b-services-design.md`) and M4e's sleep
(`docs/superpowers/specs/2026-10-26-yard-m4e-on-demand-services-design.md`).

Decisions marked **(maintainer)** are the maintainer's, from the 2026-11-15 conversation. Decisions
marked **(builder's call)** were made while writing this and can be reversed on their own.

## 1. Intent

The maintainer is moving about 30 projects off Herd and DBngin onto Yard. Each project's database
has to come along, and the M4b spec's answer was "a dump and a re-import into the Yard instance",
with the dump, the import, the `CREATE DATABASE` and finding the right client and port all left to
the user. That is the friction this removes **(maintainer: mainly for migration)**. Past the
migration, listing, creating and opening a project's database are a nice default to have
**(maintainer)**.

The second half is telling a GUI client how to connect. TablePlus, Sequel Ace, DBeaver and
DataGrip all need a host, port, user and password, and some take a URL or a socket; today the only
place those appear is `yard env`'s `.env` lines **(maintainer asked for it)**.

## 2. Decisions

- **`yard db` covers list, create, shell, open, dump and import (maintainer).** For MySQL and
  PostgreSQL instances only; connection details (§8) also cover Redis.
- **No drop, and no users, roles or grants (maintainer).** Destroying a database stays with the
  user's client (`yard db shell`). Yard creates no users: every login is the instance's own
  (`root` with no password on MySQL, the macOS user over trust on PostgreSQL, M4b §7.5).
- **Import takes a file, never another server (maintainer, option 1 of §4).** Yard never connects
  to DBngin's, Postgres.app's, Herd's or any other server, and never reads their files. The M4b
  non-goal "importing data from DBngin or Postgres.app" stands: the user makes the dump with their
  own tools (Sequel Ace, DBngin's `mysqldump`, `pg_dump`), and Yard imports the file.
- **This changes M4b's "Yard never creates databases".** It becomes: Yard creates a database only
  when the user asks, through `yard db create` or `yard db import` into a name that does not exist
  (or the same in Yard.app). `yard link`, `yard init` and a `.yard` still never create one, and Yard
  still creates no roles or buckets. The M4b spec's §2 and non-goals are edited to say so.
- **Dump and import take every flag their tool takes (maintainer).** Everything after `--` goes to
  the tool verbatim, except the flags that would point it at another server or database, which are
  Yard's and refused (§7).
- **Dump compression is a flag (maintainer):** `--compress`, gzip, off by default (§6.5).
- **An import lands only in the database it names (maintainer, 2026-11-15).** A dump that switches
  database (`USE`, `CREATE DATABASE`, `\connect`) is refused before that statement runs (§6.6).
- **PostgreSQL imports skip ownership and grants by default (maintainer, 2026-11-15).** A dump from
  another server names roles a Yard instance does not have; everything imported is owned by the
  instance's superuser, the macOS user. `--keep-owners` turns it off (§6.6).
- **The client runs dump and import, not yardd (builder's call, from the builder check).** yardd is
  a LaunchAgent with its own macOS privacy (TCC) identity, so reading `~/Downloads/shop.sql` or
  writing to `~/Desktop` from yardd would raise an access prompt (again after each ad hoc rebuild)
  or be refused. The CLI runs in the user's terminal, and Yard.app opens the file through an open or
  save panel, which grants access. yardd still decides everything: it resolves the instance, wakes
  it, creates the database, checks it is empty, validates the flags and answers with the exact
  command to run (§11); the client only runs it and streams the file. This also keeps a long import
  off yardd's single job queue, where it would hold up installs and tunnels.
- **Every `yard db` command wakes a sleeping instance first (builder's call).** A stopped one is
  refused, as Yard never starts an instance the user stopped (M4b §9).

## 3. Goals, non-goals, anti-goals

Goals:

1. Moving a project's MySQL database from DBngin is: dump it in Sequel Ace or with DBngin's
   `mysqldump`, then `yard db import shop shop.sql` (or a gzipped `.sql.gz`) in the project. The
   same for PostgreSQL from Postgres.app with `pg_dump -Fc` and `yard db import`, with no
   `--no-owner` to remember.
2. `yard db list` shows each instance's databases and their sizes.
3. `yard db connect` tells a person exactly what to type into TablePlus, Sequel Ace, DBeaver or
   DataGrip, and `yard db open` opens the database in whichever of them handles the URL.
4. `yard db shell` opens the right client, by absolute keg path, on the right instance, never a
   bare `mysql` or `psql` that could be DBngin's, Postgres.app's or Herd's.
5. Yard.app does all of it from the Services screen.

Non-goals:

- Dropping or renaming databases; users, roles, grants or passwords.
- Connecting to any server that is not a Yard instance (no `--from`, no `--host`).
- Copying a database between Yard instances in one step (dump, then import).
- MinIO buckets, Redis keyspaces, emailulator.
- Reading a project's `.env` for its `DB_DATABASE` (Yard reads no `.env`, project-file spec §6).
- Scheduled backups; dump or import running after the terminal that started it closes.

Anti-goals (a failure even if it works):

- A `yard db` command reaching a server that is not the Yard instance it names.
- An import writing to any database but the one it names, or into one that already has tables
  without the user saying so.
- A cancelled or failed dump leaving a file that looks complete.
- A `psql` import reporting success after errors (psql carries on past errors unless told not to).
- Any `yard db` command starting an instance the user stopped.

## 4. Approaches

Where the data for an import comes from was the open question; the maintainer chose 1.

1. **A dump file only (chosen).** `yard db import shop ~/Downloads/shop.sql.gz`. The user dumps
   with their own tools. Yard never talks to another server, so the M4b line on DBngin and
   Postgres.app holds. Two steps per project.
2. **A file, or straight from the old server.** `yard db import shop --from
   mysql://root@127.0.0.1:3306/shop` would run Yard's keg `mysqldump` against the old server and
   stream it in. One step per project, but Yard becomes a client of other servers, softening the
   M4b non-goal.
3. **A bulk migration.** `yard db migrate --from 127.0.0.1:3306` copying every database. Fastest
   for 30 projects, and dead weight after the migration.

## 5. Which instance and which database

- `--service <instance>` names the instance (`mysql-8.4`, `postgres-17`), as `yard env` does.
- Without it, in a linked project (or with `--site`), it is the database instance the project's
  `.yard` `[services]` asks for. A project asking for both MySQL and PostgreSQL needs `--service`
  (`DB_SERVICE_AMBIGUOUS`, hint naming both).
- A project whose `.yard` asks for no database (most projects fresh from Herd have no `.yard`), and
  anywhere outside a project, falls back to the machine: with exactly one MySQL or PostgreSQL
  instance, it is that one, and the human output says which (`using mysql-8.4`) **(builder's
  call)**; with several, `DB_SERVICE_AMBIGUOUS` lists them; with none, `DB_NO_SERVICE`, hint `yard
  service add mysql`. The params carry the CLI's `path`, as `EnvParams` does.
- A Redis, MinIO or emailulator instance named for anything but `connect` is
  `DB_UNSUPPORTED_ENGINE`.
- A database name is `[A-Za-z0-9_-]`, 1 to 63 characters (PostgreSQL's limit, under MySQL's 64)
  **(builder's call)**; anything else is `INVALID_ARGUMENT`. Yard quotes it as an identifier in
  every statement it runs. There is no default database name: Yard cannot read `DB_DATABASE`.

## 6. Commands

All of them support `--json` and `--service`/`--site` (§5), and wake the instance (§9).

### 6.1 `yard db list`

The instance's databases, sorted by name: name, size on disk, number of tables. System schemas
are left out (`mysql`, `sys`, `information_schema`, `performance_schema`; PostgreSQL's
`template0`, `template1` and `postgres`). With no `--service` outside a project, every MySQL and
PostgreSQL instance, one block each. Sleeping instances are woken to answer; a list that would
wake several says so on stderr first **(builder's call)**. yardd runs the queries itself, with the
instance's client by keg path over its socket.

### 6.2 `yard db create <name>`

`CREATE DATABASE` with `utf8mb4` and `utf8mb4_unicode_ci` on MySQL (the instance's defaults, M4b
§16), and `ENCODING 'UTF8'` owned by the instance's superuser on PostgreSQL. A name that exists
is not an error: it prints `shop already exists on mysql-8.4` and exits 0 (`created: false`).
yardd runs it.

### 6.3 `yard db shell [name] [-- <client flags>]`

Replaces itself with the instance's client by absolute keg path (`<keg>/bin/mysql` or
`<keg>/bin/psql`), logged in, on the database when one is named. Client flags pass through as in
§7. The connection goes over the instance's Unix socket. The CLI asks yardd for the command
(`db.command`, §11), which wakes the instance; it never builds a path or port on its own.

### 6.4 `yard db open [name]`

Asks yardd for the connection (waking the instance), then hands macOS the URL with `open`:
`mysql://root@127.0.0.1:3307/shop`, `postgresql://me@127.0.0.1:5433/shop`. macOS opens the
default handler for the scheme. On the Mac mini, Sequel Ace claims `mysql:` (Launch Services,
2026-11-15); TablePlus is said to claim `mysql:` and `postgresql:`, which the plan checks on a Mac
that has it. When no app handles the scheme, `open` exits 1 ("No application knows how to open
URL", checked 2026-11-15), and `yard db open` prints the connection details of §8 and exits 1 with
`DB_NO_HANDLER`, hint naming TablePlus.

### 6.5 `yard db dump <name> [file] [--compress] [-- <dump flags>]`

Runs `mysqldump` or `pg_dump` (§7.1 says which keg) against the named database and writes the file.

- **Compression is a flag (maintainer).** `--compress` gzips plain SQL output; without it the file
  is plain SQL. It is off by default **(builder's call: a plain `.sql` opens and imports anywhere,
  and the maintainer asked for compression as a flag)**. gzip is `/usr/bin/gzip` in the pipeline,
  so no crate is added **(builder's call)**; zstd can come later.
- **The flag decides, never the file name.** A file name ending `.gz` without `--compress` is
  `INVALID_ARGUMENT`, hint `--compress`. `pg_dump`'s own `-Z`/`--compress` with plain output is
  refused (`DB_FLAG_RESERVED`, hint `--compress`), so plain SQL is gzipped only one way.
  PostgreSQL's `-Fc`, `-Ft` and `-Fd` compress themselves: `-Z` passes through for them, and Yard's
  `--compress` with one of them is `INVALID_ARGUMENT`. MySQL's own `--compress` after `--`
  compresses the connection, not the file, and passes through.
- **Default file name**, in the current directory: `<name>-<YYYY-MM-DD-HHMM>.sql`, `.sql.gz` with
  `--compress`, `.dump` with `-Fc`, `.tar` with `-Ft`, and a directory `<name>-<YYYY-MM-DD-HHMM>`
  with `-Fd`.
- The dump is written to `<file>.partial` (created fresh, never through a symlink, mode 0600) and
  renamed when the tool (and gzip) exit 0. A failed or
  cancelled dump deletes the partial file, so no file that looks complete is left (§3).
- An existing file is refused (`FILE_EXISTS`, hint `--overwrite`) unless `--overwrite`.
- Yard adds nothing to the tool's own defaults: `mysqldump` gets no `--single-transaction` or
  `--routines` unless the user passes them **(builder's call: a flag the user knows behaves as it
  always has)**; the human output suggests `-- --single-transaction --routines` once, under the
  result, for MySQL.
- Progress is a line on stderr with the bytes written (`shop: 312 MB written`), updated in place on
  a terminal; Ctrl-C stops the tool.

### 6.6 `yard db import <name> <file> [--into-existing] [--keep-owners] [-- <import flags>]`

Feeds the file to the instance, creating the database first when it does not exist.

- **What reads it.** MySQL: `mysql`, the file on stdin. PostgreSQL, by content, not name: a file
  starting `PGDMP` (custom format), a tar archive, or a directory holding `toc.dat` (directory
  format) goes to `pg_restore -d <name>`; anything else is plain SQL for `psql`, run with
  `-v ON_ERROR_STOP=1` first, so an error fails the import (§3). A file starting with gzip's magic is
  gunzipped (`/usr/bin/gzip -dc`) first, whatever its name.
- **A database that has tables** (in any schema but PostgreSQL's system ones) is refused
  (`DB_NOT_EMPTY`, hint the same command with `--into-existing`) unless `--into-existing`. yardd
  checks this when it answers `db.command` (§11).
- **Only the named database (maintainer).** Plain SQL passes through a statement filter in the
  client (the rules in `yard-core`, pure). It holds each statement until it has read all of it, and
  a statement that switches, creates or drops a database (MySQL's `USE`, `CREATE|DROP
  DATABASE|SCHEMA`; PostgreSQL's `\connect`, `\c`, `CREATE|DROP DATABASE`; `DROP` added while
  planning, as `mysqldump --databases --add-drop-database` writes it ahead of the `CREATE`) is
  never sent; a PostgreSQL `CREATE SCHEMA` passes, being a namespace every `pg_dump` makes: the import stops with
  `DB_DUMP_SWITCHES_DATABASE` naming the database the dump asked for, hint `yard db import <that
  name> <file>` or re-dumping without it (`mysqldump` without `--databases`, `pg_dump` without
  `-C`). What ran before it stays, in the named database. The filter follows each dialect's
  quoting: MySQL's quotes, backticks, comments and `DELIMITER` blocks (a `USE` inside a procedure
  body is not a statement); PostgreSQL's quotes, dollar quoting, comments and `COPY … FROM stdin`
  data up to `\.`. An archive for `pg_restore` cannot switch on its own once `-C` is refused (§7).
- **Client commands (built, after a security review, 2026-11-15).** The mysql client runs its own
  commands from piped input (`\!` a shell command, `\u`, `connect`, `source`), so MySQL imports
  run with `--binary-mode`, which turns them off, and the filter also refuses a bare `\` outside
  quotes and a statement starting `connect`, `source` or `system`. psql has no such switch: any
  meta-command but `pg_dump`'s own `\restrict`/`\unrestrict` is refused, wherever it appears in a
  statement. The lexers read strings as psql and the server do (an `E'` only where `E` starts a
  token, bytes from 0x80 as identifier bytes, dollar tags included, `COPY` data only after `COPY
  <table> [(…)] FROM STDIN`), and a statement that would change how strings end
  (`standard_conforming_strings` but `= on`, `NO_BACKSLASH_ESCAPES`) is refused.
- **What the filters are for (builder's call, 2026-11-15).** They guard against accidents, not
  hostile dumps: an import runs as the instance's superuser, and PostgreSQL's superuser runs
  programs through `COPY … FROM PROGRAM`, so no lexer makes an untrusted dump safe. Import only
  dumps you trust; README and `--help` say so.
- **Ownership (maintainer).** Unless `--keep-owners`, `pg_restore` gets `--no-owner
  --no-privileges`, and the plain-SQL filter drops `ALTER … OWNER TO`, `GRANT`, `REVOKE`, `ALTER
  DEFAULT PRIVILEGES` and `SET ROLE`/`SET SESSION AUTHORIZATION`, counting them; the result says how
  many it skipped. MySQL dumps carry `DEFINER` clauses on views, triggers and routines, which work
  as `root` and are left alone.
- **On failure** the import exits 1 with `DB_TOOL_FAILED` and the tool's last lines (`data.tail`).
  What was imported stays: Yard drops nothing. The message says so and names `yard db shell`.
- **On cancel** (Ctrl-C) the tool is stopped; what it imported stays, and the message says so.
- Progress is a line on stderr with the bytes of the file read against its size (for a directory,
  the sum of its files).

## 7. Passing flags through

- Everything after `--` is appended to the tool's command line as separate arguments, with no
  shell: `mysqldump` or `pg_dump` for `dump`; `mysql`, `psql` or `pg_restore` for `import`; `mysql`
  or `psql` for `shell`.
- **Argument order (built).** MySQL's tools take Yard's flags, then the database name (`mysqldump`)
  or `--database=<name>` (`mysql`), then the user's, since MySQL's option parser reads options after
  the name (checked against mysqldump 8.4). PostgreSQL's take the database as `-d` and nothing
  positional before the user's flags (macOS's getopt does not reorder); a directory-format path
  comes last.
- **Yard owns the server, the database and the output.** yardd refuses, as a whole flag, a
  `--flag=value`, the short form with its value attached or separate, and any prefix of a refused
  long option of three characters or more (MySQL's option parser and getopt_long both take a unique
  prefix; MySQL's `_` spelling and `loose-` prefix are read as MySQL reads them). A cluster of short
  flags is judged by its first letter, so `-Fd` stays `-F d`:
  - every MySQL tool: `-h --host -P --port -u --user -p --password -S --socket --protocol
    --defaults-file --defaults-extra-file --defaults-group-suffix --login-path --no-defaults
    --print-defaults -D --database`;
  - `mysqldump` also `--databases -B --all-databases -A --result-file -r`;
  - `mysql` for `import` also `-e --execute --init-command` (they would run instead of, or before,
    the file);
  - every PostgreSQL tool: `-h --host -p --port -U --username -W --password -d --dbname -l --list -D
    --pgdata`;
  - `pg_dump` also `-f --file`, and `-Z --compress` with plain output (§6.5);
  - `pg_restore` also `-C --create -f --file`; `psql` for `import` also `-c --command -f --file`.

  Each is `DB_FLAG_RESERVED`, naming the flag and why (`--host: yard db only talks to its own
  instance`). `-w`/`--no-password` is allowed, and `shell` may use `-e` and `-c`.
- Anything else is the tool's to judge: an unknown flag fails with the tool's own error. Yard keeps
  no list of allowed flags, so a new tool version's flags work.
- A positional argument after `--` reaches the tool after the database name (a table list for
  `mysqldump`). The plan checks how each tool reads one before relying on it.
- Yard's own arguments come first and the user's after, so for a flag both set the user's wins
  where the tool takes the last one (`-v ON_ERROR_STOP=0` turns psql's stop back off).
- `dump` and `import` start MySQL's tools with `--no-defaults` and clear `PGSERVICEFILE`,
  `PGPASSFILE` and the other `PG*` connection variables for PostgreSQL's, so a `~/.my.cnf` or
  `~/.pgpass` cannot point them elsewhere. `shell` keeps the user's files (a prompt, a pager); its
  connection flags win over them.

### 7.1 Which keg

MySQL's tools come from the instance's own keg (`mysql@8.4` for `mysql-8.4`). PostgreSQL's come from
the newest PostgreSQL keg installed at or above the instance's major **(builder's call)**:
`pg_restore` and `psql` refuse an archive or syntax from a newer server, while a newer client works
against an older server, and Postgres.app is often a major ahead of a Yard instance. The human
output names the keg when it is not the instance's (`using pg_restore 18 for postgres-17`). An
archive newer than every installed keg fails with `pg_restore`'s own error, and the hint names `brew
install postgresql@<its version>`, which Yard never runs on its own.

## 8. Connection details

`yard db connect [instance]` (the argument is the instance; without one, §5's rules pick it, and
outside a project every MySQL, PostgreSQL and Redis instance is shown). It never wakes anything; it
shows the state.

```
mysql-8.4  sleeping — yard db open or yard db shell wakes it; or yard service start mysql-8.4
  Host      127.0.0.1
  Port      3307
  User      root
  Password  (none)
  Socket    /Users/me/.yard/run/mysql-8.4.sock
  URL       mysql://root@127.0.0.1:3307

  TablePlus, Sequel Ace: yard db open [database], or paste the URL
  DBeaver, DataGrip: a new MySQL connection with the fields above
```

- PostgreSQL shows the macOS user, `(none)` with trust, the socket directory `~/.yard/run` (the
  socket is `.s.PGSQL.<port>` in it, which clients find by host = that directory and the port), and
  `postgresql://<user>@127.0.0.1:<port>`. Redis shows host, port, no user or password, and
  `redis://127.0.0.1:<port>`; it has no socket.
- `--json` is `ConnectInfo` (instance, engine, state, `sleeping`, host, port, user, password,
  socket, url); `sleeping` (added while building) tells a sleeping instance from a stopped one.
- The paths are absolute (a GUI does not expand `~`). The URL carries no password (none exists).
- A GUI connection keeps the instance awake while it is open (M4e counts TCP and socket
  connections); once the GUI disconnects, the instance sleeps after its `sleep_after` as usual, and
  the GUI's next connect fails until something wakes it. The human output's state line says how.

## 9. Sleep and stopped instances

- `db.*` methods (and `db.connect` with `wake`) wake a sleeping instance through yardd's `Sleeper`,
  the same path as the wake hook, and wait up to 30 s for its port to answer. `sleep::wake` today
  returns when its wait runs out without saying so, so this adds `SERVICE_WAKE_TIMEOUT`
  (`<instance> did not come up within 30 s`, hint `yard service logs <instance>`).
- A stopped instance (`stopped = true`) is refused with `SERVICE_STOPPED`, hint `yard service
  start <instance>`; a failed one with its failure (M4b §7.2).
- A dump or import holds its connection for its whole run, so the instance cannot fall asleep under
  it (M4e counts socket connections too). The wake happens when yardd answers `db.command`, and the
  client connects at once.

## 10. Yard.app

On the Services screen, a MySQL or PostgreSQL instance's detail gains **Databases** (the `db.list`
rows, each with **Open**, **Dump…**), **Create database…**, **Import…** (an open panel for the
file, then the name, **Into existing**, **Keep owners** and **Extra options**), and **Connect…**;
Redis gets **Connect…**.

- **Connect…** is a sheet with each field of §8 and a Copy button beside it, **Copy URL**, and
  **Open** (the URL through the app's URL opener). It shows the state line and does not wake.
- **Dump…** takes a save panel for the file, a **Compress** switch (off) and an **Extra options**
  text field. The field is split into arguments like a shell would (quotes and backslashes, no
  variables or globbing) **(builder's call)**, and §7's refusals show in the sheet.
- The app runs dumps and imports through the CLI (`yard db dump|import … --progress-json`, decided
  while planning: the runner then exists once, and the CLI child inherits the file access the open
  and save panels grant); the progress JSON lines on stderr feed Activity. **Built:** the sheet stays
  open until the run has started, so a refusal shows in it; a file the save panel already asked
  about replacing gets `--overwrite`; **Keep owners** shows for PostgreSQL only; the Databases list
  is fetched only while the instance runs (**List databases** wakes it), so the app never wakes an
  instance on its own; and Quit asks while a dump or import runs. Each shows in Activity with its
  bytes and Cancel, as the app's own entries; quitting the app stops them, after asking.
- The app writes nothing of Yard's (app rule): every decision is yardd's, and the only files it
  writes are the dump the user saved.

## 11. RPC and errors

Protocol 2.12 (additive). Every method takes `service?`, `site?` and `path?` (§5).

| Method | Params | Answer |
|---|---|---|
| `db.list` | — | `[DbInstanceList { instance, engine, databases: [DbInfo { name, size_bytes, tables }] }]` |
| `db.create` | `name` | `DbCreated { instance, name, created }` |
| `db.connect` | `wake` | `[ConnectInfo]` (with `wake`, one, after it is up) |
| `db.command` | `action` (`shell`, `dump`, `import`), `name?`, `args`, and for dump `format` hints (`compress`), for import `into_existing`, `keep_owners`, `import_kind` (plain, archive, directory, read by the client from the file's first bytes) | `DbCommand { instance, program, args, env, filter, created }`: the absolute program, its full argument list (Yard's then the user's), the environment to set and clear, which `yard-core` filter the client runs the stream through (`none`, `mysql`, `postgres` with `keep_owners`), and whether the database was created |

`db.command` is where yardd decides: it resolves and wakes the instance, validates the name and the
flags (§7), picks the keg (§7.1), for `import` creates the database when missing and refuses one
with tables, and builds the command. The client runs it and moves the bytes; it builds no path,
port or flag itself.

New error codes: `DB_SERVICE_AMBIGUOUS`, `DB_NO_SERVICE`, `DB_UNSUPPORTED_ENGINE`, `DB_NOT_EMPTY`,
`DB_FLAG_RESERVED`, `DB_DUMP_SWITCHES_DATABASE` and `DB_NO_HANDLER` (client side), `DB_TOOL_FAILED`
(client side, `data.tail`, the tool's last lines; the CLI's own failure, since no yardd job runs),
`FILE_EXISTS` (client side), `SERVICE_STOPPED` and `SERVICE_WAKE_TIMEOUT`. Each hint is the command
that fixes it (rpc-protocol rule). The methods join `methods::ALL` and main spec §6.2.

## 12. Testing

- **Unit (`yard-core`):** instance resolution (§5), name validation, the reserved-flag check
  (every form of every refused flag, MySQL prefixes, and lookalikes that are not refused,
  `--hostname-file`, `--compress` on `mysqldump`), the command built for each tool and format, the
  keg choice (§7.1), import format detection from a file's first bytes, and the statement filter:
  each switching statement in each dialect, the same words inside strings, comments, backticks,
  `DELIMITER` blocks, dollar quotes and `COPY` data (not refused), ownership statements dropped and
  counted, `--keep-owners`, and a statement split across read buffers.
- **Daemon:** `db.list`, `db.create`, `db.connect` and `db.command` against the fake engines;
  `yard-fake-unit` also answers as `mysql` and `psql` for yardd's queries (canned rows by switch
  file), so tests cover a sleeping instance woken, a stopped one refused, the wake timeout,
  `DB_NOT_EMPTY` and the reserved flags.
- **CLI:** against `MockDaemon` answering `db.command` with `yard-fake-unit` as the program (it
  logs its arguments, reads stdin to EOF into a file the test reads, writes a canned dump, and fails
  or hangs on a switch file): the `--` split, `--compress` (gzip on, file names), `.partial` removed
  on failure and Ctrl-C, `FILE_EXISTS`, a switching dump stopped before its statement is sent, the
  failure tail, and `--json`.
- **Real machine:** `scripts/e2e-db.sh`, after `e2e-m1.sh`, on the real `mysql-8.4` and a
  PostgreSQL instance: creates scratch databases named `yard_e2e_<pid>_*`, imports fixtures (plain,
  gzipped, `mysqldump --databases` refused, `pg_dump -Fc` with an owner role the instance lacks,
  plain SQL with `OWNER TO`), dumps one, imports the dump into a second scratch database and compares
  row counts, checks `DB_NOT_EMPTY`, a passed-through flag and a refused one, and waits for the
  instance to sleep and checks `yard db list` wakes it. It drops only the databases it created,
  through the keg client by absolute path, and fails if any other database appeared or vanished.
  It touches no other server: the listeners on 3306, 5432 and 6379 are checked before and after.
  It needs the maintainer's approval (it writes to the real instances).
- **Yard.app:** `checklist.sh` gains the Connect sheet, Create, and a Dump and Import against the
  scratch yardd's instance (the fake tools in `checklist-installs.sh` where a real one is missing).

## 13. Docs that change with it

The M4b spec (§2's decision and §3's non-goal on creating databases, §7.1's command list), the
main spec's §6.2 method list, the M6 app spec's Services section, README (a "Databases" section:
moving a project from DBngin, connecting a GUI), AGENTS.md (the decision, `e2e-db.sh` in Build &
test), the config-ownership rule (Yard now creates databases on request), the rpc-protocol rule
(2.12), and `--help`.

## 14. Left to the builder

The exact Rust types; whether `db.*` lives in `rpc/db.rs` or under `rpc/services`; the wording of
messages and hints within the cli-output rule; the size query per engine; the order of blocks in
`yard db list` and `connect`; how the statement filter is structured, as long as it is pure
`yard-core` and streams; the Yard.app layout within the visual design spec.

## 15. What the builder check changed

The first draft ran dump and import as yardd jobs with progress events and had yardd open the file.
A builder's read found that yardd, as a LaunchAgent, would be refused or prompted for the user's
protected folders, that a long import would block the single job queue, that job progress carries
text lines only, that gzip also needs a crate in the root workspace, that the wake has no timeout
error, and that dumps can switch database and carry another server's owners. The maintainer chose
§6.6's refusal and ownership defaults; the rest are the builder's calls marked above.
