//! What a GUI client needs to connect to an instance (db spec §8).

use crate::services::engines::Connect;
use yard_proto::{Account, ConnectInfo, ServiceState};

/// The loopback address every instance listens on.
pub const HOST: &str = "127.0.0.1";

/// `connect` is what the engine says about the instance (its URL, socket and the clients that
/// take it); `database` ends the URL with `/<database>`.
pub fn info(
    instance: &str,
    engine: &str,
    state: ServiceState,
    port: u16,
    account: &Account,
    connect: &Connect,
    database: Option<&str>,
) -> ConnectInfo {
    let path = database.map(|d| format!("/{d}")).unwrap_or_default();
    ConnectInfo {
        instance: instance.to_string(),
        engine: engine.to_string(),
        state,
        sleeping: false,
        host: HOST.to_string(),
        port,
        user: account.user.clone(),
        password: account.password.clone().filter(|p| !p.is_empty()),
        socket: connect.socket.as_ref().map(|s| s.display().to_string()),
        url: format!("{}{path}", connect.url),
        clients: connect.clients.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;
    use crate::services::engines::{Engines, InstanceCtx};
    use std::path::Path;
    use yard_proto::{PortRef, ServiceState};

    fn connect_of(key: &str, instance: &str, port: u16) -> (Account, Connect) {
        let engines = Engines::builtin();
        let engine = engines.get(key).unwrap();
        let paths = Paths::new("/r");
        let ports = [PortRef { name: instance.into(), port }];
        let ctx = InstanceCtx {
            instance,
            home: Path::new("/k"),
            paths: &paths,
            ports: &ports,
            macos_user: "me",
            node: None,
        };
        (engine.account("me"), engine.connect(&ctx).unwrap())
    }

    #[test]
    fn connect_info_per_engine() {
        let (account, connect) = connect_of("mysql", "mysql-8.4", 3307);
        let my = info("mysql-8.4", "mysql", ServiceState::Running, 3307, &account, &connect, None);
        assert_eq!(my.url, "mysql://root@127.0.0.1:3307");
        assert_eq!((my.host.as_str(), my.port), ("127.0.0.1", 3307));
        assert_eq!(my.socket.as_deref(), Some("/r/run/mysql-8.4.sock"));
        assert_eq!((my.user.as_deref(), my.password), (Some("root"), None));
        assert_eq!(my.clients.len(), 2);
        let shop = info("mysql-8.4", "mysql", ServiceState::Running, 3307, &account, &connect, Some("shop"));
        assert_eq!(shop.url, "mysql://root@127.0.0.1:3307/shop");

        let (account, connect) = connect_of("postgres", "postgres-17", 5433);
        let pg = info("postgres-17", "postgres", ServiceState::Stopped, 5433, &account, &connect, None);
        assert_eq!(pg.url, "postgresql://me@127.0.0.1:5433");
        assert_eq!(pg.socket.as_deref(), Some("/r/run"));
        assert_eq!(pg.engine, "postgres");

        let (account, connect) = connect_of("redis", "redis", 6380);
        let redis = info("redis", "redis", ServiceState::Running, 6380, &account, &connect, None);
        assert_eq!(redis.url, "redis://127.0.0.1:6380");
        assert_eq!((redis.user, redis.password, redis.socket), (None, None, None));
    }
}
