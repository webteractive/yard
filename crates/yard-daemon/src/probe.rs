//! Is a port free to bind, and is something answering on it (spec §6.2).

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
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
/// succeeds and nothing answers on it.
pub fn is_free(port: u16, proto: Proto) -> bool {
    let addrs =
        [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))];
    let tcp_free = addrs.iter().all(|a| binds(a, Type::STREAM, Protocol::TCP, false))
        || (addrs.iter().all(|a| binds(a, Type::STREAM, Protocol::TCP, true)) && !answers(&addrs));
    match proto {
        Proto::Tcp => tcp_free,
        Proto::TcpUdp => tcp_free && addrs.iter().all(|a| binds(a, Type::DGRAM, Protocol::UDP, false)),
    }
}

fn binds(addr: &SocketAddr, ty: Type, proto: Protocol, reuse: bool) -> bool {
    let Ok(socket) = Socket::new(Domain::for_address(*addr), ty, Some(proto)) else {
        return false;
    };
    if addr.is_ipv6() && socket.set_only_v6(true).is_err() {
        return false;
    }
    if reuse && socket.set_reuse_address(true).is_err() {
        return false;
    }
    socket.bind(&SockAddr::from(*addr)).is_ok()
}

fn answers(addrs: &[SocketAddr]) -> bool {
    addrs.iter().any(|a| TcpStream::connect_timeout(a, Duration::from_millis(200)).is_ok())
}

pub fn is_listening(port: u16) -> bool {
    answers(&[SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{Shutdown, TcpListener, UdpSocket};

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
        let p = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(is_free(p, Proto::Tcp));
        assert!(!is_listening(p));
    }

    #[test]
    fn udp_holder_blocks_tcpudp_probe() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // Take the port from TCP so its TCP side is known free: macOS hands out ephemeral
        // ports in sequence, so a UDP :0 can land on a port another test holds over TCP.
        let p = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
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
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        let mut client = TcpStream::connect(("127.0.0.1", p)).unwrap();
        let (server, _) = l.accept().unwrap();
        server.shutdown(Shutdown::Both).unwrap();
        let _ = client.read_to_end(&mut Vec::new());
        drop((client, server, l));
        assert!(is_free(p, Proto::Tcp));
    }

    #[test]
    fn dnsmasq_probes_udp_too() {
        assert_eq!(proto_for("dnsmasq"), Proto::TcpUdp);
        assert_eq!(proto_for("nginx"), Proto::Tcp);
        assert_eq!(proto_for("php-8.4"), Proto::Tcp);
    }
}
