//! The privileged profile's sockets (spec §3.1). The helper binds 80 and 443 on loopback
//! and yardd holds what it hands over for as long as the profile is privileged, so nginx,
//! running as the user, inherits them on every start and restart. A failed nginx gives
//! them back until it starts again.

use crate::helper_client::HelperClient;
use crate::supervisor::lock;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::Mutex;
use yard_proto::YardError;

pub struct PrivilegedSockets {
    helper: HelperClient,
    held: Mutex<BTreeMap<u16, Vec<OwnedFd>>>,
}

impl PrivilegedSockets {
    pub fn new(helper: HelperClient) -> Self {
        Self { helper, held: Mutex::default() }
    }

    /// Copies of the sockets for `ports`, in order (127.0.0.1 then ::1 for each), asking
    /// the helper only for ports not held yet. The copies are the caller's to close, so
    /// a `release_except` meanwhile cannot close a socket a unit is about to inherit.
    pub async fn get(&self, ports: &[u16]) -> Result<Vec<OwnedFd>, YardError> {
        for &port in ports {
            if lock(&self.held).contains_key(&port) {
                continue;
            }
            let fds = self.helper.bind_port(port).await?;
            lock(&self.held).insert(port, fds);
        }
        let held = lock(&self.held);
        let mut copies = vec![];
        for fd in ports.iter().filter_map(|p| held.get(p)).flatten() {
            copies
                .push(fd.try_clone().map_err(|e| YardError::internal(format!("cannot copy a socket: {e}")))?);
        }
        Ok(copies)
    }

    /// Closes yardd's sockets for `ports`.
    pub fn release(&self, ports: &[u16]) {
        lock(&self.held).retain(|port, _| !ports.contains(port));
    }

    /// Closes yardd's sockets for every port but `keep`: a port nginx no longer serves
    /// on goes back to the system.
    pub fn release_except(&self, keep: &[u16]) {
        lock(&self.held).retain(|port, _| keep.contains(port));
    }
}
