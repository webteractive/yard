//! `yard tunnel`, `untunnel` and `tunnels` (tunnels spec §5): sharing a site at a public URL.
//! The choice between a quick and a named tunnel is made here, never in yardd (§4.1).

use super::sites::containing;
use super::{Ctx, JobEnd, Outcome, cwd, interactive, wait_job};
use crate::client::{CliError, Client};
use crate::output::{for_agents, print_error, print_json_line, table};
use serde_json::Value;
use std::io::{BufRead, Write};
use yard_core::tunnel::domain_for;
use yard_proto::methods::{
    self, TunnelAddParams, TunnelDomainAddParams, TunnelDomainParams, TunnelRemoveParams,
};
use yard_proto::{
    ErrorCode, SiteInfo, SiteKind, TunnelDomain, TunnelInfo, TunnelKind, TunnelRemoved, YardError,
};

pub struct TunnelArgs {
    pub site: Option<String>,
    pub quick: bool,
    pub host: Option<String>,
    pub provider: Option<String>,
    pub tunnel: Option<String>,
    pub overwrite_dns: bool,
}

/// The site named, or the one containing the current directory.
async fn site_or_here(client: &mut Client, site: Option<String>, command: &str) -> Result<String, CliError> {
    match site {
        Some(s) => Ok(s),
        None => {
            let sites: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
            Ok(containing(&sites, &cwd()?, &format!("yard {command} <site>"))?)
        }
    }
}

pub async fn tunnel(ctx: &Ctx, args: TunnelArgs) -> Outcome {
    let mut client = ctx.client().await?;
    let sites: Vec<SiteInfo> = client.call(methods::SITES_LIST, Value::Null).await?;
    let site = match args.site {
        Some(s) => s,
        None => containing(&sites, &cwd()?, "yard tunnel <site>")?,
    };
    let proxied = sites.iter().any(|s| s.name == site && matches!(s.kind, SiteKind::Proxy { .. }));
    let domains: Vec<String> = if args.quick {
        vec![]
    } else {
        let listed: Vec<TunnelDomain> = client.call(methods::TUNNELS_DOMAINS, Value::Null).await?;
        listed.into_iter().map(|d| d.domain).collect()
    };
    let host = match (args.quick, args.host) {
        (_, Some(host)) => Some(host),
        (true, None) => None,
        (false, None) if domains.is_empty() => None,
        (false, None) if interactive(ctx) => choose(&site, &domains)?,
        (false, None) => {
            let e = YardError::new(
                ErrorCode::TunnelKindRequired,
                format!("{site} can be shared at a random URL or on a domain you set up; say which"),
            )
            .with_hint(format!("yard tunnel {site} --quick, or --host <name>"));
            print_error(ctx.json, &e);
            return Ok(2);
        }
    };
    let params = TunnelAddParams {
        site,
        provider: args.provider,
        host,
        tunnel: args.tunnel,
        overwrite_dns: args.overwrite_dns,
    };
    let info: TunnelInfo = match result(ctx, methods::TUNNELS_ADD, params).await? {
        Done::Made(info) => info,
        Done::Printed => return Ok(0),
        Done::Abandoned(code) => return Ok(code),
    };
    print!("{}", shared(&info, proxied, &domains));
    Ok(0)
}

/// The spec §4.1 question: a quick tunnel, `<site>.<domain>` for each domain, or another name.
/// `None` is a quick tunnel.
fn choose(site: &str, domains: &[String]) -> Result<Option<String>, YardError> {
    println!("Share {site} at:");
    println!("  1) a random URL on trycloudflare.com (until yardd stops)");
    for (i, domain) in domains.iter().enumerate() {
        println!("  {}) {site}.{domain}", i + 2);
    }
    let other = domains.len() + 2;
    println!("  {other}) another name on a domain you set up");
    let answer = ask("Choice [2]: ");
    let choice = if answer.is_empty() { 2 } else { answer.parse().unwrap_or(0) };
    match choice {
        1 => Ok(None),
        n if n == other => {
            let host = ask("Name: ").to_lowercase();
            match domain_for(&host, domains) {
                Some(_) => Ok(Some(host)),
                None => {
                    let parent = host.split_once('.').map_or(host.as_str(), |(_, rest)| rest);
                    Err(YardError::new(
                        ErrorCode::TunnelDomainNotSetUp,
                        format!("{host} is not under a domain you set up"),
                    )
                    .with_hint(format!("yard tunnels domain add {parent}")))
                }
            }
        }
        n if (2..other).contains(&n) => Ok(Some(format!("{site}.{}", domains[n - 2]))),
        _ => Err(YardError::new(ErrorCode::InvalidArgument, format!("{answer:?} is not one of the choices"))
            .with_hint(format!("yard tunnel {site} --quick, or --host <name>"))),
    }
}

/// A line read from stdin after `prompt`, trimmed; empty when nothing could be read.
fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    answer.trim().to_string()
}

/// What `yard tunnel` prints once the tunnel is up (spec §5).
fn shared(info: &TunnelInfo, proxied: bool, domains: &[String]) -> String {
    let url =
        info.url.clone().or_else(|| info.host.as_ref().map(|h| format!("https://{h}"))).unwrap_or_default();
    let host = url.strip_prefix("https://").unwrap_or(&url);
    let mut out = match (&info.kind, &info.tunnel) {
        (TunnelKind::Named, Some(t)) => format!(
            "Shared {} → {url} (named tunnel {})\n  It stays up whenever yardd runs, until yard untunnel.\n",
            info.site, t.name
        ),
        _ => format!(
            "Shared {site} → {url}\n  Anyone with this URL reaches {site} until yardd stops or you run yard untunnel.\n",
            site = info.site
        ),
    };
    out.push_str(&format!("  For the app: APP_URL={url}\n"));
    if proxied {
        out.push_str(&format!(
            "  Vite and similar dev servers refuse unknown hosts: add {host} to server.allowedHosts.\n"
        ));
    }
    out.push_str("  Error pages and debug output are public too.\n");
    if let Some(domain) = info.host.as_deref().and_then(|h| domain_for(h, domains)) {
        let below = host.strip_suffix(domain).and_then(|rest| rest.strip_suffix('.')).unwrap_or_default();
        if below.contains('.') {
            out.push_str(&format!(
                "  Cloudflare's free certificate covers {domain} and *.{domain} only; {host} needs an advanced certificate.\n"
            ));
        }
    }
    out
}

pub async fn untunnel(ctx: &Ctx, site: Option<String>, keep_tunnel: bool) -> Outcome {
    let mut client = ctx.client().await?;
    let site = site_or_here(&mut client, site, "untunnel").await?;
    let tunnels: Vec<TunnelInfo> = client.call(methods::TUNNELS_LIST, Value::Null).await?;
    let name = tunnels.into_iter().find(|t| t.site == site).and_then(|t| t.tunnel).map(|t| t.name);
    let removed: TunnelRemoved =
        match result(ctx, methods::TUNNELS_REMOVE, TunnelRemoveParams { site, keep_tunnel }).await? {
            Done::Made(removed) => removed,
            Done::Printed => return Ok(0),
            Done::Abandoned(code) => return Ok(code),
        };
    let mut out = match (name, removed.deleted_tunnel) {
        (Some(name), true) => {
            format!("Stopped the tunnel for {} and deleted the Cloudflare tunnel {name}.\n", removed.site)
        }
        (Some(name), false) => {
            format!("Stopped the tunnel for {} and kept the Cloudflare tunnel {name}.\n", removed.site)
        }
        (None, _) => format!("Stopped the tunnel for {}.\n", removed.site),
    };
    for record in &removed.dns_left {
        out.push_str(&format!(
            "The DNS record is still there. Delete the CNAME {}\n(→ {}) in the Cloudflare dashboard, under the zone's DNS records;\nuntil then visitors get a Cloudflare error page. yard doctor reminds you until it is gone.\n",
            record.host, record.target
        ));
    }
    print!("{out}");
    Ok(0)
}

/// How a job [`result`] waited for ended.
enum Done<T> {
    /// It made `T`, for the human output.
    Made(T),
    /// Under `--json`, its events and then its result were printed; nothing more is.
    Printed,
    /// It was no longer waited for (Ctrl-C twice), with the exit code.
    Abandoned(i32),
}

/// Runs a job and reads its result. With `--json`, `wait_job` prints the job's events and
/// this prints the result as the last line.
async fn result<P: serde::Serialize, T: serde::de::DeserializeOwned>(
    ctx: &Ctx,
    method: &str,
    params: P,
) -> Result<Done<T>, CliError> {
    let value = match wait_job(ctx, method, params).await? {
        JobEnd::Finished(value) => value.unwrap_or_default(),
        JobEnd::Abandoned(code) => return Ok(Done::Abandoned(code)),
    };
    if ctx.json {
        print_json_line(&value);
        return Ok(Done::Printed);
    }
    serde_json::from_value(value).map(Done::Made).map_err(|e| {
        YardError::internal(format!("yardd answered {method} with an unexpected result: {e}")).into()
    })
}

pub async fn list(ctx: &Ctx) -> Outcome {
    let tunnels: Vec<TunnelInfo> = ctx.client().await?.call(methods::TUNNELS_LIST, Value::Null).await?;
    ctx.emit(&tunnels, || {
        let mut out = if tunnels.is_empty() {
            "No tunnels. Share a site with yard tunnel, run inside it.\n".to_string()
        } else {
            let rows: Vec<Vec<String>> = tunnels
                .iter()
                .map(|t| {
                    vec![
                        t.site.clone(),
                        t.url.clone().unwrap_or_else(|| "-".into()),
                        name_of(&t.kind),
                        name_of(&t.state),
                        t.tunnel.as_ref().map_or_else(|| "-".into(), |t| t.name.clone()),
                    ]
                })
                .collect();
            table(&["SITE", "URL", "KIND", "STATE", "TUNNEL"], &rows)
        };
        out.push('\n');
        out.push_str(&for_agents("yard tunnels"));
        out
    });
    Ok(0)
}

/// An enum's wire name, as `--json` shows it.
fn name_of<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "-".into())
}

pub async fn domains(ctx: &Ctx) -> Outcome {
    let domains: Vec<TunnelDomain> = ctx.client().await?.call(methods::TUNNELS_DOMAINS, Value::Null).await?;
    ctx.emit(&domains, || {
        let mut out = if domains.is_empty() {
            "No domains. Set one up with yard tunnels domain add <domain>.\n".to_string()
        } else {
            let rows: Vec<Vec<String>> = domains
                .iter()
                .map(|d| {
                    let signed_in = if d.logged_in { "yes" } else { "no" };
                    vec![d.domain.clone(), name_of(&d.provider), signed_in.into()]
                })
                .collect();
            table(&["DOMAIN", "PROVIDER", "SIGNED IN"], &rows)
        };
        out.push('\n');
        out.push_str(&for_agents("yard tunnels domains"));
        out
    });
    Ok(0)
}

/// `cloudflared tunnel login` opens a browser and waits for the user, so it needs a person.
pub async fn domain_add(ctx: &Ctx, domain: String, provider: Option<String>) -> Outcome {
    if !interactive(ctx) {
        return Err(YardError::new(
            ErrorCode::InvalidArgument,
            "yard tunnels domain add opens a browser to sign in",
        )
        .with_hint("run it in a terminal")
        .into());
    }
    let added: TunnelDomain =
        match result(ctx, methods::TUNNELS_DOMAIN_ADD, TunnelDomainAddParams { provider, domain }).await? {
            Done::Made(added) => added,
            Done::Printed => return Ok(0),
            Done::Abandoned(code) => return Ok(code),
        };
    println!("Signed in for {0}. Share a site on it with yard tunnel --host <name>.{0}", added.domain);
    Ok(0)
}

pub async fn domain_remove(ctx: &Ctx, domain: String) -> Outcome {
    let mut client = ctx.client().await?;
    let left: Vec<TunnelDomain> =
        client.call(methods::TUNNELS_DOMAIN_REMOVE, TunnelDomainParams { domain: domain.clone() }).await?;
    ctx.emit(&left, || {
        format!("Forgot {domain}. Tunnels on it keep running; yard tunnel --host can no longer pick it.\n")
    });
    Ok(0)
}
