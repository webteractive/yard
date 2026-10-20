# Yard M3 (HTTPS) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every site can be served over HTTPS with a certificate from Yard's own CA, which the user trusts once in their login keychain, and that Yard renews and re-issues by itself.

**Architecture:** yardd owns the CA and the leaf certificates: reconcile issues, renews and prunes them before it renders nginx, so `nginx -t` always sees the files it names. A secured site gets two server blocks, a redirect on the http port and an `ssl` block on the https port. Trust is a user action, not a root one: `yard trust` runs `/usr/bin/security` as the user, in the user's GUI session, against the login keychain. The root helper does not change in M3.

**Tech Stack:** Rust 2024; `rcgen` 0.14 (ring backend, `x509-parser` feature) and `time` in `yard-daemon`; `sha2` (already in `yard-core`); macOS `/usr/bin/security`.

**Spec:** `docs/superpowers/specs/2026-10-19-yard-design.md` (§3.1, §3.3, §5.4, §5.5, §6.2, §6.3, §10, §12)

## Global Constraints

- CA: `rcgen`, ECDSA P-256, 10-year validity (3650 days), in `ca/yard-ca.pem` and `ca/yard-ca.key`; the key is mode 0600.
- Leaf: 825-day validity, SAN = the site host and `*.<site host>`, in `certs/<site>.pem` and `certs/<site>.key` (key 0600).
- Renewal: a certificate within 30 days of expiry is re-issued by yardd. A TLD change re-issues every secured site's certificate for its new host (§5.5).
- `secure_by_default = true` in `yard.toml` secures every current and future site.
- No `yard takeover` (dropped 2026-10-20). Herd keeps `.test`, 80/443 and `/etc/resolver/test`.
- Tests never bind 53, 80, 443, 8080, 8443 or 53535, never run the real `security` tool, and never touch a keychain.
- Never Herd's PHP; every program Yard runs is an absolute path (`/usr/bin/security`, brew kegs).
- Protocol: additive changes, so `PROTOCOL_VERSION` moves `2.1` → `2.2`.
- Docs move with each task: spec, README, `--help`, AGENTS.md, the hydra rules.

## Decisions (rulings made while planning; the maintainer reviews them)

1. **Trust goes in the login keychain, as the user** (maintainer, 2026-10-20). macOS 11+ needs the user to confirm a trust-settings change in a dialog, which a root LaunchDaemon cannot show, so the spec's `ca.trust`/`ca.untrust` helper verbs are dropped. `yard trust` runs `security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db <ca.pem>`; `yard untrust` runs `security delete-certificate -Z <SHA-256> -t <login keychain>`. Cost if wrong: trust is per user, not per machine.
2. **http redirects to https on the profile's ports**: on `standard`, `http://site.yard:8080/x` answers `301 https://site.yard:8443/x`; on `privileged`, 80 → 443 with no port in the URL. Cost if wrong: one render branch.
3. **`yard secure [site]` / `yard unsecure [site]`**: the site is positional as the spec writes it, and defaults to the site that contains the current directory (as `isolate`). Proxies are named explicitly.
4. **The CA is created on first need** (the first reconcile with a secured site, or `ca.info` from `yard trust`), not at daemon start, so an HTTP-only user never gets a CA.
5. **`secure` is written to `sites.toml` only when it differs from `secure_by_default`**: `yard secure` with the default off writes `secure = true`; with it on, it removes the key. `unsecure` mirrors that.
6. **A failed issue aborts the reconcile** (`INTERNAL`, the previous config keeps serving), as a failed render does.
7. **`yard doctor` prints the Firefox step** (`security.enterprise_roots.enabled` in `about:config`) instead of editing Firefox's bundle; it also prints `NODE_EXTRA_CA_CERTS=<path>`.
8. CA subject `CN = Yard CA (<user>)`, `O = Yard`; leaves have `CN = <host>`, EKU serverAuth, and `not_before` a day back for clock skew. Renewing the CA itself (after 10 years) is out of scope.
9. Plan approval: the maintainer set the goal "implement M3", which this plan takes as approval to execute it natively.

## Review Focus

1. **Certificate files deleted or corrupted by hand** while the site stays secured: the next reconcile re-issues them instead of nginx failing. Pinned by Task 5's `deleted_certificate_is_reissued`.
2. **`ca/` deleted while sites are secured**: a new CA is made and every leaf is re-issued, since the old ones no longer chain. Pinned by Task 5's `a_new_ca_reissues_every_leaf`.
3. **A TLD change**: the SAN follows the new host. Pinned by Task 5's `tld_change_reissues_for_the_new_host`.
4. **`unsecure` with `secure_by_default` on**: the site really goes back to HTTP (`secure = false` written). Pinned by Task 6's `unsecure_overrides_secure_by_default`.
5. **A secured proxy site**: proxies get the same ssl block. Pinned by Task 3's `nginx_site_proxy_secure` snapshot and Task 6's `secure_works_for_a_proxy`.

---

### Task 1: Protocol types, methods and the decision in the docs

**Files:**
- Modify: `crates/yard-proto/src/types.rs`, `crates/yard-proto/src/methods.rs`, `crates/yard-proto/src/error.rs`, `crates/yard-proto/src/lib.rs`, `crates/yard-proto/src/helper.rs` (comment only)
- Modify: every `SiteInfo { .. }` literal (`yard-core/src/sites.rs`, `yard-core/src/render/mod.rs`, `yard-cli/tests/cli.rs`, `yard-proto/src/types.rs`)
- Docs: spec §3.1 (drop `ca.trust`/`ca.untrust` and the paragraph on reading the CA), §5.4 (trust in the login keychain, decision 1 and 2), §6.2 (method list gains `ca.info`), §6.3 (`CA_NOT_TRUSTED`); `.hydra/rules/root-helper.md`; `.hydra/rules/rpc-protocol.md` unchanged.

**Interfaces:**
- Produces: `SiteInfo.secure: bool` (`#[serde(default)]`); `types::CaInfo { cert: String, sha256: String, trusted: bool, expires: String }` (`expires` RFC 3339 date `YYYY-MM-DD`); `methods::SITES_SECURE = "sites.secure"`, `SITES_UNSECURE = "sites.unsecure"`, `CA_INFO = "ca.info"`; `methods::SiteParams { site: String }`; `ErrorCode::CaNotTrusted` (`CA_NOT_TRUSTED`); `PROTOCOL_VERSION = "2.2"`.

- [ ] **Step 1: Write the failing tests** in `yard-proto`: `site_info_secure_defaults_to_false` (deserializing a 2.1 `SiteInfo` without `secure` gives `false`); `ca_info_round_trips`; extend the method-name test with the three names and the error-code test with `CA_NOT_TRUSTED`.
- [ ] **Step 2: Run** `cargo test -p yard-proto` — Expected: FAIL to compile (no `secure`, `CaInfo`, consts).
- [ ] **Step 3: Implement** the types and consts; set `secure: false` in every existing literal; bump the protocol. Update the CLI test that expects `"2.1"`.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Docs** as listed; `hydra sync && hydra doctor` PASS.
- [ ] **Step 6: Commit** `https types: SiteInfo.secure, ca.info and sites.secure, protocol 2.2`

### Task 2: Config, paths and the renewal decision (yard-core)

**Files:**
- Modify: `crates/yard-core/src/config/sites.rs` (`SiteEntry.secure: Option<bool>`), `crates/yard-core/src/config/yard.rs` (`YardConfig.secure_by_default: bool`, key `secure_by_default`), `crates/yard-core/src/paths.rs`, `crates/yard-core/src/sites.rs`
- Create: `crates/yard-core/src/tls.rs` (pure: no crypto, no clock)

**Interfaces:**
- Produces: `Paths::ca_dir()`, `ca_cert()` (`ca/yard-ca.pem`), `ca_key()`, `certs_dir()`, `cert(name)` (`certs/<name>.pem`), `cert_key(name)` (`certs/<name>.key`).
- `sites::resolve(cfg, sites, tld, http_port, https_port, is_dir)`: `secure = entry.secure.unwrap_or(cfg.secure_by_default)`; a secure site's `url` is `https://<host>/` on 443, else `https://<host>:<https_port>/`.
- `tls::{CA_DAYS = 3650, LEAF_DAYS = 825, RENEW_BEFORE_DAYS = 30}`; `tls::CertFacts { dns_names: Vec<String>, not_after: i64 /* unix s */, signed_by_ca: bool }`; `tls::needs_issue(facts: Option<&CertFacts>, host: &str, now: i64) -> bool` — true when there is no cert, it is not signed by the current CA, its names are not exactly `{host, *.host}`, or `not_after - now < 30 days`.

- [ ] **Step 1: Write the failing tests**: `secure_key_is_parsed` and `secure_by_default_is_parsed` (config); `secure_site_has_an_https_url` (8443 → `https://blog.yard:8443/`, 443 → `https://blog.yard/`); `secure_by_default_secures_unless_the_site_says_no`; `needs_issue` table test: none → true; fresh and matching → false; 29 days left → true; 31 → false; SAN for `blog.test` while host is `blog.yard` → true; `signed_by_ca: false` → true.
- [ ] **Step 2: Run** `cargo test -p yard-core` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; update `resolve`'s callers (reconcile, doctor, tests) to pass the https port from the registry (`NGINX_HTTPS`, falling back to `profile.https_port()`).
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `secure and secure_by_default in the config, https urls, and when a cert needs issuing`

### Task 3: nginx renders secured sites (yard-core)

**Files:**
- Modify: `crates/yard-core/src/render/mod.rs` (`RenderCtx.https_port: u16`, tests), `crates/yard-core/src/render/nginx.rs`

**Interfaces:**
- Consumes: `SiteInfo.secure`, `Paths::cert`, `Paths::cert_key`.
- Produces: `nginx_site` renders, for a secure site, a redirect block on the http port (`return 301 https://$host<:https unless 443>$request_uri;`, same `server_name`) and the existing body in a block with `listen 127.0.0.1:<https> ssl;`, `listen [::1]:<https> ssl;`, `ssl_certificate <cert>;`, `ssl_certificate_key <key>;`, `ssl_protocols TLSv1.2 TLSv1.3;`, and the override include. An insecure site renders byte-for-byte as before.

- [ ] **Step 1: Write the failing tests**: snapshots `nginx_site_php_secure` and `nginx_site_proxy_secure` (8443) and `nginx_site_secure_on_443_redirects_without_a_port` (asserts the line `return 301 https://$host$request_uri;`); extend `renders_paths_with_spaces_quoted` with a secure site (cert paths are under `Application Support`).
- [ ] **Step 2: Run** `cargo test -p yard-core render` — Expected: FAIL (snapshots missing / assertion).
- [ ] **Step 3: Implement**; review the new snapshots against §5.4 before `cargo insta accept`.
- [ ] **Step 4: Run** `cargo test -p yard-core` — Expected: PASS, existing snapshots unchanged.
- [ ] **Step 5: Commit** `nginx serves secured sites over ssl and redirects their http`

### Task 4: The CA and leaf certificates (yard-daemon `tls.rs`)

**Files:**
- Modify: `crates/yard-daemon/Cargo.toml` (`rcgen = { version = "0.14.10", features = ["x509-parser"] }`, `time = "0.3"`, `sha2`), `crates/yard-daemon/src/lib.rs`
- Create: `crates/yard-daemon/src/tls.rs`, `crates/yard-daemon/tests/tls.rs`

**Interfaces:**
- Produces:
  - `tls::ensure_ca(paths: &Paths, user: &str, now: OffsetDateTime) -> Result<bool, YardError>` — creates `ca/` (0700) and the CA when either file is missing or does not parse; `Ok(true)` when it made one.
  - `tls::issue(paths: &Paths, site: &str, host: &str, now: OffsetDateTime, days: i64) -> Result<(), YardError>` — a fresh P-256 key and a leaf signed by the stored CA.
  - `tls::facts(paths: &Paths, site: &str) -> Option<yard_core::tls::CertFacts>` — `None` when the files are missing or unreadable; `signed_by_ca` checks the leaf's signature against `ca/yard-ca.pem`.
  - `tls::ca_sha256(paths) -> Result<String, YardError>` (upper-case hex of the DER) and `tls::ca_expires(paths) -> Result<String, YardError>`.
  - Keys are written through `write_private(path, text)`: created 0600 (never briefly wider), then renamed into place.
- [ ] **Step 1: Vet and add the crates** (global rust-dependencies rule: repository `github.com/rustls/rcgen`, ring backend); `cargo deny check` clean, `cargo audit` clean.
- [ ] **Step 2: Write the failing tests** in `tests/tls.rs`: `ensure_ca_makes_a_ca_once` (second call `Ok(false)`, files unchanged; key mode 0600; dir 0700; cert parses as a CA with `CN = Yard CA (me)`); `issued_leaf_names_the_host_and_its_subdomains` (`facts` gives `["blog.yard", "*.blog.yard"]`, `signed_by_ca`, `not_after` ≈ now + 825 days); `a_leaf_from_another_ca_is_not_signed_by_ca` (issue, then replace the CA → `signed_by_ca: false`); `facts_of_a_corrupt_cert_is_none`.
- [ ] **Step 3: Run** `cargo test -p yard-daemon --test tls` — Expected: FAIL to compile.
- [ ] **Step 4: Implement** with rcgen 0.14 (`CertificateParams`, `KeyPair::generate()`, `Issuer::from_ca_cert_pem`, `signed_by`), reading certificates back with `x509-parser` (re-exported by rcgen's feature) and its `verify_signature`.
- [ ] **Step 5: Run** the test file — Expected: PASS.
- [ ] **Step 6: Commit** `yardd makes a local CA and issues site certificates`

### Task 5: Reconcile issues, renews and prunes certificates

**Files:**
- Modify: `crates/yard-daemon/src/reconcile.rs`, `crates/yard-daemon/src/watch.rs`, `crates/yard-daemon/src/bin/yard-fake-unit.rs` (fake `nginx -t` fails like real nginx when an `ssl_certificate`/`ssl_certificate_key` file is missing)
- Test: `crates/yard-daemon/tests/reconcile.rs`

**Interfaces:**
- Consumes: Task 2's `needs_issue`, Task 4's `ensure_ca`/`issue`/`facts`.
- Produces: a new step between resolve and render: when any site is secure, `ensure_ca`; every secure site with `needs_issue(facts, host, now)` is issued; then every `certs/<name>.{pem,key}` whose site is not secure is removed. `Reconciler::certs_due(&self) -> bool` (any secured site whose cert `needs_issue` now), and the watch tick reconciles when `docroots_stale() || certs_due()`.
- [ ] **Step 1: Write the failing tests**: `securing_a_site_writes_its_certificate_and_ssl_block` (nginx test passes, `certs/blog.pem` exists, the site conf has `ssl_certificate`); `unsecuring_removes_the_certificate`; `deleted_certificate_is_reissued`; `a_new_ca_reissues_every_leaf`; `tld_change_reissues_for_the_new_host` (tld `yard2`, SAN `blog.yard2`); `an_expiring_certificate_is_renewed` (issue with `days = 20`, then `certs_due()` is true and a reconcile gives `not_after` ≈ now + 825 days); `no_secure_site_makes_no_ca`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test reconcile` — Expected: the new tests FAIL.
- [ ] **Step 3: Implement**; `RenderCtx.https_port` from the registry; the CA user is `$USER` (fallback `user`).
- [ ] **Step 4: Run** `cargo test -p yard-daemon` — Expected: PASS.
- [ ] **Step 5: Commit** `reconcile issues, renews and removes site certificates`

### Task 6: `sites.secure`, `sites.unsecure` and `ca.info`

**Files:**
- Modify: `crates/yard-daemon/src/rpc/handlers.rs`, `crates/yard-daemon/src/daemon.rs` (`DaemonOptions.security: PathBuf`, production `/usr/bin/security`), `crates/yard-daemon/src/reconcile.rs` (`Reconciler::ca_info`)
- Modify: fake unit (`security` argv[0] mode: `verify-cert` exits 0 when `<its dir>/fake-trusted` exists, else 1; every call appended to `<its dir>/security.log`), test support (`Fixture.security`)
- Test: `crates/yard-daemon/tests/rpc.rs`

**Interfaces:**
- `sites.secure {site}` / `sites.unsecure {site}` → `SiteInfo` (decision 5 for the TOML; `SITE_NOT_FOUND` for an unknown site).
- `ca.info` → `CaInfo`: runs `ensure_ca` under the reconcile lock; `trusted` is `security verify-cert -c <ca.pem> -l -L -q -p basic` exiting 0.
- [ ] **Step 1: Write the failing tests**: `secure_then_unsecure_round_trips_sites_toml` (the file is byte-identical afterwards); `unsecure_overrides_secure_by_default` (writes `secure = false`, the url goes back to http); `secure_works_for_a_proxy`; `secure_an_unknown_site_is_site_not_found`; `ca_info_reports_trust_from_security` (untrusted, then `fake-trusted` created → trusted; the log shows `verify-cert -c <root>/ca/yard-ca.pem -l -L -q -p basic`).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test rpc` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p yard-daemon` — Expected: PASS.
- [ ] **Step 5: Commit** `sites.secure, sites.unsecure and ca.info`

### Task 7: Doctor's CA check

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs` (`DoctorInput.security: &Path`), its callers (rpc handler, CLI offline doctor), `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- A `ca` check: no secured site and no CA → ok `no site uses HTTPS`; CA trusted → ok `the Yard CA is trusted in your login keychain; for Node: NODE_EXTRA_CA_CERTS=<ca.pem>` (plus, when `/Applications/Firefox.app` exists, `; Firefox: set security.enterprise_roots.enabled to true in about:config`); a secured site with the CA untrusted or missing → warn `browsers will warn on https sites: the Yard CA is not trusted`, hint `yard trust`.
- [ ] **Step 1: Write the failing tests**: `ca_check_ok_without_https`, `ca_check_warns_when_untrusted`, `ca_check_names_node_extra_ca_certs_when_trusted`.
- [ ] **Step 2: Run** — Expected: FAIL. **Step 3: Implement.** **Step 4: Run** `cargo test -p yard-daemon` — PASS.
- [ ] **Step 5: Commit** `doctor checks that the Yard CA is trusted`

### Task 8: CLI `secure`, `unsecure`, `trust`, `untrust`

**Files:**
- Modify: `crates/yard-cli/src/cli.rs`, `crates/yard-cli/src/commands/{mod.rs,sites.rs}`; Create: `crates/yard-cli/src/commands/trust.rs`; Test: `crates/yard-cli/tests/cli.rs`; Docs: README command table and HTTPS section.

**Interfaces:**
- `yard secure [site]` → `Secured blog → https://blog.yard:8443/`, then, when `ca.info` says untrusted, `The Yard CA is not trusted yet, so browsers will warn. Run yard trust (macOS asks for your password once).` `yard unsecure [site]` → `blog is back on http://blog.yard:8080/`.
- `yard trust`: `ca.info`; already trusted → `The Yard CA is already trusted.`; else runs `<security> add-trusted-cert -r trustRoot -k $HOME/Library/Keychains/login.keychain-db <cert>` with the terminal's stdio, then `ca.info` again: trusted → `Trusted the Yard CA in your login keychain.`; otherwise `CA_NOT_TRUSTED` (`security exited with N` / `the CA is still not trusted`, hint `yard trust`). `yard untrust`: untrusted → `The Yard CA is not in your login keychain.`; else `<security> delete-certificate -Z <sha256> -t <login keychain>`.
- `<security>` is `/usr/bin/security`, or `YARD_SECURITY` when set to an absolute path (the test seam, like `YARD_BREW_PREFIX`).
- [ ] **Step 1: Write the failing tests** against `MockDaemon` and a recording shell script as `YARD_SECURITY`: `secure_prints_the_https_url_and_the_trust_hint`; `secure_defaults_to_the_site_containing_the_cwd`; `trust_runs_add_trusted_cert_on_the_login_keychain`; `trust_when_trusted_runs_nothing`; `trust_that_does_not_take_is_ca_not_trusted`; `untrust_deletes_by_sha256`; `--json` for each prints the proto type.
- [ ] **Step 2: Run** `cargo test -p yard-cli` — Expected: FAIL. **Step 3: Implement**, with `--help` text. **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `yard secure, unsecure, trust and untrust`

### Task 9: End to end on this machine

**Files:**
- Create: `scripts/e2e-m3.sh`; Docs: AGENTS.md (status, build & test), README, spec §10 and §12 (M3 done).

- [ ] **Step 1: Write `scripts/e2e-m3.sh`** in the style of `e2e-m1.sh` (backs up and restores the TOMLs, never touches Herd): link a scratch PHP app as `hello-tls`, `yard secure hello-tls`; `curl --cacert <root>/ca/yard-ca.pem https://hello-tls.yard:8443/` prints the PHP version; `curl -sI http://hello-tls.yard:8080/x` is `301` with `Location: https://hello-tls.yard:8443/x`; `certs/hello-tls.key` is mode 600; `yard unsecure` serves http again and removes the cert files; `yard doctor` has no failing check; `/etc/resolver/test` unchanged; unlink and clean up.
- [ ] **Step 2: Run** `cargo build --release`, restart yardd, run `scripts/e2e-m1.sh`, `scripts/e2e-m2.sh` and `scripts/e2e-m3.sh` — Expected: every line `ok`, TOMLs restored.
- [ ] **Step 3: Docs**, then **commit** `e2e-m3 checks https end to end; M3 is done`. `yard trust` itself needs the maintainer at the screen (macOS's dialog), so it is reported, not run.
