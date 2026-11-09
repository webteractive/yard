//! The root helper: a fixed verb set behind a unix socket (spec §3.1).
//!
//! No caller sends a path or a command. The owning user, the data root and the brew
//! prefix are fixed at install time; a caller passes at most a TLD and a port.

pub mod fdpass;
mod peer;
mod ports;
mod resolver;
pub mod root_dir;
mod verbs;

use std::future::Future;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::Interest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use yard_proto::rpc::{Request, Response, encode_line};
use yard_proto::{ErrorCode, YardError};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub use yard_proto::helper::HELPER_REVISION;

#[derive(Debug, Clone, PartialEq)]
pub struct HelperConfig {
    pub uid: u32,
    pub user: String,
    pub root: PathBuf,
    pub brew_prefix: PathBuf,
    pub socket: PathBuf,
    pub resolver_dir: PathBuf,
    /// What `ports.bind` may open. Fixed to 80 and 443 outside tests.
    pub privileged_ports: Vec<u16>,
}

impl HelperConfig {
    /// Parses the plist's `--uid N --user NAME --root PATH --brew-prefix PATH`.
    /// The socket, resolver directory and privileged ports are fixed and never come from
    /// arguments.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let (mut uid, mut user, mut root, mut brew_prefix) = (None, None, None, None);
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--uid" => uid = Some(value.parse::<u32>().map_err(|e| format!("--uid {value}: {e}"))?),
                "--user" => user = Some(value),
                "--root" => root = Some(PathBuf::from(value)),
                "--brew-prefix" => brew_prefix = Some(PathBuf::from(value)),
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        Ok(Self {
            uid: uid.ok_or("--uid is required")?,
            user: user.ok_or("--user is required")?,
            root: root.ok_or("--root is required")?,
            brew_prefix: brew_prefix.ok_or("--brew-prefix is required")?,
            socket: PathBuf::from(yard_core::paths::HELPER_SOCKET),
            resolver_dir: PathBuf::from(yard_core::paths::RESOLVER_DIR),
            privileged_ports: yard_proto::helper::PRIVILEGED_PORTS.to_vec(),
        })
    }
}

/// Serves until `shutdown` resolves. The socket is mode 0666 so the owner can reach it
/// without a group change; each connection's peer uid is checked instead.
pub async fn serve(cfg: HelperConfig, shutdown: impl Future<Output = ()>) -> io::Result<()> {
    match std::fs::remove_file(&cfg.socket) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&cfg.socket)?;
    std::fs::set_permissions(&cfg.socket, std::fs::Permissions::from_mode(0o666))?;
    let socket = cfg.socket.clone();
    let helper = Arc::new(verbs::Helper::new(cfg));
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(connection(helper.clone(), stream));
                }
                Err(e) => eprintln!("yard-helper: accept failed: {e}"),
            },
        }
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}

async fn connection(helper: Arc<verbs::Helper>, stream: UnixStream) {
    let Ok(cred) = stream.peer_cred() else { return };
    if !peer::allowed(cred.uid(), helper.owner_uid()) {
        return;
    }
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let (response, fds) = match serde_json::from_str::<Request>(&line) {
            Ok(Request { id: None, .. }) => continue,
            Ok(Request { id: Some(id), method, params, .. }) => match helper.handle(&method, params).await {
                Ok(reply) => (Response::ok(id, reply.result), reply.fds),
                Err(e) => (Response::err(id, e), vec![]),
            },
            Err(e) => (
                Response::err(
                    0,
                    YardError::new(ErrorCode::InvalidRequest, format!("not a JSON-RPC request: {e}")),
                ),
                vec![],
            ),
        };
        if send(&mut write, &encode_line(&response), &fds).await.is_err() {
            return;
        }
    }
}

/// Writes one answer line; any descriptors ride on its first byte. The helper's copies
/// close when `fds` drops, after the kernel has duplicated them into the message.
async fn send(write: &mut OwnedWriteHalf, line: &[u8], fds: &[OwnedFd]) -> io::Result<()> {
    if fds.is_empty() {
        return write.write_all(line).await;
    }
    let raw: Vec<_> = fds.iter().map(AsRawFd::as_raw_fd).collect();
    let stream: &UnixStream = write.as_ref();
    let sent = loop {
        stream.writable().await?;
        match stream.try_io(Interest::WRITABLE, || fdpass::send_with_fds(stream.as_raw_fd(), line, &raw)) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            result => break result?,
        }
    };
    write.write_all(&line[sent..]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_plist_arguments_and_fixes_socket_and_resolver_dir() {
        let cfg = HelperConfig::from_args(args(&[
            "--uid",
            "501",
            "--user",
            "me",
            "--root",
            "/Users/me/Library/Application Support/Yard",
            "--brew-prefix",
            "/opt/homebrew",
        ]))
        .unwrap();
        assert_eq!(cfg.uid, 501);
        assert_eq!(cfg.user, "me");
        assert_eq!(cfg.root, PathBuf::from("/Users/me/Library/Application Support/Yard"));
        assert_eq!(cfg.socket, PathBuf::from("/var/run/dev.more.yard.helper.sock"));
        assert_eq!(cfg.resolver_dir, PathBuf::from("/etc/resolver"));
    }

    #[test]
    fn rejects_unknown_and_missing_arguments() {
        assert!(HelperConfig::from_args(args(&["--socket", "/tmp/x"])).is_err());
        assert!(HelperConfig::from_args(args(&["--uid", "501"])).is_err());
        assert!(HelperConfig::from_args(args(&["--uid"])).is_err());
        assert!(HelperConfig::from_args(args(&["--uid", "me"])).is_err());
    }
}
