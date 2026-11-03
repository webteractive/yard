# M5 Tunnels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `yard tunnel` shares a site Yard serves at a public HTTPS URL through brew's cloudflared,
as a quick tunnel (trycloudflare.com, no account) or a named tunnel on a domain set up with
`yard tunnels domain add`, supervised by yardd like every other unit.

**Architecture:** Each tunnel is a yardd unit `tunnel-<site>` running cloudflared from a generated
`tunnels/<site>.yml`, whose origin is a dedicated loopback nginx listener appended to the site's own
nginx conf. Named tunnels are intent in `sites.toml` plus facts in `state/tunnels.json`; quick
tunnels live in yardd's memory. Cloudflare management (login, create, route dns, delete) runs in
jobs; a monitor polls each tunnel's metrics port for readiness and a quick tunnel's hostname.

**Tech Stack:** Rust workspace (yard-proto, yard-core, yard-daemon, yard-cli), tokio, toml/toml_edit,
serde_json; brew's cloudflared 2026.6.1; the `yard-fake-unit` fixture as a fake cloudflared.

**Spec:** `docs/superpowers/specs/2026-10-21-yard-tunnels-design.md` (approved 2026-10-22). Main
spec: `docs/superpowers/specs/2026-10-19-yard-design.md`.

## Facts checked while planning (2026-11-02, cloudflared 2026.6.1, the maintainer approved the run)

A quick tunnel was run from a scratch config to a scratch loopback echo server, with `HOME` in the
scratchpad. These settle the spec's "the plan checks" items:

- A quick tunnel starts from `url:` in a `--config` file (`cloudflared tunnel --config <yml>`), no
  `--url` needed. `metrics:`, `no-autoupdate:`, `loglevel:` and `grace-period:` are read from it.
- The metrics server starts only once the quick tunnel has its hostname (about 4 s, network
  needed). `GET /quicktunnel` answers `{"hostname":"<words>.trycloudflare.com"}`; `GET /ready`
  answers 200 with `{"status":200,"readyConnections":1,…}` once connected (6 s here).
- At the origin: `Host` is the public hostname, `X-Forwarded-Proto` is `https` (a client-sent value
  is replaced), `CF-Connecting-IP` is the visitor, and `X-Forwarded-For` is the client-sent value
  **with the visitor appended** (`<forged>, <visitor>`), so it cannot be trusted as sent.
- With `grace-period: 1s` and nothing in flight, SIGTERM stopped it in 0.1 s. Its default
  `grace-period` is 30 s and `--retries` 5.
- It wrote nothing under `HOME`. It opens an ICMP proxy socket (no TCP or UDP listener).
- `cloudflared tunnel login` has no option for where the certificate goes: it writes
  `$HOME/.cloudflared/cert.pem`. `--origincert <path>` (`TUNNEL_ORIGIN_CERT`) is a `tunnel`-level
  flag the management commands read. `tunnel create` takes `--credentials-file`, `tunnel list`
  `--name` and `-o json`, `route dns` `-f`/`--overwrite-dns`.
- `~/.cloudflared` does not exist on this machine (never logged in).

## Rulings made while planning

Each changes or fills a spec builder's call; the last task writes them into the spec.

- **P1. Unit health is `Health::Tcp(metrics_port)`, not `Health::Http(/ready)`.** The supervisor
  stops a running unit after two failed health checks. With `/ready` as health, a Wi-Fi drop would
  restart cloudflared, and a quick tunnel restarted gets a new URL, so the shared link dies;
  cloudflared reconnects to the edge by itself. Readiness (`connecting`/`ready`) is read from
  `/ready` by the tunnel monitor instead. Cost if wrong: a wedged cloudflared that keeps its metrics
  port open but never reconnects stays `connecting` until `yard restart tunnel-<site>`; doctor warns
  meanwhile.
- **P2. Tunnel units get `start_timeout: Some(30 s)`.** A quick tunnel's metrics port opens only
  after it has a hostname, which needs the network.
- **P3. Every generated yml sets `grace-period: 3s`**, under Yard's 5 s stop timeout.
- **P4. The tunnel listener's proxy branch sends `X-Forwarded-For $remote_addr`** (the visitor,
  after `real_ip_header CF-Connecting-IP`), `X-Forwarded-Proto https` and `X-Forwarded-Host $host`,
  not `$proxy_add_x_forwarded_for`, which would pass on a visitor's forged list.
- **P5. One certificate per domain, always.** `tunnels.domain_add` runs `cloudflared tunnel login`
  with `HOME=<home>/.cloudflared/yard/<domain>`, so cloudflared itself writes
  `<home>/.cloudflared/yard/<domain>/.cloudflared/cert.pem`; management commands pass
  `--origincert` with that path. Yard checks it exists and never reads, copies or moves it. This
  works whether or not one certificate covers several zones (which needs the maintainer's account to check),
  and never touches a `~/.cloudflared/cert.pem` the user made for something else.
- **P6. `DOMAIN` is required** in `yard tunnels domain add DOMAIN`. Yard cannot learn which zone was
  picked in the browser without reading the certificate, so it tells the user which one to pick
  (`In the browser, pick the zone <domain>.`) and records that. Cost if wrong: a wrong pick shows up
  at the first `yard tunnel --host` as `TUNNEL_DNS_FAILED`.
- **P7. A created tunnel's credentials go to `<home>/.cloudflared/yard/<domain>/<tunnel-name>.json`**
  (`tunnel create --credentials-file`). An adopted one (`--tunnel`) uses cloudflared's own default,
  `<home>/.cloudflared/<uuid>.json`. Either path is recorded in `state/tunnels.json` as
  `credentials`.
- **P8. Unlinking or unproxying a site with a quick tunnel just ends the quick tunnel**; only a
  named tunnel (a `[sites.<site>.tunnel]` table) refuses with `SITE_TUNNELLED`, since only it leaves
  Cloudflare objects behind.
- **P9. `TUNNEL_NOT_READY`'s hint is `tail -n 50 <root>/logs/tunnel-<site>.log`**: there is no
  `yard logs` for units yet.
- **P10. The doctor DNS lookup is injectable** (`DaemonOptions.resolve_host: fn(&str) ->
  Option<bool>`), production calling `getaddrinfo`: `Some(false)` only for `EAI_NONAME`, `None`
  when the lookup itself failed. Tests never resolve a real name. Only the running yardd drops a
  `dns_left` entry; the offline doctor reports and writes nothing.
- **P11. The `tunnel::Provider` trait covers the running unit only** (program, args, env, the
  real-IP header). The add, remove and login steps are functions in `tunnel::cloudflared` that the
  RPC handlers call, until a second provider shows what they share; spec §7.1 says the trait must
  not grow ahead of ngrok and Tailscale Funnel, and a seam with one implementation guesses at the
  rest. Cost if wrong: moving those functions behind the trait when ngrok is planned.
- **P12. A named tunnel whose credentials file is gone gets no unit:** reconcile warns with
  `TUNNEL_CREDENTIALS_MISSING`'s message and hint, rather than starting a cloudflared that exits at
  once and backs off. Doctor fails `tunnel-credentials:<site>` meanwhile.

## Global Constraints

- cloudflared is brew's `cloudflared`, run as `<prefix>/opt/cloudflared/bin/cloudflared`; never
  `brew services`, never root, never through the helper; `no-autoupdate: true` always.
- Never read, copy or move cloudflared's credential files or certificates; existence checks only.
- Never bind a public interface: the tunnel listener is `listen 127.0.0.1:<port>` only.
- Never expose the default server, the site list, another site or a service through a tunnel; the
  generated yml ends with the catch-all `- service: http_status:404` for a named tunnel.
- Never run `route dns -f` unless the user passed `--overwrite-dns`.
- Never write or read a project's `.env`; Yard prints `APP_URL=https://<host>`.
- `loglevel: info` always (debug logs request headers, cookies included).
- Generated files carry `# Generated by Yard — edit sites.toml instead` and go through
  `generated::plan`; `nginx/tunnel-overrides/<site>.conf` is user-owned, created empty once by
  `ensure_user_file`, never written again.
- Unit `tunnel-<site>`; port entries `nginx-tunnel-<site>` (nginx's listener) and `tunnel-<site>`
  (cloudflared's metrics), both `auto`; log `logs/tunnel-<site>.log`.
- A named tunnel is `[sites.<site>.tunnel]` with `host` (required) and optional `tunnel` and
  `provider` (`"cloudflared"`, the only one built); domains are `[tunnels.cloudflared] domains`
  in `yard.toml`; facts are `state/tunnels.json`.
- Protocol goes from 2.9 to 2.10 (additive). Each new enum carries an `Other` fallback the daemon
  never sends.
- Tests never reach the network, Cloudflare or a real cloudflared; the fake is `yard-fake-unit`
  linked as `cloudflared`.
- Help and output copy is the spec's §5 text verbatim where the spec gives it.
- Commit messages: lowercase prose subject, prose paragraphs, no type prefix, no trailers. Ask
  the maintainer before each commit.

## Review Focus

The five failure modes most likely to bite, each pinned by a test in the task named:

1. **A laptop losing Wi-Fi keeps its quick tunnel's URL.** While `/ready` fails, the unit keeps
   its pid and `yard tunnels` shows `connecting` (Task 8,
   `quick_tunnel_survives_losing_the_edge`).
2. **Unrelated config changes never restart a tunnel.** Securing the site, changing its PHP or
   adding another site leaves `tunnel-<site>` on the same pid, since its yml depends only on its
   two ports (Task 7, `unrelated_changes_keep_the_tunnel_pid`).
3. **A site name with a dot** (`my.app`) gets a working unit, port entries, yml and tunnel override
   (Task 7, `a_dotted_site_name_tunnels`).
4. **The tunnel listener serves only its site:** any `Host` reaches that site, and nothing else Yard
   serves is reachable on it (Task 4, `tunnel_listener_serves_only_its_site`).
5. **After yardd restarts, quick tunnels are gone with their `ports.toml` entries, and named ones
   return** (Task 9, `quick_tunnels_end_with_yardd`; Task 11, `named_tunnel_returns_after_restart`).

---

### Task 1: Protocol 2.10: tunnel types, methods, event, error codes and job results

**Files:**
- Modify: `crates/yard-proto/src/types.rs`, `crates/yard-proto/src/methods.rs`,
  `crates/yard-proto/src/events.rs`, `crates/yard-proto/src/error.rs`, `crates/yard-proto/src/lib.rs`
- Modify (compile only): every `Event::JobFinished { job_id, error }` pattern in
  `crates/yard-cli/src/commands/mod.rs` and `crates/yard-daemon/src/jobs.rs` gains `result`/`..`.

**Interfaces:**
- Produces (all in `yard_proto`, re-exported like the existing types):
  - `enum TunnelProvider { Cloudflared, #[serde(other)] Other }`, serde `rename_all = "snake_case"`.
  - `enum TunnelKind { Quick, Named, #[serde(other)] Other }`, snake_case.
  - `enum TunnelState { Connecting, Ready, Backoff, Failed, Stopped, #[serde(other)] Other }`, snake_case.
  - `struct TunnelInfo { site, provider, kind, url: Option<String>, host: Option<String>,
    tunnel: Option<TunnelRef>, state: TunnelState, unit: String, origin_port: u16,
    metrics_port: u16, last_error: Option<String> }` (spec §12).
  - `struct TunnelRef { name: String, id: String, created_by_yard: bool }`.
  - `struct DnsRecord { host: String, target: String }`.
  - `struct TunnelRemoved { site: String, deleted_tunnel: bool, dns_left: Vec<DnsRecord> }`.
  - `struct TunnelDomain { provider: TunnelProvider, domain: String, logged_in: bool }`.
  - Params: `TunnelAddParams { site: String, #[serde(default)] provider: Option<String>,
    #[serde(default)] host: Option<String>, #[serde(default)] tunnel: Option<String>,
    #[serde(default)] overwrite_dns: bool }`; `TunnelRemoveParams { site: String,
    #[serde(default)] keep_tunnel: bool }`; `TunnelDomainAddParams { #[serde(default)] provider:
    Option<String>, domain: String }`; `TunnelDomainParams { domain: String }`.
  - Consts: `TUNNELS_LIST = "tunnels.list"`, `TUNNELS_DOMAINS = "tunnels.domains"`,
    `TUNNELS_DOMAIN_ADD = "tunnels.domain_add"`, `TUNNELS_DOMAIN_REMOVE = "tunnels.domain_remove"`,
    `TUNNELS_ADD = "tunnels.add"`, `TUNNELS_REMOVE = "tunnels.remove"`.
  - `Topic::Tunnels` (`"tunnels"`); `Event::TunnelChanged { tunnel: Box<TunnelInfo> }` on it.
  - `Event::JobFinished { job_id, error, #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value> }`.
  - `ErrorCode::{TunnelNotLoggedIn, TunnelDomainNotSetUp, TunnelKindRequired, TunnelExists,
    TunnelNotFound, TunnelCredentialsMissing, TunnelDnsExists, TunnelDnsFailed, TunnelNotReady,
    SiteTunnelled}`.
  - `PROTOCOL_VERSION = "2.10"`.

- [ ] **Step 1: Write the failing tests** in the crates' existing test modules:

```rust
#[test]
fn tunnel_wire_names() {
    assert_eq!(serde_json::to_value(TunnelProvider::Cloudflared).unwrap(), json!("cloudflared"));
    assert_eq!(serde_json::to_value(TunnelState::Connecting).unwrap(), json!("connecting"));
    assert_eq!(serde_json::from_value::<TunnelState>(json!("paused")).unwrap(), TunnelState::Other);
    assert_eq!(serde_json::from_value::<TunnelKind>(json!("funnel")).unwrap(), TunnelKind::Other);
    assert_eq!(serde_json::from_value::<TunnelProvider>(json!("ngrok")).unwrap(), TunnelProvider::Other);
    assert_eq!(serde_json::to_value(ErrorCode::SiteTunnelled).unwrap(), json!("SITE_TUNNELLED"));
    assert_eq!(serde_json::to_value(ErrorCode::TunnelDnsExists).unwrap(), json!("TUNNEL_DNS_EXISTS"));
}

#[test]
fn tunnel_method_names() {
    assert_eq!(
        [TUNNELS_LIST, TUNNELS_DOMAINS, TUNNELS_DOMAIN_ADD, TUNNELS_DOMAIN_REMOVE, TUNNELS_ADD, TUNNELS_REMOVE],
        ["tunnels.list", "tunnels.domains", "tunnels.domain_add", "tunnels.domain_remove", "tunnels.add", "tunnels.remove"]
    );
    let p: TunnelAddParams = serde_json::from_value(json!({"site": "blog"})).unwrap();
    assert_eq!((p.host, p.overwrite_dns), (None, false));
}

#[test]
fn job_finished_carries_a_result_and_reads_without_one() {
    let old: Event = serde_json::from_value(json!({"type":"job_finished","job_id":"j-1","error":null})).unwrap();
    assert!(matches!(old, Event::JobFinished { result: None, .. }));
    let done = Event::JobFinished { job_id: "j-1".into(), error: None, result: Some(json!({"site":"blog"})) };
    assert_eq!(serde_json::to_value(&done).unwrap()["result"]["site"], "blog");
    let changed = Event::TunnelChanged { tunnel: Box::new(sample_tunnel()) };
    assert_eq!(changed.topic(), Topic::Tunnels);
}
```

`sample_tunnel()` is a test helper building a quick `TunnelInfo` for `blog`.

- [ ] **Step 2: Run** `cargo test -p yard-proto`. Expected: FAIL, the types and consts do not exist.
- [ ] **Step 3: Implement** the Interfaces above. Fix the `JobFinished` patterns in yard-cli and
  yard-daemon so the workspace builds (`result: None` where it is emitted today).
- [ ] **Step 4: Run** `cargo test -p yard-proto && cargo build --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `protocol 2.10 types, methods and error codes for tunnels`.

### Task 2: `sites.toml` tunnel tables and `yard.toml` domains

**Files:**
- Create: `crates/yard-core/src/tunnel/mod.rs` (host validation, domain matching)
- Modify: `crates/yard-core/src/config/sites.rs`, `crates/yard-core/src/config/yard.rs`,
  `crates/yard-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `yard_core::tunnel::valid_host(host: &str) -> bool`: lowercase ASCII letters, digits and
    hyphens per label, labels 1–63 chars not starting or ending with `-`, at least two labels,
    total ≤ 253.
  - `yard_core::tunnel::domain_for<'a>(host: &str, domains: &'a [String]) -> Option<&'a str>`: the
    longest domain equal to `host` or that `host` ends with as `.<domain>`.
  - `config::sites::TunnelEntry { host: toml::Spanned<String>, tunnel: Option<String>,
    provider: Option<toml::Spanned<String>> }` with `fn host(&self) -> &str`; `SiteEntry.tunnel:
    Option<TunnelEntry>`.
  - `YardConfig.tunnel_domains: Vec<String>`, from `[tunnels.cloudflared] domains`.

- [ ] **Step 1: Write the failing tests** (`config/sites.rs`, `config/yard.rs`, `tunnel/mod.rs`):

```rust
#[test]
fn site_tunnel_table_is_parsed() {
    let text = "[sites.blog]\npath = \"/b\"\n\n[sites.blog.tunnel]\nhost = \"blog.example.com\"\ntunnel = \"blog-shared\"\n";
    let c = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap();
    let t = c.sites["blog"].tunnel.as_ref().unwrap();
    assert_eq!((t.host(), t.tunnel.as_deref()), ("blog.example.com", Some("blog-shared")));
}

#[test]
fn bad_tunnel_host_is_config_invalid_at_its_line() {
    for host in ["Blog.example.com", "localhost", "a..b", "-a.b"] {
        let text = format!("[sites.blog]\npath = \"/b\"\n\n[sites.blog.tunnel]\nhost = {host:?}\n");
        let e = parse_sites(Some(&text), Path::new("/r/sites.toml")).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid, "{host}");
        assert_eq!(e.data.unwrap()["line"], 5, "{host}");
    }
}

#[test]
fn a_host_two_sites_share_is_config_invalid_at_the_later_one() {
    let text = "[sites.a.tunnel]\nhost = \"x.example.com\"\n\n[sites.b]\npath = \"/b\"\n\n[sites.b.tunnel]\nhost = \"x.example.com\"\n";
    let e = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap_err();
    assert_eq!(e.data.unwrap()["line"], 8);
}

#[test]
fn an_unbuilt_provider_is_config_invalid() {
    let text = "[sites.b]\npath = \"/b\"\n\n[sites.b.tunnel]\nhost = \"b.example.com\"\nprovider = \"ngrok\"\n";
    let e = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap_err();
    assert!(e.message.contains("cloudflared"), "{}", e.message);
}

#[test]
fn tunnel_domains_are_parsed_and_checked() {
    let (c, _) = parse_yard(Some("[tunnels.cloudflared]\ndomains = [\"example.com\"]\n"), Path::new(FILE)).unwrap();
    assert_eq!(c.tunnel_domains, ["example.com"]);
    let e = parse(Some("[tunnels.cloudflared]\ndomains = [\"Example\"]\n")).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfigInvalid);
    assert!(parse(Some("[tunnels.ngrok]\ndomains = []\n")).is_err());
}

#[test]
fn domain_for_picks_the_longest_match() {
    let d = vec!["example.com".to_string(), "dev.example.com".to_string()];
    assert_eq!(domain_for("a.dev.example.com", &d), Some("dev.example.com"));
    assert_eq!(domain_for("example.com", &d), Some("example.com"));
    assert_eq!(domain_for("badexample.com", &d), None);
}
```

- [ ] **Step 2: Run** `cargo test -p yard-core config:: tunnel::`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Validate in `parse_sites` after deserializing, using the `Spanned`
  spans with `config_error`; duplicate hosts are reported at the later site in `BTreeMap` order
  read by span position (the later span). `yard.toml` keeps `deny_unknown_fields`; `[tunnels]`
  accepts only `cloudflared`.
- [ ] **Step 4: Run** `cargo test -p yard-core`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `sites.toml tunnel tables and yard.toml tunnel domains`.

### Task 3: cloudflared config rendering, paths and the facts file

**Files:**
- Create: `crates/yard-core/src/tunnel/cloudflared.rs`, `crates/yard-core/src/tunnel/facts.rs`
- Modify: `crates/yard-core/src/tunnel/mod.rs`, `crates/yard-core/src/paths.rs`,
  `crates/yard-core/src/brew.rs`

**Interfaces:**
- Produces:
  - `Paths::tunnel_yml(site) -> PathBuf` (`<root>/tunnels/<site>.yml`), `Paths::tunnels_dir()`,
    `Paths::tunnel_override(site)` (`<root>/nginx/tunnel-overrides/<site>.conf`),
    `Paths::tunnel_overrides_dir()`, `Paths::tunnels_state()` (`<root>/state/tunnels.json`).
  - `Brew::cloudflared_bin() -> PathBuf` (`<prefix>/opt/cloudflared/bin/cloudflared`).
  - `tunnel::unit(site) -> String` (`tunnel-<site>`), `tunnel::origin_entry(site) -> String`
    (`nginx-tunnel-<site>`), `tunnel::UNIT_PREFIX = "tunnel-"`.
  - `tunnel::cloudflared::cert_dir(home, domain) -> PathBuf` (`<home>/.cloudflared/yard/<domain>`),
    `cert_path(home, domain)` (`<cert_dir>/.cloudflared/cert.pem`),
    `created_credentials(home, domain, name)` (`<cert_dir>/<name>.json`),
    `default_credentials(home, id)` (`<home>/.cloudflared/<id>.json`).
  - `tunnel::cloudflared::Origin { origin_port: u16, metrics_port: u16 }`.
  - `tunnel::cloudflared::quick_yml(o: Origin) -> String` and
    `named_yml(o: Origin, id: &str, credentials: &Path, host: &str) -> String`.
  - `tunnel::facts::{Facts, NamedFacts, LeftRecord}`: `Facts { named: BTreeMap<String, NamedFacts>,
    dns_left: Vec<LeftRecord> }`, serialized as the spec's §6 shape (site keys at the top level beside
    `dns_left`) with `#[serde(flatten)]`; `NamedFacts { id, name, created_by_yard: bool,
    routed: Vec<String>, credentials: PathBuf, domain: String }`; `LeftRecord { host, target,
    since: String }`; `Facts::load(path) -> Result<Facts, YardError>` (missing file is empty) and
    `Facts::save(&self, path) -> Result<(), YardError>` (atomic write, mode 0600);
    `tunnel::cloudflared::target(id) -> String` (`<id>.cfargotunnel.com`).

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn quick_yml_is_url_metrics_and_fixed_settings() {
    let o = Origin { origin_port: 33013, metrics_port: 33014 };
    assert_eq!(quick_yml(o), "# Generated by Yard — edit sites.toml instead\nurl: http://127.0.0.1:33013\nmetrics: 127.0.0.1:33014\nno-autoupdate: true\nloglevel: info\ngrace-period: 3s\n");
}

#[test]
fn named_yml_routes_one_host_and_404s_the_rest() {
    let o = Origin { origin_port: 33011, metrics_port: 33012 };
    let y = named_yml(o, "1b2c", Path::new("/h/.cloudflared/yard/example.com/yard-blog.json"), "blog.example.com");
    assert_eq!(y, "# Generated by Yard — edit sites.toml instead\ntunnel: 1b2c\ncredentials-file: \"/h/.cloudflared/yard/example.com/yard-blog.json\"\nmetrics: 127.0.0.1:33012\nno-autoupdate: true\nloglevel: info\ngrace-period: 3s\ningress:\n  - hostname: blog.example.com\n    service: http://127.0.0.1:33011\n  - service: http_status:404\n");
}

#[test]
fn facts_round_trip_in_the_spec_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/tunnels.json");
    assert_eq!(Facts::load(&path).unwrap(), Facts::default());
    let mut f = Facts::default();
    f.named.insert("blog".into(), NamedFacts { id: "1b2c".into(), name: "yard-blog".into(), created_by_yard: true,
        routed: vec!["blog.example.com".into()], credentials: "/c.json".into(), domain: "example.com".into() });
    f.dns_left.push(LeftRecord { host: "old.example.com".into(), target: target("9f8e"), since: "2026-11-02".into() });
    f.save(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["blog"]["name"], "yard-blog");
    assert_eq!(v["dns_left"][0]["target"], "9f8e.cfargotunnel.com");
    assert_eq!(Facts::load(&path).unwrap(), f);
}
```

plus path assertions for every `Paths`/`cert_*` function above (one test, `tunnel_paths`).

- [ ] **Step 2: Run** `cargo test -p yard-core tunnel:: paths::`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** The yml quotes `credentials-file` as a JSON string (valid YAML).
- [ ] **Step 4: Run** `cargo test -p yard-core`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `render cloudflared configs and keep tunnel facts`.

### Task 4: the nginx tunnel listener

**Files:**
- Modify: `crates/yard-core/src/render/nginx.rs`, `crates/yard-core/src/render/mod.rs` (the
  `USER_OWNED` header text for the tunnel override, if it lives there)
- Test: the existing tests module in `render/nginx.rs`

**Interfaces:**
- Consumes: `Paths::tunnel_override` (Task 3).
- Produces:
  - `pub struct TunnelListener { pub port: u16, pub real_ip_header: &'static str }`
    (`"CF-Connecting-IP"` for cloudflared).
  - `nginx_site(ctx, site, wake, tunnel: Option<&TunnelListener>) -> String`: unchanged output for
    `None`; with `Some`, the site's blocks followed by the tunnel server block (spec §9).
  - `proxy_location` gains a `forwarded: Forwarded` parameter, `enum Forwarded { Local, Tunnel }`:
    `Local` is today's three headers; `Tunnel` is `X-Forwarded-For $remote_addr`,
    `X-Forwarded-Proto https`, `X-Forwarded-Host $host` (ruling P4).

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn tunnel_listener_serves_only_its_site() {
    let t = TunnelListener { port: 33011, real_ip_header: "CF-Connecting-IP" };
    let conf = nginx_site(&ctx(), &php_site("blog"), false, Some(&t));
    let block = conf.split("# Tunnel listener").nth(1).expect("a tunnel block");
    assert!(block.contains("listen 127.0.0.1:33011;"));
    assert!(!block.contains("[::1]") && !block.contains("default_server"));
    assert!(block.contains("server_name _;"));
    assert!(block.contains("set_real_ip_from 127.0.0.1;\n    real_ip_header CF-Connecting-IP;"));
    assert!(block.contains("fastcgi_param HTTPS on;") && block.contains("fastcgi_param SERVER_PORT 443;"));
    let over = block.find("nginx/overrides/blog.conf").unwrap();
    let tunnel_over = block.find("nginx/tunnel-overrides/blog.conf").unwrap();
    assert!(over < tunnel_over);
    // The site's own blocks are unchanged.
    assert!(conf.starts_with(&nginx_site(&ctx(), &php_site("blog"), false, None)));
}

#[test]
fn tunnel_proxy_trusts_only_the_visitor_address() {
    let t = TunnelListener { port: 33011, real_ip_header: "CF-Connecting-IP" };
    let conf = nginx_site(&ctx(), &proxy_site("api", "http://127.0.0.1:3000"), false, Some(&t));
    let block = conf.split("# Tunnel listener").nth(1).unwrap();
    assert!(block.contains("proxy_set_header X-Forwarded-For $remote_addr;"));
    assert!(block.contains("proxy_set_header X-Forwarded-Proto https;"));
    assert!(block.contains("proxy_set_header X-Forwarded-Host $host;"));
    assert!(!block.contains("$proxy_add_x_forwarded_for"));
}
```

A third test, `a_secured_site_tunnel_block_has_no_redirect`, asserts that the tunnel block of a
`secure` PHP site has no `return 301` and no `ssl_certificate`: the redirect is for `<site>.yard`.
`ctx()`, `php_site()` and `proxy_site()` follow the module's existing helpers.

- [ ] **Step 2: Run** `cargo test -p yard-core render::nginx`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** The tunnel block reuses the site's body (the setup page for a site
  that needs setup, the wake ask for one that may sleep), with the PHP location's two extra
  `fastcgi_param` lines after `include fastcgi_params`, and the comment line
  `# Tunnel listener: only cloudflared (tunnel-<site>) connects here.` above it. Update existing
  callers of `nginx_site` to pass `None` for now.
- [ ] **Step 4: Run** `cargo test -p yard-core`. Expected: PASS, existing snapshots unchanged.
- [ ] **Step 5: Commit** (ask the maintainer): `render the loopback listener a tunnel connects to`.

### Task 5: jobs that return a result

**Files:**
- Modify: `crates/yard-daemon/src/jobs.rs`, `crates/yard-cli/src/commands/mod.rs`
- Test: `crates/yard-daemon/tests/jobs.rs`, `crates/yard-cli/tests/cli.rs`

**Interfaces:**
- Produces:
  - `JobCtx::set_result(&self, value: serde_json::Value)`: the last value set is sent as
    `JobFinished.result` when the job succeeds; a failed job sends none.
  - CLI: `pub enum JobEnd { Finished(Option<serde_json::Value>), Abandoned(i32) }` and
    `pub async fn wait_job<P: Serialize>(ctx: &Ctx, method: &str, params: P) -> Result<JobEnd, CliError>`
    (the current `run_job` loop); `run_job` keeps its signature and calls it. Under `--json`,
    `wait_job` prints progress and `job_finished` lines as `run_job` does today.

- [ ] **Step 1: Write the failing tests.** In `tests/jobs.rs`, `a_job_result_rides_job_finished`:
  a job that calls `ctx.set_result(json!({"n": 1}))` finishes with `result == {"n": 1}`, and a job
  that sets one and then fails finishes with `result` absent. In the CLI tests, extend an existing
  `run_job` test's `MockDaemon` events with a `result` and assert the output is unchanged.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test jobs`. Expected: FAIL (`set_result` missing).
- [ ] **Step 3: Implement** with an `Arc<Mutex<Option<Value>>>` in `JobCtx`, read in `Jobs::start`
  when the work returns `Ok`.
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test jobs && cargo test -p yard-cli`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `let a job hand back a result when it finishes`.

### Task 6: a fake cloudflared

**Files:**
- Modify: `crates/yard-daemon/src/bin/yard-fake-unit.rs`, `crates/yard-daemon/tests/support/mod.rs`

**Interfaces:**
- Produces:
  - The fake, linked as `cloudflared`, keeps its state in its own directory (`argv0`'s parent) and
    appends each call's arguments, one line, to `calls` there. It accepts `--origincert <path>`
    anywhere after `tunnel`.
    - `tunnel --config <yml> [run]`: reads `url:`/`metrics:`/`tunnel:` from the yml, serves HTTP on
      the metrics address: `GET /ready` → 200 `{"status":200,"readyConnections":1}` unless
      `not-ready` exists in its directory (then 503); `GET /quicktunnel` →
      `{"hostname":"quick-<origin port>.trycloudflare.com"}` for a quick tunnel, `{"hostname":""}`
      for a named one. A named one exits 1 with `credentials file … doesn't exist` when its
      `credentials-file` is missing. Exits 0 on SIGTERM. Exits 1 at once while `start-fails` exists.
    - `tunnel login`: writes `$HOME/.cloudflared/cert.pem` (content `fake`), prints a login URL
      line and `You have successfully logged in.`; exits 1 while `login-fails` exists.
    - `tunnel list --name <n> -o json`: prints the entries of `tunnels.json` (a JSON array of
      `{"id","name"}`) with that name.
    - `tunnel create --credentials-file <f> <name>`: fails if the name is in `tunnels.json`;
      appends `{"id":"id-<name>","name":<name>}`, writes `<f>`, prints `Created tunnel <name> with
      id id-<name>`.
    - `tunnel route dns [-f] <id> <host>`: exits 1 with `Failed to add route: code: 1003, reason:
      An A, AAAA, or CNAME record with that host already exists.` when `<host>` is a line of
      `dns-records` and `-f` is absent; exits 1 with `Failed to add route: zone not found` when
      `<host>` ends with a line of `foreign-zones`; else appends the host to `dns-records`.
    - `tunnel cleanup <id>` succeeds; `tunnel delete <id>` removes the entry from `tunnels.json`,
      exits 1 while `delete-fails` exists.
  - `Fixture::add_cloudflared(&self) -> PathBuf`: links the fake at `brew.cloudflared_bin()` and
    returns its directory.

- [ ] **Step 1: Write the failing test** `fake_cloudflared_serves_ready_and_a_quick_hostname` in
  `crates/yard-daemon/tests/supervisor.rs` (where the other fakes are exercised): run the fake with
  a quick yml on two free ports, wait for `/ready` 200 and `/quicktunnel` naming
  `quick-<origin>.trycloudflare.com`, create `not-ready` and see 503, SIGTERM it and see exit 0.
  Plus `fake_cloudflared_manages_tunnels`: `create`, `list`, `route dns` twice (second fails 1003),
  `route dns -f`, `delete`, and `calls` has five lines.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test supervisor fake_cloudflared`. Expected: FAIL.
- [ ] **Step 3: Implement** the fake with std only (a blocking `TcpListener` thread for metrics).
- [ ] **Step 4: Run** the same. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `a fake cloudflared for the tests`.

### Task 7: tunnels in reconcile

**Files:**
- Create: `crates/yard-daemon/src/tunnel/mod.rs` (runtime state, unit specs, rendering glue),
  `crates/yard-daemon/src/tunnel/cloudflared.rs` (the provider: program, args, env)
- Modify: `crates/yard-daemon/src/lib.rs`, `crates/yard-daemon/src/reconcile.rs`,
  `crates/yard-daemon/src/state.rs`, `crates/yard-daemon/src/daemon.rs`,
  `crates/yard-daemon/src/watch.rs`
- Test: create `crates/yard-daemon/tests/tunnels.rs`

**Interfaces:**
- Consumes: Tasks 2–4, 6.
- Produces:
  - `tunnel::Runtime` (shared `Arc`, held by `Reconciler` and `Context` like `Sleeper`):
    `quick: Mutex<BTreeSet<String>>` with `add_quick(site)`, `remove_quick(site) -> bool`,
    `quick_sites() -> BTreeSet<String>`; `live: Mutex<BTreeMap<String, Live>>` where
    `Live { ready: bool, hostname: Option<String> }`, read by Task 8.
    `Reconciler::tunnels(&self) -> &Arc<Runtime>` and `Context.tunnels: Arc<Runtime>` (the same one).
  - `trait tunnel::Provider` with `fn unit_command(&self, brew: &Brew, home: &Path, yml: &Path,
    kind: TunnelKind) -> UnitCommand` and `fn real_ip_header(&self) -> &'static str`;
    `tunnel::cloudflared::Cloudflared` implements it: program `brew.cloudflared_bin()`, args
    `tunnel --config <yml>` (+ `run` for named), env `HOME=<home>` and `PATH` from
    `shim::child_path`.
  - `Snapshot.tunnels: BTreeMap<String, Planned>` with
    `Planned { kind: TunnelKind, origin_port: u16, metrics_port: u16, host: Option<String>,
    facts: Option<NamedFacts> }`, the tunnels this reconcile ran.
  - `port_owner("nginx-tunnel-<site>") == "nginx"`.
  - `ReconcileReport.warnings` gains `tunnel-<site>: …` lines.
- Behaviour, in reconcile's existing steps:
  - Step 2 (ports): for each site with a tunnel (named from `sites_cfg`, quick from
    `Runtime::quick_sites`, only if the site resolved), `registry.ensure(origin_entry, Auto, None,
    is_free)` then `ensure(unit, Auto, …)`; drop `nginx-tunnel-*` and `tunnel-*` entries of tunnels
    that no longer exist.
  - A named tunnel whose site has no `facts` entry is skipped with the warning
    `tunnel-<site>: no Cloudflare tunnel is recorded for it; run yard tunnel <site> --host <host>`.
    `cloudflared` missing skips every tunnel with the `FORMULA_MISSING` message as a warning. A
    named tunnel whose `facts.credentials` file does not exist is skipped with the
    `TUNNEL_CREDENTIALS_MISSING` warning (ruling P12).
  - Step 5 (render): `tunnels/<site>.yml` from Task 3; the site's nginx conf with
    `Some(&TunnelListener)`; `prepare_dirs` creates `nginx/tunnel-overrides/<site>.conf` with
    `ensure_user_file` (header `# USER-OWNED — Yard never writes this file. Included only in
    <site>'s tunnel listener.`). `tunnels/` joins the managed dirs, so a gone tunnel's yml is
    removed.
  - Step 8 (units): `tunnel-<site>`, `health: Health::Tcp(metrics_port)`, `ports:
    vec![(metrics_port, Proto::Tcp)]`, `reload: Reload::Restart`, `stop: SIGTERM`,
    `start_timeout: Some(Duration::from_secs(30))`, `pid_file: Some(<root>/run/tunnel-<site>.pid)`;
    restarted when its yml changed.
  - A `[sites.<site>.tunnel]` gone from `sites.toml` while `facts` still has the site: warning
    `tunnel-<site>: the Cloudflare tunnel <name> and its DNS record remain; yard doctor names them`.
  - `watch.rs` watches `nginx/tunnel-overrides/` as it watches `nginx/overrides/`.

- [ ] **Step 1: Write the failing tests** in `tests/tunnels.rs` (helpers: a prepared daemon with
  `add_cloudflared`, a linked PHP site `blog`, and a helper `add_quick(&daemon, "blog")` that calls
  `reconciler.tunnels().add_quick` and reconciles; Task 9 replaces it with the RPC):

```rust
#[tokio::test]
async fn a_quick_tunnel_gets_ports_a_yml_a_listener_and_a_unit() {
    // after add_quick: ports.toml has nginx-tunnel-blog and tunnel-blog (auto); tunnels/blog.yml
    // is quick_yml with those ports; nginx/sites/blog.conf has the tunnel block on the origin port;
    // nginx/tunnel-overrides/blog.conf exists; unit tunnel-blog reaches Running.
}

#[tokio::test]
async fn ending_a_quick_tunnel_removes_all_of_it() {
    // remove_quick + reconcile: unit gone, both port entries gone, yml removed, the tunnel block
    // gone from blog.conf, nginx/tunnel-overrides/blog.conf kept (user-owned).
}

#[tokio::test]
async fn unrelated_changes_keep_the_tunnel_pid() {
    // with tunnel-blog running: secure blog, isolate it to another PHP, link a second site; after
    // each reconcile tunnel-blog has the same pid.
}

#[tokio::test]
async fn a_dotted_site_name_tunnels() {
    // site "my.app": unit tunnel-my.app Running, entries nginx-tunnel-my.app and tunnel-my.app,
    // tunnels/my.app.yml and nginx/tunnel-overrides/my.app.conf exist.
}

#[tokio::test]
async fn a_named_tunnel_without_facts_is_a_warning() {
    // [sites.blog.tunnel] host = "blog.example.com", no state/tunnels.json: no unit, the warning
    // names yard tunnel blog --host blog.example.com.
}

#[tokio::test]
async fn a_named_tunnel_with_facts_runs() {
    // facts for blog with a credentials file that exists: tunnels/blog.yml is named_yml with the id,
    // the credentials path and the host; tunnel-blog is Running with args ending in "run".
}
```

- [ ] **Step 2: Run** `cargo test -p yard-daemon --test tunnels`. Expected: FAIL (no tunnel module).
- [ ] **Step 3: Implement** the Interfaces. Keep reconcile's additions as calls into
  `tunnel::` functions (`settle_ports`, `render`, `units`), so `reconcile.rs` grows by tens of
  lines, not hundreds.
- [ ] **Step 4: Run** `cargo test -p yard-daemon`. Expected: PASS, including the existing reconcile
  and nginx tests.
- [ ] **Step 5: Commit** (ask the maintainer): `run tunnels as yardd units behind their own nginx listener`.

### Task 8: the tunnel monitor, `tunnels.list` and `tunnel_changed`

**Files:**
- Create: `crates/yard-daemon/src/tunnel/monitor.rs`, `crates/yard-daemon/src/rpc/tunnels.rs`
- Modify: `crates/yard-daemon/src/daemon.rs` (spawn it; `DaemonOptions.tunnel_poll: Duration`,
  2 s in production, 100 ms in `support::options`), `crates/yard-daemon/src/rpc/handlers.rs`,
  `crates/yard-daemon/src/rpc/mod.rs`
- Test: `crates/yard-daemon/tests/tunnels.rs`

**Interfaces:**
- Produces:
  - `tunnel::monitor::http_get(port: u16, path: &str, timeout: Duration) -> Option<(u16, String)>`:
    HTTP/1.1 `GET` to `127.0.0.1:<port>` with `Connection: close`, status and body.
  - `tunnel::monitor::spawn(ctx: Arc<Context>, every: Duration) -> JoinHandle<()>`: each tick, for
    each tunnel in the published snapshot whose unit is `Running` or `Starting`, reads `/ready`
    (200 = ready) and, for a quick one without a hostname yet, `/quicktunnel`; updates
    `Runtime.live`; emits `Event::TunnelChanged` when a tunnel's `TunnelInfo` differs from the last
    one emitted (state, url or last_error).
  - `tunnel::info(site, planned: &Planned, unit: Option<&UnitInfo>, live: Option<&Live>) ->
    TunnelInfo`: state `Ready` when the unit is Running and `live.ready`; `Connecting` when Running
    or Starting otherwise; `Backoff`, `Failed`, `Stopped` from the unit; no unit is `Stopped`.
    `url` is `https://<host>` for named, `https://<hostname>` for quick once known. `last_error` is
    the unit's.
  - `rpc::tunnels::list(ctx) -> Result<Vec<TunnelInfo>, YardError>`, sorted by site;
    `methods::TUNNELS_LIST` dispatches to it.

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn tunnels_list_shows_a_quick_url_once_known() {
    // add_quick blog; poll tunnels.list until its url is https://quick-<origin>.trycloudflare.com
    // and state "ready"; kind "quick", unit "tunnel-blog", provider "cloudflared".
}

#[tokio::test]
async fn quick_tunnel_survives_losing_the_edge() {
    // ready; create the fake's not-ready file: within 2 s tunnels.list says "connecting" and a
    // tunnel_changed event arrives; for 3 s more the unit keeps its pid; remove not-ready: "ready"
    // again, same pid, same url.
}
```

- [ ] **Step 2: Run** `cargo test -p yard-daemon --test tunnels tunnels_list quick_tunnel_survives`.
  Expected: FAIL.
- [ ] **Step 3: Implement** the Interfaces. Add the `Topic::Tunnels` subscription path in
  `rpc/server.rs` if topics are filtered there.
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test tunnels`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `watch each tunnel's readiness and list tunnels`.

### Task 9: quick tunnels through `tunnels.add` and `tunnels.remove`

**Files:**
- Modify: `crates/yard-daemon/src/rpc/tunnels.rs`, `crates/yard-daemon/src/rpc/handlers.rs`
- Test: `crates/yard-daemon/tests/tunnels.rs` (replace the `add_quick` helper with the RPC)

**Interfaces:**
- Produces:
  - `rpc::tunnels::add(ctx, p: TunnelAddParams) -> Result<JobStarted, YardError>`. Before the job:
    the site must resolve (`SITE_NOT_FOUND`), the provider must be `cloudflared` or absent
    (`INVALID_ARGUMENT`, message `no tunnel provider <name>; the providers are: cloudflared`), the
    site must have no tunnel (`INVALID_ARGUMENT`, hint `yard untunnel <site>`), cloudflared must
    exist (`FORMULA_MISSING`, hint `brew install cloudflared`). With no `host`, the job
    (`tunnel-add`) prints `Starting a quick tunnel for <site>…`, adds it to `Runtime`, reconciles,
    and waits up to `DaemonOptions.tunnel_ready_wait` (30 s in production, 2 s in
    `support::options`; `Context` carries it) for `/ready` and a hostname, checking
    cancel between polls; then sets the `TunnelInfo` as its result. On timeout or cancel it
    removes the quick tunnel again, reconciles, and fails `TUNNEL_NOT_READY` (message
    `tunnel-<site> did not connect to Cloudflare within 30 s`, hint `tail -n 50
    <root>/logs/tunnel-<site>.log`, `data.tail` the log's last 20 lines).
  - `rpc::tunnels::remove(ctx, p: TunnelRemoveParams) -> Result<JobStarted, YardError>`: a site
    with no tunnel is `TUNNEL_NOT_FOUND` (hint `yard tunnel <site>`) before the job. For a quick
    tunnel the job (`tunnel-remove`) removes it, reconciles, and sets `TunnelRemoved { site,
    deleted_tunnel: false, dns_left: [] }`. (Named: Task 11.)
  - `sites.unlink`/`sites.unproxy` end a quick tunnel on the site (ruling P8).

- [ ] **Step 1: Write the failing tests:** `quick_tunnel_add_and_remove_by_rpc` (job result is a
  ready `TunnelInfo` with a url; remove leaves no unit and no entries);
  `a_quick_tunnel_that_never_connects_leaves_nothing` (fake `not-ready`, job fails
  `TUNNEL_NOT_READY` with a `tail`, no `tunnel-blog` entries left);
  `quick_tunnels_end_with_yardd` (stop and start the daemon on the same root: `tunnels.list` empty,
  no `tunnel-*` entries in `ports.toml`); `untunnel_without_a_tunnel_is_not_found`;
  `add_refuses_a_second_tunnel_and_an_unknown_provider`; `unlink_ends_a_quick_tunnel`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test tunnels`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.** A yardd that starts finds `tunnel-*` and `nginx-tunnel-*` entries with
  no tunnel and drops them in its first reconcile (Task 7's stale-entry rule already covers this).
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test tunnels`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `start and stop quick tunnels`.

### Task 10: domains: `tunnels.domains`, `tunnels.domain_add`, `tunnels.domain_remove`

**Files:**
- Modify: `crates/yard-daemon/src/rpc/tunnels.rs`, `crates/yard-daemon/src/rpc/handlers.rs`,
  `crates/yard-daemon/src/tunnel/cloudflared.rs`
- Test: `crates/yard-daemon/tests/tunnels.rs`

**Interfaces:**
- Produces:
  - `rpc::tunnels::domains(ctx) -> Result<Vec<TunnelDomain>, YardError>`: one per
    `YardConfig.tunnel_domains`, `logged_in` = `cert_path(home, domain)` exists.
  - `rpc::tunnels::domain_add(ctx, p: TunnelDomainAddParams) -> Result<JobStarted, YardError>`:
    `INVALID_ARGUMENT` for a domain failing `valid_host` or an unknown provider, before the job.
    The job (`tunnel-login`): when the certificate already exists it prints `Already signed in for
    <domain>.`; else it prints `In the browser, pick the zone <domain>.` and runs `cloudflared
    tunnel login` with `HOME=<cert_dir>` (created first) through `JobCtx::run`, then fails
    `TUNNEL_NOT_LOGGED_IN` if the certificate still does not exist. Then it adds the domain to
    `[tunnels.cloudflared] domains` with `TomlFile::edit` (`array_push_unique`), reconciles, and
    sets the `TunnelDomain` as its result.
  - `rpc::tunnels::domain_remove(ctx, p: TunnelDomainParams) -> Result<Vec<TunnelDomain>,
    YardError>`: `INVALID_ARGUMENT` (hint `yard tunnels domains`) for a domain not listed; removes
    it with `array_remove`, reconciles, returns the remaining list. The certificate stays.

- [ ] **Step 1: Write the failing tests:** `domain_add_signs_in_and_records_the_domain` (the fake's
  `calls` has `tunnel login`, the cert exists under `<home>/.cloudflared/yard/example.com/`,
  `yard.toml` lists it, `tunnels.domains` says `logged_in: true`);
  `domain_add_with_a_certificate_skips_login`; `a_failed_login_records_nothing` (`login-fails`);
  `domain_remove_forgets_only_the_domain` (a comment above `domains` in `yard.toml` survives, the
  cert stays).
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test tunnels domain`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test tunnels`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `set up Cloudflare domains for named tunnels`.

### Task 11: named tunnels

**Files:**
- Modify: `crates/yard-daemon/src/rpc/tunnels.rs`, `crates/yard-daemon/src/tunnel/cloudflared.rs`,
  `crates/yard-daemon/src/rpc/handlers.rs` (`unlink`)
- Test: `crates/yard-daemon/tests/tunnels.rs`

**Interfaces:**
- Consumes: `Facts` (Task 3), `domain_for`, `cert_path`, `created_credentials`,
  `default_credentials`.
- Produces: `tunnels.add` with `host`, following spec §8 exactly, as the job `tunnel-add`:
  1. Before the job: `host` fails `valid_host` → `INVALID_ARGUMENT`; no domain → 
     `TUNNEL_DOMAIN_NOT_SET_UP` (hint `yard tunnels domain add <last two labels of host>`); the
     domain's certificate missing → `TUNNEL_NOT_LOGGED_IN` (hint `yard tunnels domain add
     <domain>`); another site's `[tunnel] host` equal → `INVALID_ARGUMENT`.
  2. `cloudflared tunnel --origincert <cert> list --name <name> -o json`, `<name>` = `p.tunnel` or
     `yard-<site>`. Without `p.tunnel`, a listed tunnel that `Facts` does not record as this site's
     and Yard's → `TUNNEL_EXISTS` (hint `yard tunnel <site> --host <host> --tunnel yard-<site>`).
     With `p.tunnel`, none listed → `TUNNEL_NOT_FOUND` (hint `cloudflared tunnel list`); adopted
     facts get `created_by_yard: false`, `credentials: default_credentials(home, id)`.
  3. Otherwise print `Creating the Cloudflare tunnel <name>…`, run `tunnel --origincert <cert> create
     --credentials-file <created_credentials> <name>`, read the id with `list` again, record facts
     (`created_by_yard: true`); a missing credentials file → `TUNNEL_CREDENTIALS_MISSING`.
  4. Print `Routing <host> to it…`, run `tunnel --origincert <cert> route dns [-f] <id> <host>`
     (`-f` only for `overwrite_dns`). Output containing `already exists` → `TUNNEL_DNS_EXISTS`
     (hint `yard tunnel <site> --host <host> --overwrite-dns`); any other failure →
     `TUNNEL_DNS_FAILED` with `data.output` (hint `check that <domain> is in the Cloudflare account
     you signed in to`). Record the host in `routed`.
  5. Write `[sites.<site>.tunnel] host` (and `tunnel` when adopted) with `TomlFile::edit`, save
     facts, reconcile, wait `tunnel_ready_wait` for `/ready`. A timeout fails `TUNNEL_NOT_READY` but
     keeps the config and facts (the unit keeps trying).
  6. A failure after step 3 runs `tunnel delete <id>` for a tunnel this job created and drops its
     facts; a host routed in step 4 moves to `dns_left` (`since` today, local date).
  - `tunnels.remove` for a named tunnel (job `tunnel-remove`): remove the table, reconcile; if
    `created_by_yard` and not `keep_tunnel`, run `tunnel --origincert <cert> cleanup <id>` then
    `delete <id>` (`deleted_tunnel: true`; a failure prints a `warning:` line, keeps the facts entry
    with `routed` emptied, `deleted_tunnel: false`); move every `routed` host to `dns_left`; set
    `TunnelRemoved` with those records (target `<id>.cfargotunnel.com`).
  - `sites.unlink`/`sites.unproxy` of a site with `[sites.<site>.tunnel]` → `SITE_TUNNELLED`
    (message `<site> is shared through a named tunnel`, hint `yard untunnel <site>`).

- [ ] **Step 1: Write the failing tests:** `named_tunnel_creates_routes_and_runs` (calls in order:
  list, create, list, route dns; `sites.toml` has the table; facts recorded; result url
  `https://blog.example.com`); `named_tunnel_refuses_a_foreign_yard_tunnel` (fake `tunnels.json`
  already has `yard-blog`: `TUNNEL_EXISTS`, nothing written); `adopting_with_tunnel_never_deletes`
  (add with `tunnel: "blog-shared"`, remove: no `delete` call, `deleted_tunnel: false`, the host in
  `dns_left`); `an_existing_record_is_dns_exists_and_overwrite_passes_f` (first add fails
  `TUNNEL_DNS_EXISTS` and deletes the tunnel it made; the second with `overwrite_dns` routes with
  `-f`); `a_foreign_zone_deletes_the_new_tunnel` (`TUNNEL_DNS_FAILED`, a `delete` call, no facts);
  `untunnel_deletes_and_leaves_the_record` (`deleted_tunnel: true`, `dns_left` names the host and
  `id-yard-blog.cfargotunnel.com`); `a_failed_delete_keeps_the_id` (`delete-fails`);
  `named_tunnel_returns_after_restart`; `unlink_refuses_a_named_tunnel`;
  `host_outside_every_domain_is_not_set_up` and `missing_certificate_is_not_logged_in`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test tunnels named`. Expected: FAIL.
- [ ] **Step 3: Implement.** Parse `list -o json` with `serde_json` (`[{"id","name",…}]`).
- [ ] **Step 4: Run** `cargo test -p yard-daemon`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `named tunnels on a domain you set up`.

### Task 12: doctor

**Files:**
- Modify: `crates/yard-daemon/src/doctor.rs`, `crates/yard-daemon/src/daemon.rs`
  (`DaemonOptions.resolve_host`), `crates/yard-daemon/src/tunnel/mod.rs` (`resolves_with_getaddrinfo`)
- Test: `crates/yard-daemon/tests/doctor.rs`

**Interfaces:**
- Produces: `fn tunnel_checks(view: &TunnelView) -> Vec<Check>`, run only when a tunnel is
  configured, quick tunnels run, or `Facts` is not empty. Checks (spec §13):
  - `cloudflared`: ok when `Brew::cloudflared_bin` exists; fail with hint `brew install cloudflared`.
  - `tunnel-login`: warn per configured domain without its certificate, hint `yard tunnels domain
    add <domain>`.
  - `tunnel:<site>`: ok when `Ready`; warn `connecting to Cloudflare` while `Connecting`; fail with
    the unit's last error when `Failed` or `Backoff`, hint `yard restart tunnel-<site>`.
  - `tunnel-credentials:<site>`: fail when a named tunnel's `credentials` file is missing, message
    `TUNNEL_CREDENTIALS_MISSING`'s, hint `yard untunnel <site> && yard tunnel <site> --host <host>`.
  - `tunnel-leftover:<name>`: warn for a facts entry whose site has no `[tunnel]` table and was
    created by Yard, hint `cloudflared tunnel delete <id>`.
  - `tunnel-dns:<host>`: warn per `dns_left` record that `resolve_host` says still resolves (or
    could not be asked), message `the DNS record <host> → <target> is still there`, hint `delete the
    CNAME <host> in the Cloudflare dashboard`; a record that no longer resolves (`Some(false)`) is
    dropped from `state/tunnels.json` by the running yardd and gets no check.
  - `tunnel-hot:<site>`: warn when a tunnelled linked site's docroot has `hot`, i.e.
    `<docroot>/hot` (Laravel's `public/hot`), hint `stop npm run dev, or delete <file>`.
  - `DaemonOptions.resolve_host: fn(&str) -> Option<bool>` (ruling P10); tests set a function
    returning `Some(host.starts_with("kept."))`.

- [ ] **Step 1: Write the failing tests:** `doctor_is_silent_without_tunnels` (no `tunnel` ids at
  all); `doctor_reports_a_ready_tunnel_and_a_missing_login`; `doctor_warns_while_connecting`;
  `doctor_fails_missing_credentials`; `doctor_drops_a_gone_record_and_warns_a_kept_one`
  (`dns_left` has `gone.example.com` and `kept.example.com`: one `tunnel-dns:kept.example.com`
  warning, and `state/tunnels.json` keeps only the kept one); `doctor_warns_a_vite_hot_file`;
  `doctor_names_a_leftover_tunnel`.
- [ ] **Step 2: Run** `cargo test -p yard-daemon --test doctor tunnel`. Expected: FAIL.
- [ ] **Step 3: Implement.** The offline doctor builds `TunnelView` from the files (no quick tunnels,
  no unit states, so no `tunnel:<site>` check) and never writes the facts file.
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test doctor`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `doctor checks for tunnels and the DNS records they leave`.

### Task 13: `yard env` prints the public `APP_URL`

**Files:**
- Modify: `crates/yard-daemon/src/rpc/info.rs`
- Test: `crates/yard-daemon/tests/tunnels.rs`

**Interfaces:**
- Produces: for `env.print` with a site (or path) whose tunnel has a url, an extra
  `EnvBlock { instance: "tunnel", lines: ["APP_URL=<url>"], note: Some("while <site> is shared
  through a tunnel; Yard does not write .env".into()) }` after the service blocks.

- [ ] **Step 1: Write the failing test** `env_prints_the_tunnel_app_url` (quick tunnel ready:
  the block is present with `APP_URL=https://quick-<port>.trycloudflare.com`; after untunnel it is
  gone).
- [ ] **Step 2: Run** it. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p yard-daemon --test tunnels`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `yard env prints the public APP_URL of a shared site`.

### Task 14: the CLI: `tunnel`, `untunnel`, `tunnels`, `tunnels domain add|remove`, `tunnels domains`

**Files:**
- Create: `crates/yard-cli/src/commands/tunnels.rs`
- Modify: `crates/yard-cli/src/cli.rs`, `crates/yard-cli/src/commands/mod.rs`
- Test: `crates/yard-cli/tests/cli.rs` (or a new `crates/yard-cli/tests/tunnels.rs` with the same
  support), snapshots in `crates/yard-cli/tests/snapshots`

**Interfaces:**
- Consumes: `wait_job`/`JobEnd` (Task 5), the proto types (Task 1).
- Produces, with the spec §5 help text verbatim as each command's first line:
  - `Tunnel { site: Option<String>, #[arg(long, conflicts_with = "host")] quick: bool,
    #[arg(long)] host: Option<String>, #[arg(long)] provider: Option<String>, #[arg(long)]
    tunnel: Option<String>, #[arg(long)] overwrite_dns: bool }`. The site defaults to the one
    containing the current directory (`containing`, as `secure` does). With neither `--quick` nor
    `--host`: no domains → quick; domains and `interactive(ctx)` → the spec §4.1 prompt (default
    the first suggestion, choice 3 reads a name and checks `domain_for`); domains and not
    interactive → print `TUNNEL_KIND_REQUIRED` (message `<site> can be shared at a random URL or on
    a domain you set up; say which`, hint `yard tunnel <site> --quick, or --host <name>`) and exit 2.
  - Human output after the job, from its `TunnelInfo` result, exactly as spec §5 shows (quick and
    named), plus for a proxied site `  Vite and similar dev servers refuse unknown hosts: add <host>
    to server.allowedHosts.` and for every site `  Error pages and debug output are public too.`
    and, when the host is two or more labels below its domain, `  Cloudflare's free certificate
    covers <domain> and *.<domain> only; <host> needs an advanced certificate.`
  - `Untunnel { site: Option<String>, #[arg(long)] keep_tunnel: bool }`: the spec §5 teardown
    text from `TunnelRemoved` (quick: `Stopped the tunnel for <site>.`; adopted or kept: `…and kept
    the Cloudflare tunnel <name>.`).
  - `Tunnels { #[command(subcommand)] command: Option<TunnelsCommand> }`; `None` lists:
    the table `SITE URL KIND STATE TUNNEL` (`-` for none), then the agents line
    `For agents and scripts: yard tunnels --json`; empty: `No tunnels. Share a site with yard
    tunnel, run inside it.`
  - `TunnelsCommand::Domain { #[command(subcommand)] command: DomainCommand }` with
    `DomainCommand::Add { domain: String, #[arg(long)] provider: Option<String> }` (refuses
    without a terminal: `INVALID_ARGUMENT`, message `yard tunnels domain add opens a browser to
    sign in`, hint `run it in a terminal`) and `Remove { domain: String }`; and
    `TunnelsCommand::Domains` (table `DOMAIN PROVIDER SIGNED IN`, agents line, empty: `No domains.
    Set one up with yard tunnels domain add <domain>.`).
  - `--json` prints `TunnelInfo`, `TunnelRemoved`, `Vec<TunnelInfo>`, `Vec<TunnelDomain>`.

- [ ] **Step 1: Write the failing tests** against `MockDaemon`: `tunnel_quick_prints_the_url`
  (sequenced `sites.list`, `tunnels.domains` → `[]`, `tunnels.add` job events with a result; output
  matches the spec's quick text); `tunnel_named_prints_the_app_url`;
  `tunnel_without_a_terminal_and_with_domains_exits_2` (stdin not a TTY; exit 2, stderr
  `TUNNEL_KIND_REQUIRED`'s message and hint, no `tunnels.add` sent); `tunnel_quick_and_host_conflict`
  (clap exit 2); `untunnel_prints_the_record_to_delete`; `tunnels_lists_and_ends_with_agents_line`;
  `tunnels_empty_says_what_to_do`; `tunnels_domains_lists`; `domain_add_needs_a_terminal`;
  `tunnels_json_is_the_proto_list`.
- [ ] **Step 2: Run** `cargo test -p yard-cli tunnel`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add every new method to the `MockDaemon`-based tests' expectations.
- [ ] **Step 4: Run** `cargo test -p yard-cli`. Expected: PASS.
- [ ] **Step 5: Commit** (ask the maintainer): `yard tunnel, untunnel and tunnels`.

### Task 15: docs, the e2e script, and the whole suite

**Files:**
- Create: `scripts/e2e-m5.sh`
- Modify: `docs/superpowers/specs/2026-10-19-yard-design.md` (§3.3 data layout, §5 a tunnels
  section, §6.1 tunnel units' TCP health and readiness, §6.2 the six methods and the `tunnels`
  topic, §6.3 the ten codes, §7 the two entries per tunnel, §12 M5), the tunnels spec (status
  `built`, rulings P1–P10 replacing the builder's calls they change: §4.1 DOMAIN required and
  per-domain certificates, §7 health, §9 forwarded headers), `README.md`, `AGENTS.md` (status,
  the e2e line), `.hydra/rules/cli-output.md`, `.hydra/rules/port-registry.md`,
  `.hydra/rules/config-ownership.md`, `.hydra/rules/homebrew.md`, `.hydra/rules/rpc-protocol.md`
  (protocol 2.10)

**Interfaces:**
- `scripts/e2e-m5.sh` (shellcheck clean; real machine, network, no Cloudflare account; ask the maintainer
  before running): needs yardd running, brew's cloudflared and jq; backs up and restores
  `sites.toml` and `ports.toml`; links a scratch PHP project under `$TMPDIR` whose `index.php`
  prints `$_SERVER['HTTPS']`, `$_SERVER['HTTP_HOST']` and `$_SERVER['REMOTE_ADDR']`; runs `yard
  tunnel --quick --json` in it; curls the trycloudflare URL (retrying up to 60 s for DNS) and checks
  `HTTPS=on`, the public host, and a `REMOTE_ADDR` that is not `127.0.0.1`; checks a `Host:
  <other site>.yard` request on the tunnel's origin port still gets the scratch site; runs `yard
  untunnel`, checks the unit and both port entries are gone; unlinks; fails if Herd's resolver
  changed or `~/.cloudflared` was created.

- [ ] **Step 1: Grep the docs** for every name this milestone touched (`tunnel`, `untunnel`,
  `cloudflared`, `TUNNEL_`, `SITE_TUNNELLED`, `2.10`, `tunnel-overrides`, `tunnels.json`) and update
  each place; say which docs were checked in the task's report.
- [ ] **Step 2: Write** `scripts/e2e-m5.sh`; run `shellcheck scripts/e2e-m5.sh` and `bash -n`.
  Expected: clean.
- [ ] **Step 3: Run** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings &&
  cargo test --workspace`. Expected: all pass.
- [ ] **Step 4: Commit** (ask the maintainer): `M5 docs and the quick-tunnel e2e check`.
- [ ] **Step 5: With the maintainer's approval:** rebuild release, restart yardd, run `scripts/e2e-m1.sh` and
  `scripts/e2e-m5.sh`. A named-tunnel check needs the maintainer's Cloudflare zone and login and is his
  manual step: `yard tunnels domain add <his domain>`, `yard tunnel --host <name>.<domain>`, curl
  it, `yard untunnel`, delete the CNAME, `yard doctor` clears within one run after the record goes.
