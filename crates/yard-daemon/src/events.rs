//! Fan-out of daemon events to RPC subscribers. A slow subscriber lags and loses old
//! events rather than holding the daemon back.

use tokio::sync::broadcast;
use yard_proto::Event;

const CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        Self { tx: broadcast::channel(CAPACITY).0 }
    }

    /// Having no subscribers is normal, not an error.
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
