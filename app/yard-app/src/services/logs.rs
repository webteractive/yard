//! A live log tail: the last lines a service wrote, then `log_line` events as they come, on a
//! connection of its own that closes when the tail is dropped.

use crate::daemon::call;
use crate::env::AppEnv;
use gpui::{Context, Task};
use yard_app_model::logs::LogBuffer;
use yard_client::api;
use yard_proto::Event;
use yard_proto::methods::ServiceLogsParams;

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
            let Ok(mut rx) = daemon.logs().await else { return };
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
        });
        LogTail { buffer: LogBuffer::default(), _task: task }
    }
}
