//! A live log tail: the last lines a service wrote, then `log_line` events as they come, on a
//! connection of its own that closes when the tail is dropped and is opened again after yardd
//! restarts.

use crate::daemon::call;
use crate::env::AppEnv;
use gpui::{Context, Task};
use yard_app_model::logs::LogBuffer;
use yard_client::api;
use yard_proto::Event;
use yard_proto::methods::ServiceLogsParams;

/// How long a tail waits before it opens a new stream.
const RETRY: std::time::Duration = std::time::Duration::from_secs(2);

pub struct LogTail {
    pub buffer: LogBuffer,
    _task: Task<()>,
}

impl LogTail {
    /// Lines of `unit` (all units when `None`); with `service`, its last 100 lines first.
    pub fn new(
        env: &AppEnv,
        unit: Option<String>,
        service: Option<String>,
        cx: &mut Context<Self>,
    ) -> LogTail {
        let daemon = env.daemon.clone();
        let task = cx.spawn(async move |this, cx| {
            if let Some(name) = service {
                let last = call(
                    &*daemon,
                    api::SERVICES_LOGS,
                    ServiceLogsParams { name: name.clone(), lines: Some(100) },
                )
                .await;
                if let Ok(lines) = last {
                    let _ = this.update(cx, |t, cx| {
                        for l in &lines {
                            t.buffer.push(&name, l);
                        }
                        cx.notify();
                    });
                }
            }
            // A stream ends when yardd goes (a restart, a stop): open another once it is back.
            // The tail's task, and with it this loop, ends when the tail is dropped.
            loop {
                if let Ok(mut rx) = daemon.logs().await {
                    while let Some(event) = rx.recv().await {
                        if let Event::LogLine { unit: u, line } = event
                            && unit.as_ref().is_none_or(|want| *want == u)
                            && this
                                .update(cx, |t, cx| {
                                    t.buffer.push(&u, &line);
                                    cx.notify();
                                })
                                .is_err()
                        {
                            return;
                        }
                    }
                }
                cx.background_executor().timer(RETRY).await;
            }
        });
        LogTail { buffer: LogBuffer::default(), _task: task }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::env;
    use gpui::{AppContext, TestAppContext};
    use std::time::Duration;

    fn lines(tail: &gpui::Entity<LogTail>, cx: &mut TestAppContext) -> Vec<String> {
        tail.read_with(cx, |t, _| t.buffer.visible().map(|l| l.text.clone()).collect())
    }

    /// The tail opens a new stream when yardd restarts, rather than going silent (M6 final review).
    #[gpui::test]
    async fn a_tail_follows_yardd_through_a_restart(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        let tail = cx.new(|cx| LogTail::new(&env, None, None, cx));
        cx.run_until_parked();
        fake.log_line("nginx", "before");
        cx.run_until_parked();
        fake.drop_logs();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        assert_eq!(fake.logs_opened(), 2, "a second stream");
        fake.log_line("nginx", "after");
        cx.run_until_parked();
        assert_eq!(lines(&tail, cx), ["before", "after"]);
    }
}
