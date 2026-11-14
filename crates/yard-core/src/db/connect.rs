//! What a GUI client needs to connect to an instance (db spec §8).

use crate::services::Engine;
use std::path::Path;
use yard_proto::{Account, ConnectInfo, ServiceState};

/// The loopback address every instance listens on.
pub const HOST: &str = "127.0.0.1";

/// `socket` is MySQL's socket file or PostgreSQL's socket directory; Redis has none.
/// `database` ends the URL with `/<database>`.
pub fn info(
    instance: &str,
    engine: Engine,
    state: ServiceState,
    port: u16,
    account: &Account,
    socket: Option<&Path>,
    database: Option<&str>,
) -> ConnectInfo {
    let user = account.user.clone();
    let password = account.password.clone().filter(|p| !p.is_empty());
    let scheme = match engine {
        Engine::Postgres => "postgresql",
        Engine::Redis => "redis",
        _ => "mysql",
    };
    let login = user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
    let path = database.map(|d| format!("/{d}")).unwrap_or_default();
    ConnectInfo {
        instance: instance.to_string(),
        engine: engine.key().to_string(),
        state,
        sleeping: false,
        host: HOST.to_string(),
        port,
        user,
        password,
        socket: socket.map(|s| s.display().to_string()),
        url: format!("{scheme}://{login}{HOST}:{port}{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::Engine;
    use std::path::Path;
    use yard_proto::{Account, ServiceState};

    fn account(user: Option<&str>, password: Option<&str>) -> Account {
        Account { user: user.map(String::from), password: password.map(String::from) }
    }

    #[test]
    fn connect_info_per_engine() {
        let sock = Path::new("/r/run/mysql-8.4.sock");
        let my = info(
            "mysql-8.4",
            Engine::Mysql,
            ServiceState::Running,
            3307,
            &account(Some("root"), Some("")),
            Some(sock),
            None,
        );
        assert_eq!(my.url, "mysql://root@127.0.0.1:3307");
        assert_eq!((my.host.as_str(), my.port), ("127.0.0.1", 3307));
        assert_eq!(my.socket.as_deref(), Some("/r/run/mysql-8.4.sock"));
        assert_eq!((my.user.as_deref(), my.password), (Some("root"), None));
        let shop = info(
            "mysql-8.4",
            Engine::Mysql,
            ServiceState::Running,
            3307,
            &account(Some("root"), Some("")),
            Some(sock),
            Some("shop"),
        );
        assert_eq!(shop.url, "mysql://root@127.0.0.1:3307/shop");

        let pg = info(
            "postgres-17",
            Engine::Postgres,
            ServiceState::Stopped,
            5433,
            &account(Some("me"), Some("")),
            Some(Path::new("/r/run")),
            None,
        );
        assert_eq!(pg.url, "postgresql://me@127.0.0.1:5433");
        assert_eq!(pg.socket.as_deref(), Some("/r/run"));
        assert_eq!(pg.engine, "postgres");

        let redis =
            info("redis", Engine::Redis, ServiceState::Running, 6380, &account(None, None), None, None);
        assert_eq!(redis.url, "redis://127.0.0.1:6380");
        assert_eq!((redis.user, redis.password, redis.socket), (None, None, None));
    }
}
