//! nginx.conf, one server block per site, and the default site list (spec §5.2).

use super::quote::{html, nginx_str};
use super::{RenderCtx, header};
use crate::PhpVersion;
use std::fmt::Write;
use std::path::Path;
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
    )
}

pub fn nginx_site(ctx: &RenderCtx, site: &SiteInfo) -> String {
    let override_file = ctx.paths.nginx_override(&site.name);
    let http = ctx.http_port;
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
            format!(
                "    root {root};
    index index.php index.html;
    charset utf-8;

    location / {{
        try_files $uri $uri/ /index.php?$query_string;
    }}

    location ~ \\.php$ {{
        fastcgi_split_path_info ^(.+\\.php)(/.+)$;
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
",
                root = nginx_str(docroot),
                pass = nginx_str(&format!("unix:{}", socket.display())),
                params = q(&ctx.brew.nginx_fastcgi_params()),
            )
        }
        SiteKind::Proxy { target } => format!(
            "    location / {{
        proxy_pass {target};
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host $host;
        proxy_read_timeout 3600;
    }}
",
            target = nginx_str(target),
        ),
    };
    if !site.secure {
        return format!(
            "{header}server {{
    listen 127.0.0.1:{http};
    listen [::1]:{http};
    server_name {host} *.{host};
    client_max_body_size 1G;

{body}
    include {override_file};
}}
",
            header = header(&override_file),
            override_file = q(&override_file),
        );
    }
    // A secured site answers http with a redirect to the same path over https.
    let https = ctx.https_port;
    let port = if https == 443 { String::new() } else { format!(":{https}") };
    format!(
        "{header}server {{
    listen 127.0.0.1:{http};
    listen [::1]:{http};
    server_name {host} *.{host};
    return 301 https://$host{port}$request_uri;
}}

server {{
    listen 127.0.0.1:{https} ssl;
    listen [::1]:{https} ssl;
    server_name {host} *.{host};
    client_max_body_size 1G;
    ssl_certificate {cert};
    ssl_certificate_key {key};
    ssl_protocols TLSv1.2 TLSv1.3;

{body}
    include {override_file};
}}
",
        header = header(&override_file),
        override_file = q(&override_file),
        cert = q(&ctx.paths.cert(&site.name)),
        key = q(&ctx.paths.cert_key(&site.name)),
    )
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
    let them = if site.needs.len() > 1 { "them" } else { "it" };
    format!("{} needs {list}.\nRun yard init in {path} to install {them}.\n", site.name)
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
