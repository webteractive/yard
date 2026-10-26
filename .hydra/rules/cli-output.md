---
paths:
  - "crates/yard-cli/**"
triggers:
  - adding or changing a yard CLI command or its output
---

# yard CLI output

- Every command has real `--help` text and supports `--json`. JSON prints the
  `yard-proto` result type unchanged, or `{"error": YardError}`. Human output is aligned
  tables or text, never JSON.
- Human list output ends with `For agents and scripts: <command> --json`. An empty list
  says what to do next (`No sites yet. Serve a project with yard link, run inside it.`).
- Exit codes are part of the interface: 0 success, 1 Yard error (stderr
  `error: <message>` then `  hint: <hint>`), 2 usage, 3 `yardd is not running → yard start`
  (`DAEMON_NOT_RUNNING` under `--json`). `doctor` and `ports check` also exit 1 when a
  check fails, after printing their report.
- A command that shows yardd's last good state while a TOML on disk does not parse says
  so on stderr (`yard ports`: `warning: <file> does not parse, so this is the last good
  registry: …`). `ports check` reads `ports.toml` itself and fails on it instead.
- Tests run `yard --root <tmp>` against `tests/support::MockDaemon`, never a real yardd,
  and never `yard start`/`stop` or `helper install` (they change launchd).
- `--version` is ours, not clap's (`disable_version_flag`), because it asks yardd too.
  `--version`, `-V` and `yard version [--json]` all report CLI, daemon, helper and
  protocol versions plus the commit and build date. The commit is injected by
  `crates/yard-cli/build.rs`, never typed in.
- Profiles are `yard ports profile <standard|privileged>`; M1's `trial` and `default`
  stay accepted as aliases (`ProfileArg`), and output always uses the new names.
- Offline commands (no daemon needed): `start`, `stop`, `version`, `doctor`,
  `ports check`, `install-shims`, `helper install|uninstall`. Everything else talks to `yardd`.
- A command that would install something asks first (`yard init`, and `yard link` for a
  project whose `.yard` needs a PHP, extension or service, or whose pinned Node is missing): it lists each need as
  `  <what>  (<command>)` and asks `Install these? [y/N] ` only when stdin and stdout are
  terminals and `--json` is off. `--yes` skips the question. Otherwise it installs
  nothing and fails with `NEEDS_INSTALL`, hint `yard init --yes`, and the needs in
  `data.needs`. Shared code: `commands/init.rs` `settle`. `yard init` asks the same way
  before moving a site onto a directory with its own `.yard` (`Replace it with this
  directory? [y/N] `, `--replace`, else `SITE_NAME_TAKEN`); it never asks when there
  is no `.yard` yet, since codifying the running site is what init is for.
- A missing Node is offered by `settle` as `  Node <spec>  (yard node install <spec>)` and
  installed last, but it is never a `Need`: when it is the only thing missing and nobody
  agrees, `settle` prints `<site> runs Node <spec>, which is not installed: yard node install
  <spec>` on stderr and still serves, with no `NEEDS_INSTALL`; a Node install that fails is a
  `warning:` with its hint, and the command still exits 0. Every printed `yard node install`
  goes through `node::install_hint`, which quotes `'lts/*'` so zsh does not glob it.
- `yard init` without `--local` refuses a yardd older than protocol 2.6, which would write
  `.yard` into the project (`PROTOCOL_MISMATCH`, hint `yard stop && yard start`).
- A `METHOD_NOT_FOUND` from a yardd whose protocol is older than the CLI's says
  `yardd speaks protocol <p> and has no <method>`, hint `yard stop && yard start`: it was not
  restarted after an upgrade (`Client::stale`).
- `yard init` writes the managed `.yard` (Yard's data root) unless `--local`; it prints
  `Wrote <file>:`, `Moved <managed> to <project file>:` for a `--local` move, and
  `Kept the old file as <backup>.` after a `--replace` over another project's managed file.
  `yard link` ends with `Using <file>` when it adopts a managed file, or
  `<file> is for <project>, so it is not used here.` `isolate`, `unisolate`, `secure`,
  `unsecure`, `docroot` and `node use` end their line with ` (set in <file>)` when they edited a managed
  file. `yard info` shows `config  managed (<~ path>)` and `from managed` for its settings.
- Yard never edits dotfiles. Shell setup is printed for the user to paste
  (`yard install-shims`, M2). It warns on stderr when the first `php` or `node` on PATH is
  not Yard's, naming whose it is (`shims::whose`: Herd's, nvm's, mise's, fnm's, brew's).
- A command that asks about **trust** goes through `commands/sources.rs` `with_trust`: when
  yardd answers an install with `FORMULA_UNTRUSTED` and stdin and stdout are terminals
  (and `--json` is off), it asks `Trust <formula> (from the <tap> tap)? [y/N] `, and yes
  runs the `sources.trust` job and repeats the call. `--yes` never answers it, and neither
  does `init` or `link`: with no terminal it is the error, hint `yard source trust <formula>`.
  `php install`, `ext add`, `service add` and `settle` all call through it.
- A command that deletes data (`yard service remove`) first checks what the daemon would
  refuse (unknown instance, or one a `.yard` lists: `SERVICE_NOT_FOUND`, `SERVICE_IN_USE`) and
  fails with that before asking anything, with or without `--yes`. Then it asks
  `Delete <i> and its data? [y/N] ` (`--yes` skips). With no terminal and no `--yes` it deletes nothing and fails with
  `NEEDS_CONFIRMATION`; the hint is the exact command with `--yes`, keeping `--keep-data`.
- A command whose method answers before the work is done (`yard service start`, `restart`,
  `port`) polls `services.list` until the instance is `running` (130 s, `READY_WAIT`, longer than any start timeout). A `failed` one is
  `ServiceState::failure` (yard-proto, shared with the daemon's add job): its reason first, then
  its log's last line, with the reason's code and hint (`PORT_IN_USE` naming the holder, hint
  `yard service port <i> <port>`; else `JOB_FAILED`, hint `yard service logs <i>`). Its last log
  lines go to stderr first, except for a held port or a missing keg, where the process never ran.
  `yard services` and `yard info` print `<i> failed: <reason>  (<hint>)` under their tables.
  `start` and `restart` succeed only on `running`; `port` also on `stopped` by the user. A
  `stopped` the user did not set means yardd runs nothing for it, and is an error.
- A command that follows a stream (`yard service logs -f`) subscribes before it fetches the
  tail, registers its SIGINT handler before printing, and exits 0 on Ctrl-C.
- A command that waits on a job (`run_job`) makes one SIGINT stream (`signal(SignalKind::interrupt())`)
  before it prints the first line and keeps it for the whole wait. Never a `ctrl_c()` inside the
  `select!` loop: it registers nothing until polled, so a Ctrl-C between two turns kills the
  command or is lost. CLI tests that wait on a child carry a 60 s watchdog (`yard_at`'s timeout,
  `watchdog` in `tests/cli.rs`), so a hang fails the test instead of stalling the suite.
- Sleeping services (M4e): `yard services` has a SLEEP column after STATE (the effective
  `sleep_after`, `15m` or `off`), and STATE reads `sleeping <age>` (`45s`, `12m`, `2h`, `3d`, the
  largest whole unit) for a sleeping instance and `waking` while it starts again (`state_text`,
  shared with `yard info`, which adds `wakes on the next request, or: yard service start <i>`
  under its table). `yard status` gives each sleeping instance a row (`sleeping`, PID and
  RESTARTS `-`). `yard service sleep-after <i> <duration|off|default>` prints
  `<i> now sleeps after <d> without use`, `<i> never sleeps` or `<i> now follows yard.toml (<d>)`.
- Output meant to be pasted (`yard env`) has no header or footer lines (no `For agents and
  scripts` footer), only a blank line between blocks: notes are `# …` comments, and what to do about an empty result
  goes to stderr. A request the daemon resolves against a directory (`env`, `info`) sends
  the canonicalized absolute path, since yardd's working directory is not the CLI's.
