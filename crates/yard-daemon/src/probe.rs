//! Is a port free to bind, and is something answering on it (spec §6.2).

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Tcp,
    TcpUdp,
}

pub fn proto_for(unit: &str) -> Proto {
    if unit == "dnsmasq" { Proto::TcpUdp } else { Proto::Tcp }
}

/// Binds loopback v4 and v6 and drops the sockets straight away.
///
/// The first TCP bind goes without `SO_REUSEADDR`, which would let it succeed beside a
/// wildcard listener. That bind also fails on a port left in TIME_WAIT, which nginx (setting
/// `SO_REUSEADDR` itself) can still take, so a refused port counts as free when a reuse bind
/// succeeds and nothing answers on it. A user cannot bind loopback below 1024 at all
/// (80 and 443, which the helper binds), so there a refused bind says nothing, and the
/// port is free when nothing answers on it.
pub fn is_free(port: u16, proto: Proto) -> bool {
    let addrs =
        [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))];
    let tcp = |reuse| addrs.map(|a| bind(&a, Type::STREAM, Protocol::TCP, reuse));
    let plain = tcp(false);
    let denied = plain.iter().all(|r| r.as_ref().is_err_and(|e| e.kind() == io::ErrorKind::PermissionDenied));
    let tcp_free = plain.iter().all(Result::is_ok)
        || ((denied || tcp(true).iter().all(Result::is_ok)) && !answers(&addrs));
    match proto {
        Proto::Tcp => tcp_free,
        Proto::TcpUdp => tcp_free && addrs.iter().all(|a| bind(a, Type::DGRAM, Protocol::UDP, false).is_ok()),
    }
}

fn bind(addr: &SocketAddr, ty: Type, proto: Protocol, reuse: bool) -> io::Result<()> {
    let socket = Socket::new(Domain::for_address(*addr), ty, Some(proto))?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    if reuse {
        socket.set_reuse_address(true)?;
    }
    socket.bind(&SockAddr::from(*addr))
}

fn answers(addrs: &[SocketAddr]) -> bool {
    addrs.iter().any(|a| match TcpStream::connect_timeout(a, Duration::from_millis(200)) {
        Ok(stream) => {
            // Not a client of whatever answers: an instance it reaches stays free to sleep.
            if let Ok(local) = stream.local_addr() {
                crate::conns::own_connection(local.port());
            }
            true
        }
        Err(_) => false,
    })
}

pub fn is_listening(port: u16) -> bool {
    answers(&[SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{Shutdown, TcpListener, UdpSocket};

    /// A port below macOS's ephemeral range that nothing listens on. A test that asserts a port it
    /// released is free must not take it from the ephemeral range: any process's next outbound
    /// connection can be given that port in between, and then it is not free (found 2026-11-06,
    /// when time_wait_port_is_free failed once in a full run).
    fn quiet_port() -> u16 {
        let start = 29_000 + (std::process::id() % 500) as u16;
        (start..29_999)
            .chain(29_000..start)
            .find(|p| TcpListener::bind(("127.0.0.1", *p)).is_ok())
            .expect("a free port in 29000-29999")
    }

    #[test]
    fn bound_port_is_not_free() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        assert!(!is_free(p, Proto::Tcp));
        assert!(is_listening(p));
    }

    #[test]
    fn released_port_is_free() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let p = quiet_port();
        assert!(is_free(p, Proto::Tcp));
        assert!(!is_listening(p));
    }

    #[test]
    fn udp_holder_blocks_tcpudp_probe() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // Take the port from TCP so its TCP side is known free: macOS hands out ephemeral
        // ports in sequence, so a UDP :0 can land on a port another test holds over TCP.
        let p = quiet_port();
        let _u = UdpSocket::bind(("127.0.0.1", p)).unwrap();
        assert!(!is_free(p, Proto::TcpUdp));
        assert!(is_free(p, Proto::Tcp));
    }

    #[test]
    fn wildcard_listener_is_not_free() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let l = TcpListener::bind("0.0.0.0:0").unwrap();
        assert!(!is_free(l.local_addr().unwrap().port(), Proto::Tcp));
    }

    #[test]
    fn time_wait_port_is_free() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // nginx closes keep-alive connections first, which leaves TIME_WAIT on its own port.
        let l = TcpListener::bind(("127.0.0.1", quiet_port())).unwrap();
        let p = l.local_addr().unwrap().port();
        let mut client = TcpStream::connect(("127.0.0.1", p)).unwrap();
        let (server, _) = l.accept().unwrap();
        server.shutdown(Shutdown::Both).unwrap();
        let _ = client.read_to_end(&mut Vec::new());
        drop((client, server, l));
        assert!(is_free(p, Proto::Tcp));
    }

    /// A port below 1024 that no test binds, so it is free whatever else runs.
    const LOW: u16 = 999;

    /// Binds the wildcard address on the first port below 1024 (and below `LOW`) it can. A fixed
    /// port flaked: a second test process binding it made `LOW` look taken.
    fn bind_low() -> (u16, TcpListener) {
        (900..990)
            .find_map(|p| TcpListener::bind(("0.0.0.0", p)).ok().map(|l| (p, l)))
            .expect("a free port in 900..990")
    }

    #[test]
    fn privileged_port_without_a_listener_is_free() {
        // SAFETY: geteuid cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return; // root binds it, and the plain probe answers
        }
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(is_free(LOW, Proto::Tcp), "a user cannot bind loopback below 1024, but nothing holds it");
    }

    #[test]
    fn privileged_port_with_a_listener_is_busy() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // macOS lets any user bind the wildcard address below 1024.
        let (port, _l) = bind_low();
        assert!(!is_free(port, Proto::Tcp));
    }

    #[test]
    fn dnsmasq_probes_udp_too() {
        assert_eq!(proto_for("dnsmasq"), Proto::TcpUdp);
        assert_eq!(proto_for("nginx"), Proto::Tcp);
        assert_eq!(proto_for("php-8.4"), Proto::Tcp);
    }
}
