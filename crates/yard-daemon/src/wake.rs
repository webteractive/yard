//! `run/wake.sock`: nginx (through `auth_request`) and the shims ask here before a site's
//! sleeping services are needed (M4e spec §5.1, §5.2). One route,
//! `GET /wake/<site>?wait=<s>`, and every connection is answered `204`, whatever it sent and
//! whatever the wake did, so asking never stops a request: a wake that fails is the
//! instance's own `failed` state. The one exception is `403`, with a body naming them, when
//! an instance the site needs cannot start because another program holds its port: that
//! program could pose as the database, so nginx answers `503` and the shims do not run.

use crate::daemon::Context;
use crate::sleep::{needed_by, wake};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use yard_core::sites::is_site_name;

/// How long a client gets to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest request head read.
const MAX_HEAD: usize = 8192;
/// The longest a client may ask to wait.
const MAX_WAIT_SECS: u64 = 30;
/// The wait when the request names none.
const DEFAULT_WAIT_SECS: u64 = 10;

/// How many asks may wait on a wake at once; one past it starts the wake and is answered
/// straight away, so a flood of requests cannot pile up in yardd.
pub const WAKE_SLOTS: usize = 64;

const ANSWER: &[u8] = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Serves `wake.sock` until the task is aborted, each connection in its own task.
pub async fn serve(listener: UnixListener, ctx: Arc<Context>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(answer(stream, ctx.clone()));
            }
            Err(e) => {
                tracing::warn!("wake.sock: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn answer(mut stream: UnixStream, ctx: Arc<Context>) {
    let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream)).await.unwrap_or_default();
    let mut blocked = vec![];
    match parse(&head) {
        Some((site, wait)) => {
            let needed = ctx
                .reconciler
                .read(|s| s.resolution.sites.iter().any(|x| x.name == site).then(|| needed_by(s, &site)))
                .flatten();
            match needed {
                Some(instances) => {
                    // Past the cap, the wake still goes ahead but nobody waits on it.
                    let slot = ctx.wake_slots.clone().try_acquire_owned().ok();
                    let wait = if slot.is_some() { Duration::from_secs(wait) } else { Duration::ZERO };
                    blocked = wake(&ctx, &instances, wait).await;
                }
                None => tracing::info!("wake.sock: no site named {site:?}"),
            }
        }
        None => {
            tracing::info!("wake.sock: not a wake request: {:?}", head.lines().next().unwrap_or_default())
        }
    }
    let _ = stream.write_all(&answer_for(&blocked)).await;
    let _ = stream.shutdown().await;
}

/// `204`, or `403` with one line per instance whose port another program holds.
fn answer_for(blocked: &[String]) -> Vec<u8> {
    if blocked.is_empty() {
        return ANSWER.to_vec();
    }
    let body = format!("{}\n", blocked.join("\n"));
    format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The head up to its blank line, or what arrived of it within `MAX_HEAD` bytes.
async fn read_head(stream: &mut UnixStream) -> String {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while head.len() < MAX_HEAD && !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    head.truncate(MAX_HEAD);
    String::from_utf8_lossy(&head).into_owned()
}

/// `GET /wake/<site>?wait=<n> HTTP/1.x` as `(site, n)`: `n` is 10 when absent and at most
/// 30. `None` for anything else, a site name Yard would refuse included.
fn parse(head: &str) -> Option<(String, u64)> {
    let mut words = head.lines().next()?.split(' ');
    let (method, target, version) = (words.next()?, words.next()?, words.next()?);
    if method != "GET" || !version.starts_with("HTTP/1.") || words.next().is_some() {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let site = path.strip_prefix("/wake/")?;
    if !is_site_name(site) {
        return None;
    }
    let mut wait = DEFAULT_WAIT_SECS;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        if let Some(n) = pair.strip_prefix("wait=") {
            wait = n.parse().ok()?;
        }
    }
    Some((site.to_string(), wait.min(MAX_WAIT_SECS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_port_is_a_403_naming_it() {
        assert_eq!(answer_for(&[]), ANSWER);
        let answer =
            String::from_utf8(answer_for(&["redis: port 6380 is held by nc (pid 9)".into()])).unwrap();
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{answer}");
        assert!(answer.ends_with("\r\n\r\nredis: port 6380 is held by nc (pid 9)\n"), "{answer}");
        assert!(answer.contains("Content-Length: 39\r\n"), "{answer}");
    }

    #[test]
    fn parse_takes_the_one_route() {
        let ask = |line: &str| parse(&format!("{line}\r\nHost: yard\r\n\r\n"));
        assert_eq!(ask("GET /wake/blog?wait=5 HTTP/1.1"), Some(("blog".into(), 5)));
        assert_eq!(ask("GET /wake/blog HTTP/1.0"), Some(("blog".into(), 10)));
        assert_eq!(ask("GET /wake/blog?wait=90 HTTP/1.1"), Some(("blog".into(), 30)));
        for bad in [
            "GET / HTTP/1.1",
            "POST /wake/blog HTTP/1.1",
            "GET /wake/../x HTTP/1.1",
            "GET /wake/blog?wait=soon HTTP/1.1",
            "GET /wake/ HTTP/1.1",
            "hello",
        ] {
            assert_eq!(ask(bad), None, "{bad}");
        }
        assert_eq!(parse(""), None);
    }
}
