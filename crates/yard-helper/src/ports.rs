//! `ports.bind`: the one thing the privileged profile needs root for. The helper binds a
//! privileged port on both loopback addresses and hands the listening sockets to yardd,
//! which passes them to nginx. Root runs nothing.

use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::OwnedFd;
use yard_proto::{ErrorCode, YardError};

/// The same backlog nginx asks for when it binds a port itself.
const BACKLOG: i32 = 511;

/// The addresses `bind_loopback` listens on, in the order it returns their sockets.
pub fn loopback(port: u16) -> [SocketAddr; 2] {
    [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))]
}

/// Listening TCP sockets on `127.0.0.1:port` and `[::1]:port`. Either both or neither.
pub fn bind_loopback(port: u16) -> Result<Vec<OwnedFd>, YardError> {
    loopback(port).iter().map(|addr| listen(*addr).map(OwnedFd::from)).collect()
}

fn listen(addr: SocketAddr) -> Result<Socket, YardError> {
    let open = || -> std::io::Result<Socket> {
        let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        if addr.is_ipv6() {
            socket.set_only_v6(true)?;
        }
        socket.bind(&addr.into())?;
        socket.listen(BACKLOG)?;
        Ok(socket)
    };
    open().map_err(|e| {
        let port = addr.port();
        if e.kind() == std::io::ErrorKind::AddrInUse {
            YardError::new(ErrorCode::PortInUse, format!("port {port} is in use on {addr}"))
                .with_hint("yard ports check")
                .with_data(serde_json::json!({ "port": port }))
        } else {
            YardError::internal(format!("cannot listen on {addr}: {e}"))
        }
    })
}
