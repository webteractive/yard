//! How many clients a service instance has (M4e spec §7): its established TCP connections
//! and accepted unix-socket connections, across its whole process tree, since PostgreSQL
//! hands each connection to a forked backend. A connection between two of the tree's own
//! sockets is not a client.
//!
//! Asks `lsof`: libc has `proc_listchildpids` but not `socket_fdinfo`, so the sockets
//! themselves cannot be read through libproc here.

use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LSOF: &str = "/usr/sbin/lsof";

/// How long one `lsof` may take before it is killed and the count is an error.
const LSOF_TIMEOUT: Duration = Duration::from_secs(5);

/// The client connections of `pid` and its descendants. `Err` only when `lsof` cannot run
/// or does not finish.
pub fn client_connections(pid: u32) -> Result<usize, String> {
    let pids = tree(pid).iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let tcp = lsof(&["-nP", "-a", "-p", &pids, "-iTCP", "-sTCP:ESTABLISHED", "-F", "dn"])?;
    let unix = lsof(&["-nP", "-a", "-p", &pids, "-U", "-F", "dn"])?;
    Ok(count_tcp(&tcp) + count_unix(&unix))
}

/// `pid` and every process below it.
fn tree(pid: u32) -> Vec<u32> {
    let mut all = vec![pid];
    let mut i = 0;
    while i < all.len() {
        all.extend(children(all[i]));
        i += 1;
    }
    all
}

fn children(pid: u32) -> Vec<u32> {
    let Ok(ppid) = libc::pid_t::try_from(pid) else { return vec![] };
    let mut buf = vec![0 as libc::pid_t; 1024];
    let bytes = libc::c_int::try_from(buf.len() * size_of::<libc::pid_t>()).unwrap_or(libc::c_int::MAX);
    // SAFETY: the buffer holds `bytes` bytes and outlives the call.
    let n = unsafe { libc::proc_listchildpids(ppid, buf.as_mut_ptr().cast(), bytes) };
    // libproc answers the number of pids it wrote.
    buf.truncate(usize::try_from(n).unwrap_or(0).min(buf.len()));
    buf.into_iter().filter_map(|p| u32::try_from(p).ok()).filter(|p| *p > 0).collect()
}

/// lsof's stdout, whatever its exit status: it exits 1 when nothing matched.
fn lsof(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(LSOF);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    // lsof must not keep a socket it inherited.
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let mut child = cmd.spawn().map_err(|e| format!("cannot run {LSOF}: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("lsof has no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + LSOF_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{LSOF} did not finish within {} s", LSOF_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(format!("cannot wait for {LSOF}: {e}")),
        }
    }
    reader.join().map_err(|_| "cannot read lsof's output".to_string())
}

/// Established TCP connections in `lsof -F dn` output (`n127.0.0.1:3307->127.0.0.1:50000`),
/// each socket once: a backend may hold its connection on two descriptors, and a postmaster
/// holds a copy until it has handed it over. A connection whose other end the tree holds too
/// (MinIO's console calling its own API) is the engine talking to itself, not a client.
fn count_tcp(out: &str) -> usize {
    let connections: Vec<(&str, (&str, &str))> =
        sockets(out).filter_map(|(socket, name)| Some((socket, name.split_once("->")?))).collect();
    let ends: BTreeSet<(&str, &str)> = connections.iter().map(|(_, ends)| *ends).collect();
    connections
        .iter()
        .filter(|(_, (local, remote))| !ends.contains(&(*remote, *local)))
        .map(|(socket, _)| *socket)
        .collect::<BTreeSet<_>>()
        .len()
}

/// Accepted unix-socket connections in `lsof -U -F dn` output. An accepted socket shows its
/// listener's path, as the listener itself does, so of the sockets naming a path, one per
/// path is the listener; `->0x…` names are the process's own client ends and are not counted.
fn count_unix(out: &str) -> usize {
    let named: BTreeSet<(&str, &str)> = sockets(out).filter(|(_, name)| name.starts_with('/')).collect();
    let paths: BTreeSet<&str> = named.iter().map(|(_, name)| *name).collect();
    named.len() - paths.len()
}

/// Each descriptor's `(socket, name)`: the `d` field is the socket's kernel address, which a
/// descriptor duplicated or inherited shares.
fn sockets(out: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut socket = "";
    out.lines().filter_map(move |line| {
        if let Some(address) = line.strip_prefix('d') {
            socket = address;
        } else if line.starts_with('f') {
            socket = "";
        } else if let Some(name) = line.strip_prefix('n') {
            return Some((socket, name));
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_tcp_counts_established_lines() {
        let out =
            "p1\nf7\nd0xa\nn127.0.0.1:3307->127.0.0.1:50000\nf8\nd0xb\nn127.0.0.1:3307->127.0.0.1:50001\n";
        assert_eq!(count_tcp(out), 2);
        assert_eq!(count_tcp("p1\n"), 0);
    }

    #[test]
    fn a_socket_on_two_descriptors_counts_once() {
        // A backend holding its connection as stdin and stdout, and a postmaster that has not
        // closed its copy yet.
        let out = "p1\nf9\nd0xa\nn127.0.0.1:5433->127.0.0.1:50000\np2\nf0\nd0xa\nn127.0.0.1:5433->127.0.0.1:50000\nf1\nd0xa\nn127.0.0.1:5433->127.0.0.1:50000\n";
        assert_eq!(count_tcp(out), 1);
    }

    #[test]
    fn a_connection_inside_the_tree_is_not_a_client() {
        // MinIO's console talking to its own API holds both ends; a real client holds one.
        let out = "p1\nf7\nd0xa\nn127.0.0.1:9000->127.0.0.1:50000\nf8\nd0xb\nn127.0.0.1:50000->127.0.0.1:9000\nf9\nd0xc\nn127.0.0.1:9000->127.0.0.1:50001\nf10\nd0xd\nn[::1]:9001->[::1]:50002\np2\nf3\nd0xe\nn[::1]:50002->[::1]:9001\n";
        assert_eq!(count_tcp(out), 1);
    }

    #[test]
    fn count_unix_skips_listeners_and_client_ends() {
        let out = "p1\nf3\nd0x1\nn/r/run/mysql.sock\nf5\nd0x2\nn->0xabc\nf6\nd0x3\nn/r/run/mysql.sock\nf7\nd0x4\nn/r/run/mysql.sock\nf8\nd0x4\nn/r/run/mysql.sock\n";
        assert_eq!(count_unix(out), 2);
        assert_eq!(count_unix("p1\nf3\nd0x1\nn/r/run/mysql.sock\n"), 0);
    }
}
