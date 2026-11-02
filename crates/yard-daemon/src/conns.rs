//! How many clients a service instance has (M4e spec §7): its established TCP connections
//! and accepted unix-socket connections, across its whole process tree, since PostgreSQL
//! hands each connection to a forked backend. A connection between two of the tree's own
//! sockets is not a client.
//!
//! Asks `lsof`: libc has `proc_listchildpids` but not `socket_fdinfo`, so the sockets
//! themselves cannot be read through libproc here.
//!
//! A client that connects for a moment (a `schedule:run`, which runs PHP by `PHP_BINARY`
//! and so past the shim) is rarely there when the clients are counted. The connection it
//! closed is: macOS keeps it in `TIME_WAIT` for 30 s (`net.inet.tcp.msl` is 15 s), longer
//! than the sample interval, so [`Closed`] reads them from `netstat` and counts each once.
//! yardd's own connections to an instance's port (its health checks) are left out by their
//! local port, which [`own_connection`] records.

use std::collections::{BTreeSet, VecDeque};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const LSOF: &str = "/usr/sbin/lsof";
const NETSTAT: &str = "/usr/sbin/netstat";

/// How long one `lsof` or `netstat` may take before it is killed and the count is an error.
const RUN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long yardd remembers one of its own connections: longer than its `TIME_WAIT`.
const OWN_FOR: Duration = Duration::from_secs(90);

/// The local ports of yardd's own recent connections to loopback ports.
static OWN: Mutex<VecDeque<(Instant, u16)>> = Mutex::new(VecDeque::new());

/// Records a connection yardd made from `local_port` (a health check, a probe), so the
/// `TIME_WAIT` it leaves is not taken for a client.
pub fn own_connection(local_port: u16) {
    let mut own = OWN.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    while own.front().is_some_and(|(at, _)| now.duration_since(*at) > OWN_FOR) {
        own.pop_front();
    }
    own.push_back((now, local_port));
}

fn own_ports() -> BTreeSet<u16> {
    OWN.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, port)| *port).collect()
}

/// The closed connections already counted, so each counts once.
#[derive(Debug, Default)]
pub struct Closed {
    seen: Mutex<BTreeSet<(String, String)>>,
}

impl Closed {
    /// The ports among `ports` that a connection closed since the last call used, leaving
    /// out yardd's own. `Err` only when `netstat` cannot run or does not finish.
    pub fn new_on(&self, ports: &BTreeSet<u16>) -> Result<BTreeSet<u16>, String> {
        let out = run(NETSTAT, &["-an", "-p", "tcp"])?;
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        Ok(new_closed(&out, &mut seen, ports, &own_ports()))
    }
}

/// The ports among `ports` with a `TIME_WAIT` in netstat's `out` that is not in `seen`, on a
/// loopback address (the instances listen on 127.0.0.1 only, so the same port elsewhere is
/// another server), and whose other end is not one of yardd's `own` ports. `seen` becomes this call's entries, so
/// it holds only what netstat still shows.
fn new_closed(
    out: &str,
    seen: &mut BTreeSet<(String, String)>,
    ports: &BTreeSet<u16>,
    own: &BTreeSet<u16>,
) -> BTreeSet<u16> {
    let now: BTreeSet<(String, String)> = out
        .lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                [proto, _, _, local, foreign, "TIME_WAIT"] if proto.starts_with("tcp") => {
                    Some((local.to_string(), foreign.to_string()))
                }
                _ => None,
            }
        })
        .collect();
    let mut used = BTreeSet::new();
    for (local, foreign) in now.difference(seen) {
        let (Some(a), Some(b)) = (port_of(local), port_of(foreign)) else { continue };
        for ((end, address), other) in [((a, local), b), ((b, foreign), a)] {
            if ports.contains(&end) && is_loopback(address) && !own.contains(&other) {
                used.insert(end);
            }
        }
    }
    *seen = now;
    used
}

/// The port of a netstat address: `127.0.0.1.3307`, `::1.3307`.
fn port_of(address: &str) -> Option<u16> {
    address.rsplit_once('.')?.1.parse().ok()
}

fn is_loopback(address: &str) -> bool {
    address.rsplit_once('.').is_some_and(|(host, _)| host == "127.0.0.1" || host == "::1")
}

/// The client connections of `pid` and its descendants. `Err` only when `lsof` cannot run
/// or does not finish.
pub fn client_connections(pid: u32) -> Result<usize, String> {
    let pids = tree(pid).iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let tcp = run(LSOF, &["-nP", "-a", "-p", &pids, "-iTCP", "-sTCP:ESTABLISHED", "-F", "dn"])?;
    let unix = run(LSOF, &["-nP", "-a", "-p", &pids, "-U", "-F", "dn"])?;
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

/// `program`'s stdout, whatever its exit status: lsof exits 1 when nothing matched.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    // It must not keep a socket it inherited.
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let mut child = cmd.spawn().map_err(|e| format!("cannot run {program}: {e}"))?;
    let mut stdout = child.stdout.take().ok_or(format!("{program} has no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + RUN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} did not finish within {} s", RUN_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(format!("cannot wait for {program}: {e}")),
        }
    }
    reader.join().map_err(|_| format!("cannot read {program}'s output"))
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

    const NETSTAT_OUT: &str = "Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)
tcp4       0      0  127.0.0.1.6380         *.*                    LISTEN
tcp4       0      0  127.0.0.1.50001        127.0.0.1.6380         TIME_WAIT
tcp4       0      0  127.0.0.1.3307         127.0.0.1.50002        TIME_WAIT
tcp6       0      0  ::1.5433               ::1.50003              TIME_WAIT
tcp4       0      0  127.0.0.1.50004        127.0.0.1.6380         ESTABLISHED
tcp4       0      0  127.0.0.1.50005        127.0.0.1.9999         TIME_WAIT
";

    fn ports(list: &[u16]) -> BTreeSet<u16> {
        list.iter().copied().collect()
    }

    #[test]
    fn a_closed_connection_counts_on_either_end() {
        let mut seen = BTreeSet::new();
        let used = new_closed(NETSTAT_OUT, &mut seen, &ports(&[6380, 3307, 5433]), &BTreeSet::new());
        // The client closed first (6380), the server did (3307), and over IPv6 (5433).
        assert_eq!(used, ports(&[6380, 3307, 5433]));
    }

    #[test]
    fn a_closed_connection_counts_once() {
        let mut seen = BTreeSet::new();
        let watched = ports(&[6380]);
        assert_eq!(new_closed(NETSTAT_OUT, &mut seen, &watched, &BTreeSet::new()), watched);
        assert!(new_closed(NETSTAT_OUT, &mut seen, &watched, &BTreeSet::new()).is_empty());
        // Gone from netstat, it is forgotten.
        new_closed("", &mut seen, &watched, &BTreeSet::new());
        assert!(seen.is_empty());
    }

    #[test]
    fn a_remote_server_on_the_same_port_is_not_a_client() {
        // TablePlus talking to a MySQL elsewhere on 3307 says nothing about the local one.
        let out = "tcp4       0      0  192.168.1.5.50010      10.0.0.9.3307          TIME_WAIT\n";
        let used = new_closed(out, &mut BTreeSet::new(), &ports(&[3307]), &BTreeSet::new());
        assert!(used.is_empty(), "{used:?}");
    }

    #[test]
    fn yardds_own_connections_are_not_clients() {
        let mut seen = BTreeSet::new();
        let used = new_closed(NETSTAT_OUT, &mut seen, &ports(&[6380, 3307]), &ports(&[50001, 50002]));
        assert!(used.is_empty(), "{used:?}");
    }

    #[test]
    fn a_real_closed_connection_is_seen() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let closed = Closed::default();
        let watched: BTreeSet<u16> = [port].into();
        assert!(closed.new_on(&watched).unwrap().is_empty());
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (server, _) = listener.accept().unwrap();
        drop(client);
        std::thread::sleep(Duration::from_millis(20));
        drop(server);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(closed.new_on(&watched).unwrap(), watched);
        assert!(closed.new_on(&watched).unwrap().is_empty(), "counted once");
    }

    #[test]
    fn own_connection_is_remembered() {
        own_connection(61234);
        assert!(own_ports().contains(&61234));
    }

    #[test]
    fn count_unix_skips_listeners_and_client_ends() {
        let out = "p1\nf3\nd0x1\nn/r/run/mysql.sock\nf5\nd0x2\nn->0xabc\nf6\nd0x3\nn/r/run/mysql.sock\nf7\nd0x4\nn/r/run/mysql.sock\nf8\nd0x4\nn/r/run/mysql.sock\n";
        assert_eq!(count_unix(out), 2);
        assert_eq!(count_unix("p1\nf3\nd0x1\nn/r/run/mysql.sock\n"), 0);
    }
}
