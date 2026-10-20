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

- The helper accepts exactly these verbs: `resolver.install`, `resolver.remove`,
  `ports.bind`, `ca.trust`, `ca.untrust` (M3), and the read-only `status`. A new verb
  needs a spec change first (§3.1). **Root never runs nginx** and never reads its
  config: nginx is a yardd unit, run as the user (M1.1).
- **No caller-supplied paths or commands, ever.** The owning uid and user, the data
  root and the brew prefix are fixed at install time as plist arguments. The only
  caller arguments are a TLD (`^[a-z][a-z0-9-]{0,62}$`, `yard_core::config::yard::valid_tld`)
  and a port: 1024–65535 for `resolver.install`, and for `ports.bind` only
  `HelperConfig.privileged_ports` (always 80 and 443 outside tests).
- **`ports.bind`** binds the port on `127.0.0.1` and `[::1]` (v6-only, `SO_REUSEADDR`,
  backlog 511) and sends the two listening sockets with its answer as `SCM_RIGHTS`
  (`yard_helper::fdpass`; `BindResult.addresses` names them in order). yardd reads them
  with `HelperClient::bind_port`, marks each close-on-exec as it arrives (macOS has no
  `MSG_CMSG_CLOEXEC`), and holds them in `PrivilegedSockets`. A held port is
  `PORT_IN_USE` with `data.port`.
- **The data root must be safe for root to read.** `root_dir::check` requires an
  absolute root and resolves it one component at a time, as written (nginx gets that
  path, not the canonical one): every directory it passes must be owned by the owner
  or root and not group- or other-writable (sticky parents like `/tmp` aside), and
  every symlink it follows must be owned by the owner or root, with its target walked
  the same way. Checking only the canonical path misses a symlink sitting in someone
  else's directory. `yard helper install` checks it, and `ca.trust` (M3) will re-check
  it before reading the CA from the root.
- Socket: `/var/run/dev.more.yard.helper.sock`, mode 0666, and every connection is
  checked with `getpeereid`. Only the owning uid (or root) is served. It is not in the
  data root, so root never creates files in a directory the user can rewrite.
- **Root never acts on a path in the data root by name.** The owner can make any name
  there a hard link or symlink to a root file. Upgrading from M1, `helper install`
  sends the old helper `nginx.stop` (`LEGACY_NGINX_STOP`, ignoring `METHOD_NOT_FOUND`)
  and then *removes* the two files M1's root nginx left (`legacy_root_files`):
  unlinking never touches what a link points at, where a chown would follow it.
- Resolver files carry `# Managed by Yard` on line 1. Never overwrite or delete a
  resolver file without that marker: `/etc/resolver/test` on this machine is Herd's.
- Threat model: the helper protects against *other* local users and malformed input.
  It does not protect against the owning user, who is an admin and can already write
  the nginx config and the brew prefix that root executes. Do not add complexity
  pretending otherwise; do not remove the checks above either.
- Install path (M1–M3): `sudo yard helper install` copies the binary to
  `/Library/PrivilegedHelperTools/dev.more.yard.helper` and writes
  `/Library/LaunchDaemons/dev.more.yard.helper.plist`, both root:wheel.
  `uninstall` removes the plist, the binary and Yard's resolver files. From M4 Yard.app
  registers the same helper with `SMAppService`.
- Agents cannot type a sudo password. Ask the maintainer to run privileged commands with
  sudo in a real terminal (the `!` prefix has no tty for the password). Tests run the helper in-process as the current user, with a temp
  `resolver_dir` and free ports standing in for 80 and 443
  (`support::spawn_helper(fx, resolver_dir, privileged_ports)`). `doctor` only reads the resolver file, from
  `DaemonOptions.resolver_dir` (`RESOLVER_DIR` in production).
