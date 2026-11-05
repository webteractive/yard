//! The tokio runtime yard-client needs, beside GPUI's own executor (M6 design §4.2). GPUI tasks
//! await a `JoinHandle` from here; tokio's channels wake whoever waits, on either side.

use std::future::Future;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

pub struct Bridge {
    pub runtime: Runtime,
}

/// The app's one bridge, kept as a GPUI global so its runtime lives as long as the app.
pub struct AppBridge(pub std::sync::Arc<Bridge>);

impl gpui::Global for AppBridge {}

impl Bridge {
    /// Two worker threads named `yard-io`: the app's I/O is a few sockets and a child process.
    pub fn new() -> Bridge {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("yard-io")
            .enable_all()
            .build()
            .expect("a tokio runtime for yard-io");
        Bridge { runtime }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the app's daemon adapter (M6 Task 10) runs calls through it")
    )]
    pub fn spawn<F>(&self, f: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    async fn a_tokio_task_answers_a_gpui_task(cx: &mut gpui::TestAppContext) {
        // The answer is woken from a tokio thread, outside GPUI's deterministic test scheduler.
        cx.executor().allow_parking();
        let bridge = Bridge::new();
        let answer = bridge
            .spawn(async {
                tokio::task::yield_now().await;
                42
            })
            .await
            .unwrap();
        assert_eq!(answer, 42);
    }
}
