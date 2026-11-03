//! Asks each running tunnel's metrics server whether it reached the edge (`/ready`) and, for a
//! quick tunnel, which hostname it got (`/quicktunnel`). A tunnel that loses the edge is only
//! `connecting` until cloudflared reconnects by itself; the unit is never restarted for it.

use super::{info, unit};
use crate::daemon::Context;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use yard_proto::{Event, TunnelInfo, TunnelKind, UnitState};

/// How long one question to a metrics server may take.
const ASK_TIMEOUT: Duration = Duration::from_secs(1);

/// An HTTP/1.1 `GET` of `path` on `127.0.0.1:<port>`: the status and the body, or `None` when
/// nothing answered in `timeout`. Blocks.
pub fn http_get(port: u16, path: &str, timeout: Duration) -> Option<(u16, String)> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, timeout).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut text = String::new();
    s.read_to_string(&mut text).ok()?;
    let status = text.split(' ').nth(1)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Some((status, body))
}

/// The hostname in `/quicktunnel`'s `{"hostname": "…"}`, when it has one yet.
fn quick_hostname(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v["hostname"].as_str().filter(|h| !h.is_empty()).map(str::to_string)
}

/// Whether two infos differ in what `tunnel_changed` reports.
fn changed(old: Option<&TunnelInfo>, new: &TunnelInfo) -> bool {
    old.is_none_or(|o| o.state != new.state || o.url != new.url || o.last_error != new.last_error)
}

pub fn spawn(ctx: Arc<Context>, every: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut emitted: BTreeMap<String, TunnelInfo> = BTreeMap::new();
        // The pid each tunnel's unit had: a new process is a new quick hostname.
        let mut pids: BTreeMap<String, Option<u32>> = BTreeMap::new();
        loop {
            tick.tick().await;
            let Some(snapshot) = ctx.reconciler.snapshot().await else { continue };
            let units = ctx.supervisor().list();
            let mut asks = vec![];
            {
                let mut live = ctx.tunnels.live.lock().unwrap_or_else(PoisonError::into_inner);
                live.retain(|site, _| snapshot.tunnels.contains_key(site));
                for (site, planned) in &snapshot.tunnels {
                    let u = units.iter().find(|u| u.name == unit(site));
                    let active =
                        u.is_some_and(|u| matches!(u.state, UnitState::Running | UnitState::Starting));
                    let pid = u.and_then(|u| u.pid);
                    if !active || pids.insert(site.clone(), pid).is_some_and(|old| old != pid) {
                        live.remove(site);
                    }
                    if active {
                        let has_host = live.get(site).is_some_and(|l| l.hostname.is_some());
                        let need_host = planned.kind == TunnelKind::Quick && !has_host;
                        asks.push((site.clone(), planned.metrics_port, need_host));
                    }
                }
            }
            pids.retain(|site, _| snapshot.tunnels.contains_key(site));
            emitted.retain(|site, _| snapshot.tunnels.contains_key(site));
            let answers = tokio::task::spawn_blocking(move || {
                asks.into_iter()
                    .map(|(site, port, need_host)| {
                        let ready = http_get(port, "/ready", ASK_TIMEOUT).is_some_and(|(s, _)| s == 200);
                        let host = need_host
                            .then(|| http_get(port, "/quicktunnel", ASK_TIMEOUT))
                            .flatten()
                            .and_then(|(_, body)| quick_hostname(&body));
                        (site, ready, host)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let infos: Vec<TunnelInfo> = {
                let mut live = ctx.tunnels.live.lock().unwrap_or_else(PoisonError::into_inner);
                for (site, ready, host) in answers {
                    let entry = live.entry(site).or_default();
                    entry.ready = ready;
                    if host.is_some() {
                        entry.hostname = host;
                    }
                }
                snapshot
                    .tunnels
                    .iter()
                    .map(|(site, planned)| {
                        let u = units.iter().find(|u| u.name == unit(site));
                        info(site, planned, u, live.get(site))
                    })
                    .collect()
            };
            for tunnel in infos {
                if changed(emitted.get(&tunnel.site), &tunnel) {
                    emitted.insert(tunnel.site.clone(), tunnel.clone());
                    ctx.bus.emit(Event::TunnelChanged { tunnel: Box::new(tunnel) });
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_hostname_needs_a_value() {
        assert_eq!(
            quick_hostname(r#"{"hostname":"a.trycloudflare.com"}"#).as_deref(),
            Some("a.trycloudflare.com")
        );
        assert_eq!(quick_hostname(r#"{"hostname":""}"#), None);
        assert_eq!(quick_hostname("not json"), None);
    }

    #[test]
    fn nothing_listening_is_none() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert_eq!(http_get(port, "/ready", Duration::from_millis(200)), None);
    }
}
