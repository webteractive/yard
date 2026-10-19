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
  says what to do next (`No sites yet. Park a directory with yard park <path> …`).
- Exit codes are part of the interface: 0 success, 1 Yard error (stderr
  `error: <message>` then `  hint: <hint>`), 2 usage, 3 `yardd is not running → yard start`
  (`DAEMON_NOT_RUNNING` under `--json`). `doctor` and `ports check` also exit 1 when a
  check fails, after printing their report.
- Tests run `yard --root <tmp>` against `tests/support::MockDaemon`, never a real yardd,
  and never `yard start`/`stop` or `helper install` (they change launchd).
- `--version` is ours, not clap's (`disable_version_flag`), because it asks yardd too.
  `--version`, `-V` and `yard version [--json]` all report CLI, daemon, helper and
  protocol versions plus the commit and build date. The commit is injected by
  `crates/yard-cli/build.rs`, never typed in.
- Offline commands (no daemon needed): `start`, `stop`, `version`, `doctor`,
  `ports check`, `helper install|uninstall`. Everything else talks to `yardd`.
- Yard never edits dotfiles. Shell setup is printed for the user to paste
  (`yard install-shims`, M2).
