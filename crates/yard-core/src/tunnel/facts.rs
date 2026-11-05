//! `state/tunnels.json`: what Yard learned about named tunnels (their Cloudflare ids, who
//! made them, the hosts routed to them) and the DNS records left behind (tunnels spec §6).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use yard_proto::YardError;

/// Site names at the top level, beside `dns_left`, as spec §6 shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    #[serde(default)]
    pub dns_left: Vec<LeftRecord>,
    #[serde(flatten)]
    pub named: BTreeMap<String, NamedFacts>,
}

/// A site's Cloudflare tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedFacts {
    pub id: String,
    pub name: String,
    /// Whether `yard untunnel` deletes it.
    pub created_by_yard: bool,
    /// Hosts `route dns` pointed at it; they become `dns_left` when it goes.
    pub routed: Vec<String>,
    /// Its credentials file, which only cloudflared reads.
    pub credentials: PathBuf,
    /// The domain whose certificate manages it.
    pub domain: String,
}

/// A DNS record Yard made and cannot delete; doctor warns until it no longer resolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftRecord {
    pub host: String,
    pub target: String,
    /// The local date it was left, `YYYY-MM-DD`.
    pub since: String,
}

impl Facts {
    /// A missing file is no facts.
    pub fn load(path: &Path) -> Result<Facts, YardError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Facts::default()),
            Err(e) => return Err(YardError::internal(format!("cannot read {}: {e}", path.display()))),
        };
        serde_json::from_str(&text)
            .map_err(|e| YardError::internal(format!("{} is not valid: {e}", path.display())))
    }

    /// Written to a temporary file created 0600, then renamed over the old one. The temporary
    /// file is this save's own, so two saves at once (a tunnel job and doctor) never write into
    /// each other's; the last rename wins.
    pub fn save(&self, path: &Path) -> Result<(), YardError> {
        static SAVES: AtomicU64 = AtomicU64::new(0);
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(format!(".{}-{}.yard-tmp", std::process::id(), SAVES.fetch_add(1, Ordering::Relaxed)));
        let tmp = PathBuf::from(tmp);
        let write = || -> io::Result<()> {
            use std::os::unix::fs::OpenOptionsExt;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
            let mut file =
                std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.write_all(b"\n")?;
            std::fs::rename(&tmp, path)
        };
        write().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            YardError::internal(format!("cannot write {}: {e}", path.display()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::cloudflared::target;

    #[test]
    fn facts_round_trip_in_the_spec_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/tunnels.json");
        assert_eq!(Facts::load(&path).unwrap(), Facts::default());
        let mut f = Facts::default();
        f.named.insert(
            "blog".into(),
            NamedFacts {
                id: "1b2c".into(),
                name: "yard-blog".into(),
                created_by_yard: true,
                routed: vec!["blog.example.com".into()],
                credentials: "/c.json".into(),
                domain: "example.com".into(),
            },
        );
        f.dns_left.push(LeftRecord {
            host: "old.example.com".into(),
            target: target("9f8e"),
            since: "2026-11-02".into(),
        });
        f.save(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["blog"]["name"], "yard-blog");
        assert_eq!(v["dns_left"][0]["target"], "9f8e.cfargotunnel.com");
        assert_eq!(Facts::load(&path).unwrap(), f);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn concurrent_saves_never_share_a_temporary_file() {
        // A tunnel job and doctor's dropping of gone records can save at once.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnels.json");
        let writers: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut f = Facts::default();
                    f.dns_left.push(LeftRecord {
                        host: format!("h{n}.example.com").repeat(50),
                        target: target("x"),
                        since: "2026-11-05".into(),
                    });
                    (0..200).map(|_| f.save(&path)).filter(Result::is_err).count()
                })
            })
            .collect();
        let failed: usize = writers.into_iter().map(|w| w.join().unwrap()).sum();
        assert_eq!(failed, 0, "saves failed");
        assert_eq!(Facts::load(&path).unwrap().dns_left.len(), 1);
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("tunnels.json")], "no temporary file is left");
    }
}
