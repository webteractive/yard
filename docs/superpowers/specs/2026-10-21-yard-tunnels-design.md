# Yard tunnels — design spec

Date: 2026-10-21
Status: draft, not yet approved. It extends `2026-10-19-yard-design.md` (the main spec),
which stays the source of truth. Where the two disagree, the main spec wins until this one
is approved and folded in.

Milestone numbers follow the order the maintainer set on 2026-10-21: M4 Node and services,
**M5 Tunnels**, M6 Yard.app, M7 Distribution. The main spec's §12 is being renumbered
separately.

Decisions marked **(builder's call)** were made while writing this, not by the maintainer. Each one
can be reversed without changing anything else here unless it says otherwise. The
questions only the maintainer can answer are in §16.

## 1. Intent

Share a site Yard serves at a public HTTPS URL through a cloudflared tunnel, with one
command and no inbound port opened on the Mac.

Why:

- Yard is not only a PHP tool. With tunnels in Yard, one daemon supervises the site, nginx
  and the tunnel, and one registry knows every port.
- Common needs: show a client a branch, receive webhooks (Stripe, GitHub) at a real URL,
  test on a phone off the LAN, and keep an internal app reachable at a fixed hostname.
- **Scope (maintainer, 2026-10-21):** a tunnel only shares a site Yard already serves, publicly or
  with whoever has the link, for webhooks and similar uses. It is not a production
  deployment: the site keeps its development settings, and there are no
  production profiles or workers.
- What to avoid: a teardown that leaves a DNS CNAME for the user to find unprompted, a
  launchd plist per process, and `route dns -f`, which overwrites whatever record was there.

## 2. Goals, non-goals, anti-goals

### Goals

1. `yard tunnel` gives the current site a public URL with no Cloudflare account (a quick
   tunnel on `trycloudflare.com`).
2. `yard tunnel --host app.example.com` gives it a stable hostname on a zone in the user's
   Cloudflare account (a named tunnel), which survives restarts of yardd and the Mac.
3. cloudflared is a yardd unit: supervised, logged, health-checked, restarted with backoff,
   shown by `yard status`, `yard doctor` and `yard ports` like every other unit.
4. A tunnel exposes exactly one site and nothing else Yard serves.
5. The app behind it sees the public hostname, `https`, and the visitor's IP, without the
   user configuring trusted proxies for a PHP site.
6. Teardown cleans up everything Yard can clean up, and names precisely what it cannot
   (the DNS record), then keeps reminding through `yard doctor` until it is gone.

### Non-goals (v1)

- Other tunnel providers (ngrok, Tailscale Funnel, Expose). The design keeps the provider
  behind one module, but v1 ships cloudflared only **(builder's call)**.
- Cloudflare Access policies, WAF rules or anything else configured in the Cloudflare
  dashboard. Users do that themselves.
- Deleting DNS records through the Cloudflare API (see §8 and open question 2).
- TCP/UDP tunnels (databases, SSH), private networks and WARP routing.
- Remote-managed (dashboard-configured, token-run) tunnels. Yard runs locally-managed
  tunnels only **(builder's call)**.
- Pausing a named tunnel without removing it. `yard untunnel` and `yard tunnel` again is the
  v1 path **(builder's call)**.
- A production PHP profile, queue workers or a scheduler Tunnels share
  a dev site and nothing more (maintainer, 2026-10-21, §1).

### Anti-goals (never)

- Never expose the default server, the site list, another site or a service through a tunnel.
- Never bind a public interface. Every listener stays on loopback; cloudflared only dials out.
- Never read, copy or move cloudflared's credential files (`~/.cloudflared/cert.pem`,
  `~/.cloudflared/<uuid>.json`). Yard may check that they exist; cloudflared reads them.
- Never overwrite an existing DNS record without the user asking for it by flag.
- Never write the project's `.env` (main spec non-goal) — Yard prints the lines to set.
- Never run `cloudflared` with root, through the helper, or with brew's service plist.
- Never let cloudflared update itself; brew owns the binary.

## 3. Constraints

From the main spec and `.hydra/rules/`:

- **Homebrew is a package source.** cloudflared is brew's `cloudflared` formula, run by
  absolute path (`<prefix>/opt/cloudflared/bin/cloudflared`), never `brew services`.
  Installing it is the user's step or an explicit `yard` command; yardd never installs it
  silently. Its brew caveat suggests `brew services restart cloudflared`; Yard ignores that.
- **Port registry.** No unit binds a port the registry has not granted (main spec §7). Each
  tunnel adds listeners (§9), so each gets entries.
- **Config ownership.** User intent goes in a user TOML edited through `TomlFile`;
  generated files carry the header and a recorded hash; user-owned files are created empty
  once and never written (§6).
- **Root helper.** No new verb. Tunnels need nothing from root.
- **RPC protocol.** New methods, events and error codes go in `yard-proto`, with the CLI,
  the `MockDaemon` and the method list in main spec §6.2 updated in the same change (§12).
- **Herd keeps working.** Tunnels touch no Herd file and no port Herd uses.
- **Tests** never reach the network or Cloudflare. A fake `cloudflared` (a new
  `yard-fake-unit` mode, dispatched on `argv[0]`) stands in, as the fake nginx does now.
- Facts about cloudflared on this machine (2026-10-21): brew `cloudflared` 2026.6.1
  installed at `/opt/homebrew/bin/cloudflared`, 2026.10.2 available. Its CLI can create,
  list, route and delete tunnels, and **can create a DNS CNAME (`tunnel route dns`) but
  has no command to delete one**. `--metrics` serves a `/ready` endpoint; `tunnel ready`
  calls it.

## 4. Two kinds of tunnel

| | Quick | Named |
|---|---|---|
| Command | `yard tunnel [site]` | `yard tunnel [site] --host <hostname>` |
| Cloudflare account | none | yes, with the zone of `<hostname>`, and `cloudflared tunnel login` done |
| URL | random `https://<words>.trycloudflare.com`, new on every start | `https://<hostname>`, stable |
| Lives in | yardd's memory only | `sites.toml` |
| After yardd restarts | gone | comes back |
| Cloudflare objects | none | a tunnel and a DNS CNAME |
| Cleanup | stop the unit | stop the unit, delete the tunnel, the user deletes the CNAME |

- **Quick tunnels are runtime state, not config (builder's call).** Their URL changes on
  every start, so restarting one after yardd restarts would bring back a URL nobody has.
  They are held in `State.quick_tunnels` and reconcile adds their units like any other.
  Cloudflare documents quick tunnels as for testing, without uptime guarantees and rate
  limited; that is fine for "show this to someone now".
- **One named tunnel per site (builder's call).** `yard tunnel --host` creates a Cloudflare
  tunnel named `yard-<site>` unless `--tunnel <name|uuid>` names an existing one to adopt
  (one made by hand, say). One tunnel per site keeps failure, restart and teardown per site,
  and maps one-to-one onto a unit. The alternative, one tunnel per machine with an ingress
  rule per hostname, is open question 3.
- A site has at most one tunnel, and a named tunnel serves at most one hostname in v1
  **(builder's call)**. Several hostnames for one site can come later as a list.
- Tunnels work for linked (PHP) sites and proxied sites alike.

## 5. CLI

Style follows the existing commands (`secure`/`unsecure`, `isolate`/`unisolate`): the site
is positional and defaults to the site containing the current directory. Every command has
`--help` and `--json`; human list output ends with the agents line; exit codes are the
standard 0/1/2/3.

```
yard tunnel [SITE] [--host HOSTNAME] [--tunnel NAME|UUID] [--overwrite-dns]
yard untunnel [SITE] [--keep-tunnel]
yard tunnels
```

`--help` text (first lines):

- `tunnel`: "Share a site at a public URL through a cloudflared tunnel. Without --host,
  a quick tunnel on trycloudflare.com that lasts until yardd stops; with --host, a named
  tunnel on your Cloudflare zone that stays until yard untunnel."
- `untunnel`: "Stop sharing a site. For a named tunnel, also deletes the Cloudflare tunnel
  Yard created; the DNS record is left for you to delete."
- `tunnels`: "List the sites shared through tunnels, with their public URLs and state."

`tunnel` and `untunnel` are jobs (cloudflared talks to Cloudflare and can take seconds);
the CLI streams their progress like `yard php install`, and Ctrl-C cancels.

### Output

Quick tunnel:

```
$ yard tunnel
Starting a quick tunnel for blog…
Shared blog → https://example-words-here.trycloudflare.com
  Anyone with this URL reaches blog until yardd stops or you run yard untunnel.
  For the app: APP_URL=https://example-words-here.trycloudflare.com
```

Named tunnel:

```
$ yard tunnel --host blog.example.com
Creating the Cloudflare tunnel yard-blog…
Routing blog.example.com to it…
Shared blog → https://blog.example.com (named tunnel yard-blog)
  It stays up whenever yardd runs, until yard untunnel.
  For the app: APP_URL=https://blog.example.com
```

Teardown:

```
$ yard untunnel
Stopped the tunnel for blog and deleted the Cloudflare tunnel yard-blog.
The DNS record is still there. Delete the CNAME blog.example.com
(→ 1b2c….cfargotunnel.com) in the Cloudflare dashboard, under the zone's DNS records;
until then visitors get a Cloudflare error page. yard doctor reminds you until it is gone.
```

An adopted tunnel (`--tunnel`) is never deleted; the message says it was kept.
`--keep-tunnel` keeps a Yard-created one too.

List:

```
$ yard tunnels
SITE   URL                                       KIND   STATE       TUNNEL
blog   https://blog.example.com                  named  ready       yard-blog
shop   https://example-words.trycloudflare.com   quick  connecting  -

For agents and scripts: yard tunnels --json
```

Empty: `No tunnels. Share a site with yard tunnel, run inside it.`

STATE is the unit's state, refined by `/ready`: `connecting` (running, not yet connected to
Cloudflare's edge), `ready`, `backoff`, `failed`, `stopped`.

### `--json`

`yard tunnels --json` prints `Vec<TunnelInfo>`; `tunnel --json` the `TunnelInfo` it made;
`untunnel --json` a `TunnelRemoved`. Shapes are in §12.

## 6. Where tunnel config lives

### User intent: `sites.toml`

A named tunnel is a `tunnel` table on the site's entry **(builder's call)**. `sites.toml`
already holds per-site intent (path, proxy, php, secure), and a tunnel belongs to a site and
goes when the site goes.

```toml
[sites.blog]
path = "/Users/me/code/blog"

[sites.blog.tunnel]
host = "blog.example.com"
# Optional: adopt an existing Cloudflare tunnel instead of creating yard-blog.
# tunnel = "blog-shared"
```

- `host` is required, a lowercase DNS name with at least two labels, validated at parse
  (`CONFIG_INVALID` at its span otherwise). Two sites with the same `host` is
  `CONFIG_INVALID` at the later one.
- `tunnel` is a tunnel name or UUID. Absent means `yard-<site>`.
- Written only by `TomlFile::edit` from the `tunnels.add`/`tunnels.remove` jobs. A hand-added
  table works too: reconcile runs it once its facts (below) exist, and otherwise reports
  `TUNNEL_NOT_FOUND` with the hint `yard tunnel <site> --host <host>`.

### Facts Yard learned: `state/tunnels.json`

Machine-written, not user config (like `state/hashes.json`):

```json
{
  "blog": {
    "id": "1b2c…-uuid",
    "name": "yard-blog",
    "created_by_yard": true,
    "routed": ["blog.example.com"]
  },
  "dns_left": [
    { "host": "old.example.com", "target": "9f8e….cfargotunnel.com", "since": "2026-11-02" }
  ]
}
```

- `id` is the tunnel UUID from `cloudflared tunnel create` or `tunnel list -o json`. If the
  file is lost, Yard recovers it with `tunnel list --name <name> -o json` (needs the login).
- `created_by_yard` decides whether `untunnel` deletes the tunnel.
- `dns_left` is the record list `yard doctor` warns about (§8).

### Generated: `tunnels/<site>.yml`

One cloudflared config per tunnel, rendered by `yard-core`, written by reconcile through
`generated::plan` with the usual header (`# Generated by Yard — edit sites.toml instead`)
and hash. Yard always passes `--config` so cloudflared never falls back to a config in
`~/.cloudflared/config.yml` or `/etc/cloudflared` that the user made for something else.

Named:

```yaml
# Generated by Yard — edit sites.toml instead
tunnel: 1b2c…-uuid
credentials-file: "/Users/me/.cloudflared/1b2c…-uuid.json"
metrics: 127.0.0.1:33012
no-autoupdate: true
loglevel: info
ingress:
  - hostname: blog.example.com
    service: http://127.0.0.1:33011
  - service: http_status:404
```

Quick:

```yaml
# Generated by Yard — edit sites.toml instead
url: http://127.0.0.1:33013
metrics: 127.0.0.1:33014
no-autoupdate: true
loglevel: info
```

- The catch-all `http_status:404` means a hostname someone else points at this tunnel gets
  a 404, not the site.
- `credentials-file` is cloudflared's own default path; Yard writes the path, never the
  file, and never reads it **(builder's call: secrets stay out of the data root and its
  backups)**.
- `loglevel: info` always. At `debug` cloudflared logs request headers, cookies included,
  into `logs/tunnel-<site>.log`.
- cloudflared has no include, so this file has no user-owned sibling **(builder's call)**.
  Hand edits are handled by the write rule (kept and reported, or backed up). Request-level
  customisation goes in the nginx override below.

### Generated nginx, and a tunnel-only override

The tunnel's nginx server block (§9) is appended to the site's own `nginx/sites/<site>.conf`
so it goes with the site. It includes the site's override `nginx/overrides/<site>.conf`, so
the site behaves the same in both places, and then a second user-owned file,
`nginx/tunnel-overrides/<site>.conf`, created empty by `ensure_user_file` on first tunnel
and never written again **(builder's call)**. That is where `auth_basic` or an IP allowlist
for the public side alone goes. A separate directory, not `<site>.tunnel.conf`, because site
names may contain dots. yardd watches it as it watches `nginx/overrides/`.

### Data layout additions (main spec §3.3)

```
tunnels/<site>.yml                  # generated
nginx/tunnel-overrides/<site>.conf  # USER-OWNED, included in the tunnel's server block
state/tunnels.json                  # tunnel ids, who created them, DNS records left behind
logs/tunnel-<site>.log              # the unit's log, rotated like the rest
```

## 7. How cloudflared runs

- One unit per tunnel, named `tunnel-<site>` (like `php-fpm-8.4`) **(builder's call)**.
- Program: `<prefix>/opt/cloudflared/bin/cloudflared`, resolved by `Brew` like nginx. A
  missing keg is `FORMULA_MISSING` with hint `brew install cloudflared` (`brew reinstall
  cloudflared` for an empty keg, per the homebrew rule).
- Args: named `tunnel --config <yml> run`; quick `tunnel --config <yml>` (the `url` key
  makes it a quick tunnel). The plan verifies on the installed cloudflared that a quick
  tunnel starts from `url` in the config file, and falls back to `--url` on the command
  line if not.
- Env: `HOME` (cloudflared finds `~/.cloudflared/cert.pem` for management commands), and
  PATH from `shim::child_path`. Nothing else from the caller.
- Stop: SIGTERM, which cloudflared handles with a grace period; SIGKILL after the supervisor's
  stop timeout. The plan checks cloudflared's default `grace-period` (30 s) against Yard's 5 s
  stop timeout and sets `grace-period` in the yml to fit.
- Reload: `Restart`. cloudflared does not reload a local config. A changed yml restarts the
  unit; quick tunnels then get a new URL, which is why nothing a quick tunnel depends on
  changes during normal reconciles (its yml depends only on its two ports).
- **Health: HTTP `GET /ready` on the metrics port, 200 = ready (builder's call).** This adds
  `Health::Http { port, path }` to the supervisor. A TCP connect would only prove the
  metrics server is up, not that the tunnel reached Cloudflare. cloudflared keeps retrying
  the edge itself, so a running unit that never becomes ready stays `Starting` until
  `start_grace`, then counts as a failure and backs off; `TunnelInfo.state` shows
  `connecting` meanwhile. The known nginx flake with HTTP health checks (main spec §6.1)
  came from inherited privileged sockets, which cloudflared does not have.
- Quick tunnel URL: read from the metrics server's `/quicktunnel` endpoint (JSON with the
  hostname), with the log line naming `trycloudflare.com` as a fallback. The plan verifies
  the endpoint on the installed version before relying on it.
- Management commands (`tunnel create`, `list -o json`, `route dns`, `delete`) run inside
  jobs through `JobCtx::run`, streaming output as `job_progress`, with SIGTERM/SIGKILL on
  cancel like brew jobs.
- `yard stop` and yardd shutdown stop tunnel units with the rest. Named tunnels come back
  on `yard start` because they are config; quick tunnels do not.
- Logging into Cloudflare stays the user's own step: Yard's hint is
  `cloudflared tunnel login`, which opens a browser **(builder's call; no `yard tunnel
  login` wrapper in v1)**.

## 8. DNS routing and cleanup

Adding (`tunnels.add` with a host):

1. Refuse early when `~/.cloudflared/cert.pem` does not exist (existence check only):
   `TUNNEL_NOT_LOGGED_IN`, hint `cloudflared tunnel login`.
2. `cloudflared tunnel list --name <name> -o json`. With no `--tunnel`, an existing tunnel
   named `yard-<site>` that `state/tunnels.json` does not record as Yard's is refused:
   `TUNNEL_EXISTS`, hint `yard tunnel <site> --host <host> --tunnel yard-<site>` to adopt it.
   With `--tunnel`, a missing one is `TUNNEL_NOT_FOUND`.
3. Create it if needed (`tunnel create yard-<site>`); record id and `created_by_yard`.
   Check that the credentials file now exists; if not, `TUNNEL_CREDENTIALS_MISSING`.
4. `cloudflared tunnel route dns <id> <host>`, **without `-f` (builder's call)**. If a record
   exists, cloudflared refuses; Yard reports `TUNNEL_DNS_EXISTS` with hint
   `yard tunnel <site> --host <host> --overwrite-dns`, which passes `-f`. Any other failure
   (zone not in this account, for instance) is `TUNNEL_DNS_FAILED` with cloudflared's output
   in `data.output`. Record the host in `routed`.
5. Write `[sites.<site>.tunnel]` with `TomlFile::edit`, reconcile, and wait up to 30 s for
   `/ready`. A timeout is `TUNNEL_NOT_READY`; the config stays, and the unit keeps trying.
6. A failure after step 3 deletes a tunnel this job created, so nothing is left half made.
   A record created in step 4 cannot be deleted, so it moves to `dns_left`.

Removing (`tunnels.remove`):

1. Remove the `tunnel` table, reconcile (the unit stops, the yml and nginx block go).
2. If `created_by_yard` and not `--keep-tunnel`: `cloudflared tunnel cleanup <id>`, then
   `tunnel delete <id>`. A failure here warns and keeps the id in state, so a later
   `yard untunnel` (or doctor's hint) can retry; the site stops being shared either way.
3. Move every `routed` host to `dns_left` with the record's target, and print it.

Cleanup of DNS is the user's, because cloudflared cannot delete records and
Yard does not hold a Cloudflare API token (open question 2). What Yard does to make that easy:

- The exact record (name and target) in the output and in `TunnelRemoved.dns_left`.
- A `yard doctor` warning per `dns_left` entry. Doctor resolves the host with the system
  resolver; once it no longer resolves (NXDOMAIN), the entry is dropped from state
  **(builder's call: the user deletes the record and the warning clears itself, no
  acknowledge command)**. A record repointed at another tunnel still resolves; for that,
  `yard tunnels forget-dns <host>` is a possible follow-up, not in v1.

## 9. Host header and HTTPS between cloudflared and nginx

Sites answer at `<site>.yard` on 8080/8443, behind Yard's own CA. The public hostname is
something else, and the local CA means nothing to anyone outside this Mac. Options:

- **A. cloudflared to `https://127.0.0.1:8443`**, with `originRequest.httpHostHeader:
  <site>.yard`, `originServerName: <site>.yard` and `caPool: ca/yard-ca.pem`. It reuses the
  existing listener, but needs the site secured, ties the tunnel to the TLD (a TLD change
  breaks it), and the app sees `Host: <site>.yard`, so every URL it generates points at a
  name visitors cannot reach.
- **B. A dedicated loopback listener per tunnel (chosen, builder's call).** nginx gets one
  more server block for a tunnelled site, on its own registry port, which accepts any Host.
  cloudflared's origin is `http://127.0.0.1:<port>`. The public hostname arrives unchanged.

B, because:

- The app sees the real public `Host`, so redirects, `url()` and absolute links are right
  without forcing anything.
- Only that site is reachable through the tunnel: the listener serves one site, and the
  default server and the site list are not on it (anti-goal 1).
- It is independent of the TLD, the port profile and whether the site is secured. A
  secured site's http→https redirect does not apply to it.
- TLS terminates at Cloudflare's edge; cloudflared to the edge is encrypted. The hop from
  cloudflared to nginx is plain HTTP on loopback, like nginx to php-fpm. The local CA plays
  no part, and the main spec's HTTPS (§5.4) is unchanged.

The block, appended to `nginx/sites/<site>.conf`:

```nginx
# Tunnel listener: only cloudflared (tunnel-blog) connects here.
server {
    listen 127.0.0.1:33011;
    server_name _;
    client_max_body_size 1G;
    set_real_ip_from 127.0.0.1;
    real_ip_header CF-Connecting-IP;

    # … the site's body, as for its http block, with for PHP:
    #   fastcgi_param HTTPS on;  fastcgi_param SERVER_PORT 443;
    # and for a proxy:
    #   X-Forwarded-Proto https, X-Forwarded-Host $host, X-Forwarded-For the visitor

    include ".../nginx/overrides/blog.conf";
    include ".../nginx/tunnel-overrides/blog.conf";
}
```

- `listen 127.0.0.1` only; cloudflared dials `127.0.0.1`, so `::1` is not needed.
- brew's nginx is built `--with-http_realip_module` (checked 2026-10-21 with `nginx -V`), so
  `$remote_addr` becomes the visitor's IP from `CF-Connecting-IP`, trusted only from
  loopback.
- `HTTPS on` for PHP is true: the visitor's connection is HTTPS at the edge.
- The plan checks how cloudflared sets `X-Forwarded-For` and `X-Forwarded-Proto` so the
  proxy branch neither duplicates nor drops the visitor's address.
- In the privileged profile nginx still binds this port itself, as the user; only 80/443
  come from the helper.

Rejected: an nginx unix socket per tunnel (cloudflared supports `unix:` origins). It skips
the registry, but socket paths under `~/Library/Application Support/Yard/run/` plus a site
name of up to 63 characters exceed macOS's 104-byte limit, and the space in the path is one
more quoting hazard in cloudflared's YAML.

## 10. What a tunnelled app needs

Yard never writes `.env`, and does not read it either **(builder's call: a project's `.env`
holds secrets, and nothing below needs it)**. Yard prints and documents; the app owner sets.

- **PHP sites (Laravel and others): no trusted-proxy setup needed.** `HTTPS=on`, the real
  Host and the real `REMOTE_ADDR` arrive as if nginx were the edge. An app that already
  trusts `*` proxies still works.
- **Proxied sites (Node, Vite and the like)** get `X-Forwarded-Proto`, `X-Forwarded-Host`
  and `X-Forwarded-For` from nginx on loopback; the app must trust `127.0.0.1` as a proxy
  (Express `trust proxy`, for instance). Vite's dev server refuses unknown hosts by default:
  add the public host to `server.allowedHosts`. `yard tunnel` prints that hint for a proxy
  site.
- **`APP_URL`** still matters for URLs made outside a request: queued mail, notifications,
  console commands, and signed URLs (signed with one host, checked against another, they
  fail). `yard tunnel` prints `APP_URL=https://<host>`; the M4 `yard env --site` prints it
  too while the site is tunnelled.
- **Vite hot file**: a `public/hot` file makes Laravel point assets at the local dev server,
  which visitors cannot reach. Doctor warns when a tunnelled linked site's docroot has one
  (a file-exists check).
- **Sessions and cookies**: a `SESSION_DOMAIN` set to `.yard` stops logins on the public
  host; Sanctum's `SANCTUM_STATEFUL_DOMAINS` needs the public host for SPA auth.
- **Debug output is public.** Yard's php.ini has `display_errors=On`, and a dev `APP_DEBUG`
  shows stack traces and config to anyone with the URL. `yard tunnel` says so once, in its
  output. There is no production ini profile: tunnels share a dev site (§1).
- **Xdebug**: with `start_with_request=trigger`, a visitor can send the trigger and open a
  debug session to the developer's IDE. The docs say so; Yard does not change xdebug's ini.
- **Cloudflare limits**: 100 MB request bodies on the free plan (uploads past that fail at
  the edge), and a 100 s origin timeout (a request held at a breakpoint gets a 524).
  Universal SSL covers `example.com` and `*.example.com` only, so `a.b.example.com` gets no
  edge certificate without a paid certificate; `yard tunnel` warns when the host is two or
  more labels below a guessed zone, and the failure mode is documented.

## 11. Port registry

Each tunnel adds two `auto` entries, allocated from `range` and saved in `ports.toml` like
`mailpit-http` **(builder's call)**:

```toml
nginx-tunnel-blog = { port = 33011, mode = "auto" }   # nginx's tunnel listener
tunnel-blog       = { port = 33012, mode = "auto" }   # cloudflared's metrics and /ready
```

- `port_owner` maps `nginx-*` to the nginx unit, as it does `nginx-http`, so the nginx
  listener is `yard` status while nginx runs; `tunnel-<site>` is owned by the unit of that
  name.
- Allocation goes through `Registry::ensure` with the usual `is_free` probe, so a taken
  port moves to another free one in the range. Entries for tunnels that no longer exist
  are dropped, like stale `proxy:` entries.
- The cloudflared unit's pre-start probe is its metrics port (TCP). The nginx listener is
  probed at allocation; a foreign listener found later shows in the 30 s scan as
  `port.conflict`, like any registered port.
- Outbound connections (QUIC on UDP 7844, HTTP/2 fallback on TCP 7844) bind nothing and are
  not in the registry.
- Quick tunnels get entries too, removed when the quick tunnel ends. They change
  `ports.toml`, which is the price of one rule for every listener.

## 12. RPC protocol

Additive, so the protocol minor version goes up. Methods (main spec §6.2 gets them when
this is built):

| Method | Params | Result |
|---|---|---|
| `tunnels.list` | none | `Vec<TunnelInfo>` |
| `tunnels.add` | `{ site, host?, tunnel?, overwrite_dns? }` | `JobStarted`; the job's result is a `TunnelInfo` |
| `tunnels.remove` | `{ site, keep_tunnel? }` | `JobStarted`; the job's result is a `TunnelRemoved` |

`job_finished` carries no result today, so jobs that produce one need a `result: Option<Value>`
field on `JobFinished` (additive) **(builder's call)**; the CLI prints it.

Types (in `yard-proto`, used unchanged by `--json`):

```rust
pub enum TunnelKind { Quick, Named }

pub struct TunnelInfo {
    pub site: String,
    pub kind: TunnelKind,
    pub url: Option<String>,          // None while a quick tunnel has no hostname yet
    pub host: Option<String>,         // named only
    pub tunnel: Option<TunnelRef>,    // named only
    pub state: TunnelState,           // connecting | ready | backoff | failed | stopped
    pub unit: String,                 // "tunnel-<site>"
    pub origin_port: u16,
    pub metrics_port: u16,
    pub last_error: Option<String>,
}

pub struct TunnelRef { pub name: String, pub id: String, pub created_by_yard: bool }

pub struct TunnelRemoved {
    pub site: String,
    pub deleted_tunnel: bool,
    pub dns_left: Vec<DnsRecord>,     // { host, target }
}
```

Events: a `tunnels` topic with `tunnel_changed { tunnel: TunnelInfo }`, emitted when a
tunnel's state or URL changes, so Yard.app (M6) shows a quick tunnel's URL the moment it
exists. `unit_state_changed` still fires for the unit.

New error codes (stable, each with a hint that is the command that fixes it):

| Code | When | Hint |
|---|---|---|
| `TUNNEL_NOT_LOGGED_IN` | a named tunnel needs Cloudflare and `cert.pem` is missing | `cloudflared tunnel login` |
| `TUNNEL_EXISTS` | `yard-<site>` exists in the account but Yard did not make it | `yard tunnel <site> --host <host> --tunnel yard-<site>` |
| `TUNNEL_NOT_FOUND` | the site has no tunnel, or `--tunnel` names none | `yard tunnel <site>` |
| `TUNNEL_CREDENTIALS_MISSING` | the tunnel's credentials file is gone | `yard untunnel <site> && yard tunnel <site> --host <host>` |
| `TUNNEL_DNS_EXISTS` | a record for the host already exists | `yard tunnel <site> --host <host> --overwrite-dns` |
| `TUNNEL_DNS_FAILED` | `route dns` failed otherwise; `data.output` | check the zone is in this account |
| `TUNNEL_NOT_READY` | no `/ready` within 30 s; `data.tail` has the log | `yard logs tunnel-<site>` (or the log path until M4 adds logs) |
| `SITE_TUNNELLED` | `unlink`/`unproxy` of a tunnelled site | `yard untunnel <site>` |

`SITE_TUNNELLED` **(builder's call)**: unlinking silently would leave a Cloudflare tunnel and
a DNS record behind, so it refuses and points at the command that cleans up. A site already
having a different tunnel is `INVALID_ARGUMENT` (hint `yard untunnel <site>` first). A
missing `cloudflared` reuses `FORMULA_MISSING`.

## 13. `yard doctor`

New checks, only when at least one tunnel is configured or running:

| id | ok | warn / fail |
|---|---|---|
| `cloudflared` | brew's cloudflared is at its keg | fail: `brew install cloudflared` |
| `tunnel-login` | `~/.cloudflared/cert.pem` exists (stat only) | warn when named tunnels exist: `cloudflared tunnel login` (running needs no login; changing does) |
| `tunnel:<site>` | unit ready, `/ready` 200 | fail with the unit's last error; warn while `connecting` |
| `tunnel-credentials:<site>` | credentials file exists (stat only) | fail: `TUNNEL_CREDENTIALS_MISSING` hint |
| `tunnel-dns:<host>` | — | warn per `dns_left` record still resolving: "delete the CNAME <host> → <target> in the Cloudflare dashboard" |
| `tunnel-hot:<site>` | — | warn when the docroot has `public/hot` |
| `generated` (existing) | covers `tunnels/<site>.yml` | as now |
| `ports` (existing) | covers the two entries per tunnel | as now |

Doctor never contacts Cloudflare and never requests the public URL **(builder's call)**:
the offline `yard doctor` should not depend on the network, and the local checks find the
failures Yard can fix. The DNS lookup for `dns_left` is the one network read, and a lookup
that fails for lack of network leaves the entry in place.

## 14. Failure modes

| Failure | What happens |
|---|---|
| cloudflared not installed | `tunnel` refused with `FORMULA_MISSING`; configured named tunnels' units fail without retry (a missing binary is fatal, like a missing helper). |
| Not logged in | `tunnel --host` refused (`TUNNEL_NOT_LOGGED_IN`); existing tunnels keep running, since `run` needs only the credentials file. |
| Credentials file deleted | unit fails fatally with `TUNNEL_CREDENTIALS_MISSING`; doctor names the fix. |
| Tunnel deleted in the dashboard | cloudflared exits with an auth error; backoff, then `failed`; the error names the tunnel; hint is untunnel and tunnel again. |
| No network, sleep, Wi-Fi change | cloudflared reconnects by itself; `/ready` fails meanwhile and state is `connecting`. Repeated failures past `start_grace` back off and may reach `failed`; `yard restart tunnel-<site>` resets. The plan tunes `start_grace` for tunnels so a laptop waking up does not end in `failed`. |
| UDP 7844 blocked | cloudflared falls back from QUIC to HTTP/2 itself (`protocol: auto`). |
| Quick tunnel rate-limited or refused | the add job fails `TUNNEL_NOT_READY` with the log tail; nothing persists. |
| DNS record exists | `TUNNEL_DNS_EXISTS`; nothing overwritten. |
| Zone not in the account | `TUNNEL_DNS_FAILED`; the tunnel this job created is deleted again. |
| Port taken | the `auto` entry moves; a probe failure on start goes to `Failed` with the holder, as for every unit. |
| Hand-edited `tunnels/<site>.yml` | write rule: kept and reported, or backed up when the render changes. |
| nginx config rejected | the whole write set rolls back (`NGINX_CONFIG_INVALID`) and the tunnel is not added. |
| yardd stops or crashes | tunnels drop with it; visitors get Cloudflare's error page; named tunnels return on start. |
| Site or its `tunnel` table removed by hand in `sites.toml` | reconcile stops the unit and drops the nginx block, and warns that the Cloudflare tunnel and DNS record remain. Its facts stay in `state/tunnels.json`; doctor reports the leftover tunnel (hint `cloudflared tunnel delete <id>`) and its record, as in §8. |
| TLD or profile change | no effect on tunnels (§9). |
| `brew upgrade cloudflared` | the running unit keeps the old binary until it restarts; nothing breaks. |

## 15. Milestone

**M5 Tunnels**, after M4 Node and services and before M6 Yard.app (the maintainer's order,
2026-10-21). It needs only what M1–M3 built (units, registry, reconcile, jobs, nginx
rendering) plus the `Health::Http` check and `JobFinished.result` above. Landing it before
the app means the app's generated models include tunnels from the start. It gets its own
plan in `docs/superpowers/plans/`, and an `scripts/e2e-m5.sh` that runs a quick tunnel
against a scratch site and curls the trycloudflare URL; it needs the network and no
Cloudflare account, so ask the maintainer first. A named-tunnel e2e needs his zone and login and is a
manual step.

When built, the same change updates: main spec §3.3 (data layout), §5 (a tunnels section),
§6.1 (HTTP health), §6.2 (methods, topic), §6.3 (codes), §7 (tunnel entries), §12; the
README; `AGENTS.md`; and the cli-output, port-registry, config-ownership and homebrew rules.

## 16. Open questions for the maintainer

Most important first. Each answer changes what gets built.

1. ~~Sharing a dev site, or running apps in production?~~ Answered by the maintainer on
   2026-10-21: sharing only, publicly or with whoever has the link, for webhooks and similar
   (§1).
2. **May Yard hold a Cloudflare API token to delete DNS records?** cloudflared cannot delete
   them. With a token (scoped to DNS edit, stored in the login keychain, never in a file),
   `yard untunnel` could clean up fully. Without it, the user deletes the CNAME by hand and
   doctor reminds them. This design assumes no token.
3. **One Cloudflare tunnel per site, or one per machine?** Per site (this design) isolates
   failures and maps onto units, but makes one tunnel and one credentials file per site.
   One per machine, with an ingress rule per hostname, is Cloudflare's usual model, fewer
   objects in the account, but every change restarts every tunnel.
4. **Should named tunnels start at login?** As designed, they come back whenever yardd
   starts, so a shared site is public whenever the Mac is on. If you would rather they
   start only on request, a named tunnel needs an `enabled` flag and `yard tunnel start|stop`.
5. **Is cloudflared the only provider for M5?** ngrok, Tailscale Funnel or Expose would each
   need their own unit and auth story; this design keeps one provider behind one module.
