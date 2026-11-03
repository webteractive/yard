//! Named tunnels (tunnels spec §8): a Cloudflare tunnel on a host under a domain set up with
//! `yard tunnels domain add`, created or adopted, routed and recorded, and the way back.

use super::cloudflared::Manage;
use super::credentials_missing;
use crate::describe;
use crate::jobs::JobCtx;
use crate::reconcile::Reconciler;
use toml_edit::value;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_core::config::TomlFile;
use yard_core::tunnel::cloudflared::{cert_path, created_credentials, default_credentials, target};
use yard_core::tunnel::facts::{Facts, LeftRecord, NamedFacts};
use yard_proto::{DnsRecord, ErrorCode, TunnelRemoved, YardError};

/// What `tunnels.add` with a host asks for, checked before its job.
pub struct AddNamed {
    pub site: String,
    pub host: String,
    pub domain: String,
    /// A tunnel to adopt instead of `yard-<site>`.
    pub tunnel: Option<String>,
    pub overwrite_dns: bool,
}

/// Today's local date, `YYYY-MM-DD`, for `dns_left`.
fn today() -> String {
    // SAFETY: time(NULL) only reads the clock.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    // SAFETY: an all-zero `tm` is valid, and localtime_r writes only into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

fn tunnel_table(site: &str) -> [&str; 3] {
    ["sites", site, "tunnel"]
}

/// Creates or adopts the tunnel, routes the host to it, writes `[sites.<site>.tunnel]`, records
/// the facts and reconciles. A failure undoes what this job did: a tunnel it created is deleted
/// (kept in the facts when the delete fails), the table goes, and a host it routed is left in
/// `dns_left`, since Yard cannot delete a DNS record.
pub async fn add(
    job: &JobCtx,
    reconciler: &Reconciler,
    brew: &Brew,
    paths: &Paths,
    a: &AddNamed,
) -> Result<(), YardError> {
    let home = reconciler.home().to_path_buf();
    let cf = Manage { job, program: brew.cloudflared_bin(), cert: cert_path(&home, &a.domain), home: &home };
    let mut facts = Facts::load(&paths.tunnels_state())?;
    let before = facts.named.get(&a.site).cloned();
    let name = a.tunnel.clone().unwrap_or_else(|| format!("yard-{}", a.site));
    let (mut record, created) = match (&a.tunnel, cf.find(&name).await?) {
        (Some(_), Some(t)) => {
            let credentials = default_credentials(&home, &t.id);
            let record = NamedFacts {
                id: t.id,
                name: t.name,
                created_by_yard: false,
                routed: vec![],
                credentials,
                domain: a.domain.clone(),
            };
            (record, false)
        }
        (Some(_), None) => {
            return Err(YardError::new(
                ErrorCode::TunnelNotFound,
                format!("no Cloudflare tunnel is named {name}"),
            )
            .with_hint("cloudflared tunnel list"));
        }
        // A tunnel Yard made for this site whose delete failed earlier is used again.
        (None, Some(t)) => {
            match before.clone() {
                Some(f) if f.id == t.id && f.created_by_yard => (f, false),
                _ => {
                    return Err(YardError::new(
                    ErrorCode::TunnelExists,
                    format!("a Cloudflare tunnel named {name} already exists, and Yard did not make it for {}", a.site),
                )
                .with_hint(format!("yard tunnel {} --host {} --tunnel {name}", a.site, a.host)));
                }
            }
        }
        (None, None) => {
            job.line(format!("Creating the Cloudflare tunnel {name}…"));
            let credentials = created_credentials(&home, &a.domain, &name);
            cf.create(&name, &credentials).await?;
            let Some(t) = cf.find(&name).await? else {
                return Err(YardError::new(
                    ErrorCode::JobFailed,
                    format!("cloudflared created the tunnel {name} but does not list it"),
                ));
            };
            let record = NamedFacts {
                id: t.id,
                name: t.name,
                created_by_yard: true,
                routed: vec![],
                credentials,
                domain: a.domain.clone(),
            };
            (record, true)
        }
    };

    let (mut routed, mut written) = (false, false);
    let outcome = async {
        if !record.credentials.exists() {
            return Err(credentials_missing(&a.site, &a.host, &record));
        }
        job.line(format!("Routing {} to it…", a.host));
        cf.route(&a.site, &record.id, &a.host, &a.domain, a.overwrite_dns).await?;
        routed = true;
        // The record is this tunnel's again, not one left for the user to delete.
        facts.dns_left.retain(|l| l.host != a.host);
        if !record.routed.contains(&a.host) {
            record.routed.push(a.host.clone());
        }
        TomlFile::edit(&paths.sites_toml(), |file| {
            file.set(&["sites", &a.site, "tunnel", "host"], value(a.host.as_str()));
            if let Some(tunnel) = &a.tunnel {
                file.set(&["sites", &a.site, "tunnel", "tunnel"], value(tunnel.as_str()));
            }
            Ok(())
        })?;
        written = true;
        facts.named.insert(a.site.clone(), record.clone());
        facts.save(&paths.tunnels_state())?;
        reconciler.reconcile().await?;
        Ok(())
    }
    .await;
    let Err(e) = outcome else { return Ok(()) };

    if written
        && let Err(undo) = TomlFile::edit(&paths.sites_toml(), |file| Ok(file.remove(&tunnel_table(&a.site))))
    {
        job.line(format!("warning: could not remove [sites.{}.tunnel]: {}", a.site, describe(&undo)));
    }
    let mut kept = before;
    if created {
        match cf.delete(&record.id).await {
            Ok(()) => kept = None,
            Err(undo) => {
                job.line(format!(
                    "warning: the Cloudflare tunnel {name} was not deleted: {}",
                    describe(&undo)
                ));
                kept = Some(NamedFacts { routed: vec![], ..record.clone() });
            }
        }
    }
    match kept {
        Some(f) => facts.named.insert(a.site.clone(), f),
        None => facts.named.remove(&a.site),
    };
    if routed {
        facts.dns_left.push(LeftRecord { host: a.host.clone(), target: target(&record.id), since: today() });
    }
    if let Err(undo) = facts.save(&paths.tunnels_state()) {
        job.line(format!("warning: {}", describe(&undo)));
    }
    if written && let Err(undo) = reconciler.reconcile().await {
        job.line(format!("warning: {}", describe(&undo)));
    }
    Err(e)
}

/// Ends a named tunnel: the table goes and the unit stops; a tunnel Yard created is deleted
/// unless `keep_tunnel` (a failed delete is a warning and keeps its facts, so doctor names it);
/// every host routed to it is left in `dns_left`.
pub async fn remove(
    job: &JobCtx,
    reconciler: &Reconciler,
    brew: &Brew,
    paths: &Paths,
    site: &str,
    keep_tunnel: bool,
) -> Result<TunnelRemoved, YardError> {
    TomlFile::edit(&paths.sites_toml(), |file| Ok(file.remove(&tunnel_table(site))))?;
    reconciler.reconcile().await?;
    let mut facts = Facts::load(&paths.tunnels_state())?;
    let Some(mut f) = facts.named.remove(site) else {
        return Ok(TunnelRemoved { site: site.to_string(), deleted_tunnel: false, dns_left: vec![] });
    };
    let delete = f.created_by_yard && !keep_tunnel;
    let mut deleted = false;
    if delete {
        let home = reconciler.home().to_path_buf();
        let cf =
            Manage { job, program: brew.cloudflared_bin(), cert: cert_path(&home, &f.domain), home: &home };
        job.line(format!("Deleting the Cloudflare tunnel {}…", f.name));
        match cf.delete(&f.id).await {
            Ok(()) => deleted = true,
            Err(e) => job.line(format!(
                "warning: the Cloudflare tunnel {} was not deleted: {}",
                f.name,
                describe(&e)
            )),
        }
    }
    let since = today();
    let left: Vec<LeftRecord> = f
        .routed
        .drain(..)
        .map(|host| LeftRecord { host, target: target(&f.id), since: since.clone() })
        .collect();
    facts.dns_left.extend(left.iter().cloned());
    if delete && !deleted {
        facts.named.insert(site.to_string(), f);
    }
    facts.save(&paths.tunnels_state())?;
    Ok(TunnelRemoved {
        site: site.to_string(),
        deleted_tunnel: deleted,
        dns_left: left.into_iter().map(|l| DnsRecord { host: l.host, target: l.target }).collect(),
    })
}
