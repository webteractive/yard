//! The Services screen's sheets: add a service, move it to another port, change when it sleeps.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::widgets::form::Form;
use crate::widgets::picker::Picker;
use gpui::{AnyElement, App, IntoElement, SharedString, Window};
use yard_app_model::jobs::JobPhase;
use yard_client::api;
use yard_proto::methods::{ServiceAddParams, ServicePortParams, ServiceSleepParams};
use yard_proto::{ErrorCode, engine_display};

pub const ENGINES: [&str; 5] = ["mysql", "postgres", "redis", "minio", "emailulator"];

fn close(env: &AppEnv, cx: &mut App) {
    env.nav.update(cx, |n, cx| {
        n.sheet = None;
        cx.notify();
    });
}

pub fn sheet(env: &AppEnv, sheet: &Sheet, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
    let cancel = env.clone();
    let form = match sheet {
        Sheet::AddService { engine, line, port, lines } => {
            let names: Vec<SharedString> =
                ENGINES.iter().map(|e| SharedString::from(engine_display(e).to_string())).collect();
            let pick = env.clone();
            let mut form = Form::new("sheet:add-service", "Add a service").field(
                Picker::new("sheet:add-service:engine", "Engine", names, Some(*engine)).on_select(
                    move |i, _, cx| {
                        pick.nav.update(cx, |n, cx| {
                            if let Some(Sheet::AddService { engine, lines, .. }) = &mut n.sheet {
                                *engine = i;
                                lines.clear();
                            }
                            cx.notify();
                        })
                    },
                ),
            );
            if lines.is_empty() {
                form = form.field(line.clone());
            } else {
                let options: Vec<SharedString> =
                    lines.iter().map(|l| SharedString::from(l.clone())).collect();
                let (pick, line) = (env.clone(), line.clone());
                let all = lines.clone();
                form = form.field(Picker::new("sheet:add-service:line", "Line", options, None).on_select(
                    move |i, _, cx| {
                        let chosen = all[i].clone();
                        line.update(cx, |f, cx| f.set_value(chosen, cx));
                        pick.nav.update(cx, |n, cx| {
                            if let Some(Sheet::AddService { lines, .. }) = &mut n.sheet {
                                lines.clear();
                            }
                            cx.notify();
                        });
                    },
                ));
            }
            let (submit, line, port, engine) = (env.clone(), line.clone(), port.clone(), *engine);
            form.field(port.clone()).submit("Add", false, move |_, _, cx| {
                let text = |f: &gpui::Entity<crate::widgets::text_field::TextField>, cx: &App| {
                    let v = f.read(cx).value().trim().to_string();
                    (!v.is_empty()).then_some(v)
                };
                let params = ServiceAddParams {
                    engine: ENGINES[engine].to_string(),
                    version: text(&line, cx),
                    port: text(&port, cx).and_then(|p| p.parse().ok()),
                    path: None,
                };
                let title = format!("Add {}", engine_display(ENGINES[engine]));
                let env = submit.clone();
                let task = env.runner.update(cx, |r, cx| {
                    r.run_job(ActionId("service:add".into()), api::SERVICES_ADD, params, title, cx)
                });
                cx.spawn(async move |cx| {
                    let phase = task.await;
                    cx.update(|cx| match phase {
                        JobPhase::Failed(e) if e.code == ErrorCode::ServiceVersionUnsupported => {
                            let offered: Vec<String> = e
                                .data
                                .as_ref()
                                .and_then(|d| d.get("available"))
                                .and_then(|a| a.as_array())
                                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                                .unwrap_or_default();
                            env.nav.update(cx, |n, cx| {
                                if let Some(Sheet::AddService { lines, .. }) = &mut n.sheet {
                                    *lines = offered;
                                }
                                cx.notify();
                            });
                        }
                        JobPhase::Succeeded(_) => close(&env, cx),
                        _ => {}
                    })
                })
                .detach();
            })
        }
        Sheet::Port { name, port } => {
            let (submit, name, port) = (env.clone(), name.clone(), port.clone());
            Form::new("sheet:port", format!("Move {name} to another port")).field(port.clone()).submit(
                "Move",
                false,
                move |_, _, cx| {
                    let Ok(number) = port.read(cx).value().trim().parse::<u16>() else { return };
                    let (daemon, n) = (submit.daemon.clone(), name.clone());
                    submit.runner.update(cx, |r, cx| {
                        r.perform(
                            ActionId(format!("service:{n}:port")),
                            async move {
                                call(
                                    &*daemon,
                                    api::SERVICES_PORT,
                                    ServicePortParams { name: n, port: number },
                                )
                                .await
                                .map(|_| ())
                            },
                            cx,
                        )
                        .detach()
                    });
                    close(&submit, cx);
                },
            )
        }
        Sheet::Sleep { name, after } => {
            let (submit, name, after) = (env.clone(), name.clone(), after.clone());
            Form::new("sheet:sleep", format!("When {name} sleeps")).field(after.clone()).submit(
                "Set",
                false,
                move |_, _, cx| {
                    let value = after.read(cx).value().trim().to_string();
                    let (daemon, n) = (submit.daemon.clone(), name.clone());
                    submit.runner.update(cx, |r, cx| {
                        r.perform(
                            ActionId(format!("service:{n}:sleep")),
                            async move {
                                call(
                                    &*daemon,
                                    api::SERVICES_SLEEP_SET,
                                    ServiceSleepParams { name: n, after: value },
                                )
                                .await
                                .map(|_| ())
                            },
                            cx,
                        )
                        .detach()
                    });
                    close(&submit, cx);
                },
            )
        }
        _ => return None,
    };
    Some(form.on_cancel(move |_, _, cx| close(&cancel, cx)).into_any_element())
}
