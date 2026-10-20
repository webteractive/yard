---
paths:
  - "scripts/e2e*"
  - "crates/yard-core/src/ports/**"
triggers:
  - running yard against the real data root on this machine
  - binding ports or touching /etc/resolver
  - switching port profiles or the TLD
  - reading Herd's configuration
---

# Herd keeps working

- Herd is installed and serving ~30 sites. Its nginx holds 127.0.0.1:80/443 and its
  resolver is `/etc/resolver/test`. Nothing Yard does may break that:
  there is no `yard takeover` (dropped 2026-10-20).
- The default profile is **standard**: nginx-http 8080, nginx-https 8443, dnsmasq
  53535, and the TLD stays `yard` (profiles never set it). Do not change the default,
  and do not run `yard ports profile privileged` on this machine except to check that
  it is refused with Herd named as the holder (Herd answers on 80).
- The TLD is refused separately: a reconcile that moves to a TLD whose
  `/etc/resolver/<tld>` lacks Yard's marker fails with `RESOLVER_FOREIGN` and keeps
  serving the old TLD, so setting `tld = "test"` while Herd's file is there changes nothing.
- Yard **only reads** Herd's files (`~/Library/Application Support/Herd/`, mainly
  `config/valet/`). Never write, move or delete anything there, and never quit Herd
  or its services. Stopping or removing Herd is
  only ever the user's own step.
- Tests never bind 53, 80, 443, 8080, 8443 or 53535. They ask the OS for a free port.
- A real-machine run (`scripts/e2e-m1.sh`) checksums `/etc/resolver/test` before and
  after, and checks that `http://<site>.test/` is still answered by Herd.
