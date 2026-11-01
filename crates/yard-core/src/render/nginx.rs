//! nginx.conf, one server block per site, and the default site list (spec §5.2).

use super::quote::{html, nginx_str};
use super::{RenderCtx, header};
use crate::PhpVersion;
use crate::emailulator::MAIL_HOST_LABEL;
use std::fmt::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use yard_proto::{Need, SiteInfo, SiteKind};

fn q(p: &Path) -> String {
    nginx_str(&p.display().to_string())
}

pub fn nginx_main(ctx: &RenderCtx) -> String {
    let paths = ctx.paths;
    let run = paths.run_dir();
    let temp = |name: &str| q(&run.join("nginx").join(name));
    let http = ctx.http_port;
    format!(
        "{header}daemon off;
worker_processes auto;
pid {pid};
error_log {error_log};

events {{
    worker_connections 1024;
}}

http {{
    include {mime};
    default_type application/octet-stream;
    access_log off;
    sendfile on;
    client_max_body_size 1G;
    server_names_hash_bucket_size 128;
    client_body_temp_path {body};
    proxy_temp_path {proxy};
    fastcgi_temp_path {fastcgi};
    uwsgi_temp_path {uwsgi};
    scgi_temp_path {scgi};

    map $http_upgrade $connection_upgrade {{
        default upgrade;
        ''      close;
    }}

    # Unknown hosts get the list of sites Yard serves.
    server {{
        listen 127.0.0.1:{http} default_server;
        listen [::1]:{http} default_server;
        server_name _;
        root {default_root};
        index index.html;

        location / {{
            try_files /index.html =404;
        }}
    }}

    # Unknown hosts on the https port are refused, not handed to the first secured site.
    server {{
        listen 127.0.0.1:{https} ssl default_server;
        listen [::1]:{https} ssl default_server;
        server_name _;
        ssl_reject_handshake on;
    }}

    include {sites};
    include {services};
}}
",
        header = header(&paths.nginx_dir().join("overrides")),
        pid = q(&paths.nginx_pid()),
        error_log = q(&paths.logs_dir().join("nginx-error.log")),
        mime = q(&ctx.brew.nginx_mime_types()),
        body = temp("client_body"),
        proxy = temp("proxy"),
        fastcgi = temp("fastcgi"),
        uwsgi = temp("uwsgi"),
        scgi = temp("scgi"),
        default_root = q(&paths.nginx_default_dir()),
        https = ctx.https_port,
        sites = q(&paths.nginx_sites_dir().join("*.conf")),
        services = q(&paths.nginx_services_dir().join("*.conf")),
    )
}

/// A site's server blocks. With `wake` (a PHP site whose `.yard` lists services), each PHP
/// request first asks yardd to wake the site's sleeping services (M4e spec §5.1).
pub fn nginx_site(ctx: &RenderCtx, site: &SiteInfo, wake: bool) -> String {
    let override_file = ctx.paths.nginx_override(&site.name);
    let host = &site.host;
    let body = match &site.kind {
        // Every request gets the page naming what the `.yard` needs, with a 503.
        SiteKind::Php { .. } if site.needs_setup() => format!(
            "    root {root};
    error_page 503 /{name}.html;

    location / {{
        return 503;
    }}

    location = /{name}.html {{
        internal;
    }}
",
            root = q(&ctx.paths.nginx_setup_dir()),
            name = site.name,
        ),
        SiteKind::Php { docroot, php, .. } => {
            let socket = php
                .parse::<PhpVersion>()
                .map(|v| ctx.paths.fpm_socket(v))
                .unwrap_or_else(|_| ctx.paths.fpm_socket(PhpVersion::DEFAULT));
            let (ask, wake_block) = if wake {
                (
                    "        auth_request /__yard_wake;\n",
                    format!("\n{}", wake_location(&ctx.paths.wake_socket(), &site.name)),
                )
            } else {
                ("", String::new())
            };
            format!(
                "    root {root};
    index index.php index.html;
    charset utf-8;

    location / {{
        try_files $uri $uri/ /index.php?$query_string;
    }}

    location ~ \\.php$ {{
{ask}        fastcgi_split_path_info ^(.+\\.php)(/.+)$;
        fastcgi_pass {pass};
        fastcgi_index index.php;
        include {params};
        fastcgi_param SCRIPT_FILENAME $realpath_root$fastcgi_script_name;
        fastcgi_param PATH_INFO $fastcgi_path_info;
        fastcgi_read_timeout 3600;
    }}

    location ~ /\\.(?!well-known).* {{
        deny all;
    }}
{wake_block}",
                root = nginx_str(docroot),
                pass = nginx_str(&format!("unix:{}", socket.display())),
                params = q(&ctx.brew.nginx_fastcgi_params()),
            )
        }
        SiteKind::Proxy { target } => proxy_location(target, "$host", ""),
    };
    let tls = site.secure.then(|| (ctx.paths.cert(&site.name), ctx.paths.cert_key(&site.name)));
    server_blocks(ctx, &format!("{host} *.{host}"), &body, &override_file, tls)
}

/// The subrequest a PHP location makes with `auth_request /__yard_wake;`: yardd's wake socket
/// answers once `site`'s services are up (at most 10 s, which `proxy_read_timeout` outlasts).
/// Any error, a missing socket and a timeout included, falls through to an empty `204`, so a
/// request never fails because yardd could not wake a service. The client's headers, cookies
/// included, and body stay out of the ask: yardd needs only the path. The whole URL is one quoted
/// argument: nginx rejects a quoted socket path inside `http://unix:…:`.
pub fn wake_location(wake_socket: &Path, site: &str) -> String {
    format!(
        "    location = /__yard_wake {{
        internal;
        proxy_pass {url};
        proxy_pass_request_headers off;
        proxy_pass_request_body off;
        proxy_set_header Content-Length \"\";
        proxy_read_timeout 15s;
        proxy_intercept_errors on;
        error_page 400 401 403 404 500 502 503 504 = @yard_wake_skipped;
    }}

    location @yard_wake_skipped {{
        return 204;
    }}
",
        url = nginx_str(&format!("http://unix:{}:/wake/{site}?wait=10", wake_socket.display())),
    )
}

/// The `location /` of a reverse proxy to `target`, which sends `host` as `Host` and keeps the
/// port the browser used in `X-Forwarded-Host` (`$http_host`; `$host` would drop it).
/// `extra` is more lines for the block, each ending in a newline.
fn proxy_location(target: &str, host: &str, extra: &str) -> String {
    format!(
        "    location / {{
        proxy_pass {target};
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host {host};
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host $http_host;
        proxy_read_timeout 3600;
{extra}    }}
",
        target = nginx_str(target),
    )
}

/// The http server block, or with `tls` (certificate and key) the redirect to https plus the
/// https block, each ending with the user's `override_file`.
fn server_blocks(
    ctx: &RenderCtx,
    server_name: &str,
    body: &str,
    override_file: &Path,
    tls: Option<(PathBuf, PathBuf)>,
) -> String {
    let http = ctx.http_port;
    let header = header(override_file);
    let override_file = q(override_file);
    let Some((cert, key)) = tls else {
        return format!(
            "{header}server {{
    listen 127.0.0.1:{http};
    listen [::1]:{http};
    server_name {server_name};
    client_max_body_size 1G;

{body}
    include {override_file};
}}
"
        );
    };
    // A secured host answers http with a redirect to the same path over https.
    let https = ctx.https_port;
    let port = if https == 443 { String::new() } else { format!(":{https}") };
    let (cert, key) = (q(&cert), q(&key));
    format!(
        "{header}server {{
    listen 127.0.0.1:{http};
    listen [::1]:{http};
    server_name {server_name};
    return 301 https://$host{port}$request_uri;
}}

server {{
    listen 127.0.0.1:{https} ssl;
    listen [::1]:{https} ssl;
    server_name {server_name};
    client_max_body_size 1G;
    ssl_certificate {cert};
    ssl_certificate_key {key};
    ssl_protocols TLSv1.2 TLSv1.3;

{body}
    include {override_file};
}}
"
    )
}

/// `nginx/services/<name>.conf`: `<name>.<tld>` proxied to a service's web console on
/// `upstream_port` (MinIO's, spec 5.4). It is not a site. `Host` is the upstream's own
/// address, which the console may check; the browser's host and port go in `X-Forwarded-Host`.
/// A secured block's certificate is `certs/services/<name>.pem`. The block ends with the
/// instance's `overrides.conf`.
pub fn service_site(ctx: &RenderCtx, name: &str, upstream_port: u16, secure: bool) -> String {
    let upstream = format!("127.0.0.1:{upstream_port}");
    let body = proxy_location(&format!("http://{upstream}"), &upstream, "");
    let certs = ctx.paths.services_certs_dir();
    let tls = secure.then(|| (certs.join(format!("{name}.pem")), certs.join(format!("{name}.key"))));
    server_blocks(ctx, &format!("{name}.{}", ctx.tld), &body, &ctx.paths.service_overrides(name), tls)
}

/// `mail.<tld>`: emailulator's web UI, whoever serves it (emailulator spec §10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailHost {
    /// `mail.<tld>`.
    pub host: String,
    /// emailulator's HTTP address, on loopback.
    pub upstream: SocketAddr,
    /// emailulator was started with `--tls-cert`.
    pub upstream_tls: bool,
    pub secure: bool,
}

/// `nginx/services/emailulator.conf`. Like a service's console it is not a site, and `Host`
/// is emailulator's own address: with no auth token its DNS-rebinding guard refuses any other,
/// and `server_name` does that guard's job here. Its event stream is not buffered, and the
/// body limit is emailulator's largest message. A secured block's certificate is
/// `certs/services/emailulator.pem`; the block ends with `nginx/overrides/mail.conf`.
pub fn mail_host(ctx: &RenderCtx, m: &MailHost) -> String {
    let scheme = if m.upstream_tls { "https" } else { "http" };
    let upstream = m.upstream.to_string();
    // The upstream is loopback and its certificate emailulator's own.
    let verify = if m.upstream_tls { "        proxy_ssl_verify off;\n" } else { "" };
    let extra = format!("        proxy_buffering off;\n        client_max_body_size 25m;\n{verify}");
    let body = proxy_location(&format!("{scheme}://{upstream}"), &upstream, &extra);
    let certs = ctx.paths.services_certs_dir();
    let tls = m.secure.then(|| (certs.join("emailulator.pem"), certs.join("emailulator.key")));
    let server_name = format!("{host} *.{host}", host = m.host);
    server_blocks(ctx, &server_name, &body, &ctx.paths.nginx_override(MAIL_HOST_LABEL), tls)
}

/// What a site whose `.yard` is not met says instead of serving the project.
fn setup_text(site: &SiteInfo) -> String {
    if let Some(e) = &site.project_error {
        let file = site.project_file.as_deref().unwrap_or(".yard");
        return format!("{}'s .yard does not parse: {}\nEdit {file} to fix it.\n", site.name, e.message);
    }
    let path = match &site.kind {
        SiteKind::Php { path, .. } => path.as_str(),
        SiteKind::Proxy { .. } => "",
    };
    let list = Need::describe_all(&site.needs);
    let mut text = format!("{} needs {list}.\n", site.name);
    // What Yard installs goes through yard init; the rest (emailulator) is the user's to run.
    let (installs, manual): (Vec<Need>, Vec<Need>) = site.needs.iter().cloned().partition(Need::installable);
    if !installs.is_empty() {
        let what = match (installs.len(), manual.is_empty()) {
            (_, false) => Need::describe_all(&installs),
            (1, true) => "it".to_string(),
            (_, true) => "them".to_string(),
        };
        let _ = writeln!(text, "Run yard init in {path} to install {what}.");
    }
    for need in &manual {
        let _ = writeln!(text, "Install {} with: {}", need.describe(), need.command());
    }
    text
}

/// `nginx/setup/<site>.html`, served with a 503 while the site's `.yard` is not met.
pub fn setup_page(ctx: &RenderCtx, site: &SiteInfo) -> String {
    let name = html(&site.name);
    format!(
        "<!-- Generated by Yard — edit {over} instead -->
<!doctype html>
<html lang=\"en\">
<head><meta charset=\"utf-8\"><title>{name} needs setup</title></head>
<body>
  <h1>{name} needs setup</h1>
  <pre>{text}</pre>
</body>
</html>
",
        over = html(&ctx.paths.nginx_override(&site.name).display().to_string()),
        text = html(&setup_text(site)),
    )
}

pub fn default_index(ctx: &RenderCtx, sites: &[SiteInfo]) -> String {
    let mut items = String::new();
    for s in sites {
        let _ = writeln!(items, "    <li><a href=\"{}\">{}</a></li>", html(&s.url), html(&s.host));
    }
    if sites.is_empty() {
        items.push_str(
            "    <li>No sites yet. Serve a project with <code>yard link</code>, run inside it.</li>\n",
        );
    }
    format!(
        "<!-- Generated by Yard — edit {sites_toml} instead -->
<!doctype html>
<html lang=\"en\">
<head><meta charset=\"utf-8\"><title>Yard</title></head>
<body>
  <h1>Yard</h1>
  <p>No site answers to this host name. Sites on .{tld}:</p>
  <ul>
{items}  </ul>
</body>
</html>
",
        sites_toml = html(&ctx.paths.sites_toml().display().to_string()),
        tld = html(ctx.tld),
    )
}
