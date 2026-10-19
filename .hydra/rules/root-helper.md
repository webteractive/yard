---
paths:
  - "crates/yard-helper/**"
  - "crates/yard-daemon/src/helper_client.rs"
commands:
  - "sudo "
  - "launchctl"
triggers:
  - adding or changing a privileged operation
  - installing or uninstalling the root helper
---

# Root helper boundary

- The helper accepts exactly these verbs: `nginx.start`, `nginx.stop`, `nginx.reload`,
  `nginx.test_config`, `resolver.install`, `resolver.remove`, `ca.trust`, `ca.untrust`
  (M3), and the read-only `status`. A new verb needs a spec change first (§3.1).
- **No caller-supplied paths or commands, ever.** The owning uid and user, the data
  root and the brew prefix are fixed at install time as plist arguments. The only
  caller arguments are a TLD (`^[a-z][a-z0-9-]{0,62}$`, `yard_core::config::yard::valid_tld`)
  and a port (1024–65535).
- **The data root must be safe for root to read.** `root_dir::check` requires an
  absolute root and resolves it one component at a time, as written (nginx gets that
  path, not the canonical one): every directory it passes must be owned by the owner
  or root and not group- or other-writable (sticky parents like `/tmp` aside), and
  every symlink it follows must be owned by the owner or root, with its target walked
  the same way. Checking only the canonical path misses a symlink sitting in someone
  else's directory. `yard helper install` checks it, and the helper re-checks before
  every verb that makes nginx read the config: `nginx.start`, `nginx.reload` and
  `nginx.test_config`. `nginx.stop` is not gated, so nginx can always be stopped.
- Socket: `/var/run/dev.more.yard.helper.sock`, mode 0666, and every connection is
  checked with `getpeereid`. Only the owning uid (or root) is served. It is not in the
  data root, so root never creates files in a directory the user can rewrite.
- **Verify before signalling.** The nginx pid file is in the user-writable `run/`. Before
  any signal, check that `pidpath(pid)` equals the canonical nginx binary path, and
  refuse otherwise. Without that check the pid file could get root to signal any process.
- Resolver files carry `# Managed by Yard` on line 1. Never overwrite or delete a
  resolver file without that marker: `/etc/resolver/test` on this machine is Herd's.
- Threat model: the helper protects against *other* local users and malformed input.
  It does not protect against the owning user, who is an admin and can already write
  the nginx config and the brew prefix that root executes. Do not add complexity
  pretending otherwise; do not remove the checks above either.
- Install path (M1–M3): `sudo yard helper install` copies the binary to
  `/Library/PrivilegedHelperTools/dev.more.yard.helper` and writes
  `/Library/LaunchDaemons/dev.more.yard.helper.plist`, both root:wheel.
  `uninstall` stops nginx through the helper first: nginx outlives the helper's job. From M4 Yard.app
  registers the same helper with `SMAppService`.
- Agents cannot type a sudo password. Ask the maintainer to run privileged commands with
  sudo in a real terminal (the `!` prefix has no tty for the password). Tests run the helper in-process as the current user, with a temp
  `resolver_dir` and a fake nginx. `doctor` only reads the resolver file, from
  `DaemonOptions.resolver_dir` (`RESOLVER_DIR` in production).
