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
- Yard never edits dotfiles. Shell setup is printed for the user to paste
  (`yard install-shims`, M2).
