//! The Services screen's sheets: add a service, move it to another port, change when it sleeps.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::widgets::form::Form;
use crate::widgets::picker::Picker;
use crate::widgets::text_field::TextField;
use gpui::{AnyElement, App, Entity, IntoElement, SharedString, Window};
use yard_app_model::actions::port_input;
use yard_app_model::jobs::JobPhase;
use yard_client::api;
use yard_proto::methods::{ServiceAddParams, ServicePortParams, ServiceSleepParams};
use yard_proto::{ErrorCode, YardError, engine_display};

pub const ENGINES: [&str; 5] = ["mysql", "postgres", "redis", "minio", "emailulator"];

fn close(env: &AppEnv, cx: &mut App) {
    env.nav.update(cx, |n, cx| {
        n.sheet = None;
        cx.notify();
    });
}

type Field = Entity<TextField>;

/// The last error of the sheet's action, shown in the sheet.
fn error(env: &AppEnv, id: &str, cx: &App) -> Option<SharedString> {
    env.runner.read(cx).errors.get(&ActionId(id.into())).map(|e| SharedString::from(e.message.clone()))
}

/// The field's port (`Some(None)` when empty), or `None` with the error recorded under `id` for
/// the sheet to show.
fn port_or_say(env: &AppEnv, id: &str, field: &Field, cx: &mut App) -> Option<Option<u16>> {
    match port_input(field.read(cx).value()) {
        Ok(port) => Some(port),
        Err(e) => {
            env.runner.update(cx, |r, cx| {
                r.errors.record(ActionId(id.into()), e);
                cx.notify();
            });
            None
        }
    }
}

/// A Move sheet's port, which cannot be left empty.
fn port_to_move_to(env: &AppEnv, id: &str, field: &Field, cx: &mut App) -> Option<u16> {
    match port_or_say(env, id, field, cx)? {
        Some(port) => Some(port),
        None => {
            env.runner.update(cx, |r, cx| {
                r.errors.record(
                    ActionId(id.into()),
                    YardError::new(ErrorCode::InvalidArgument, "give the port to move to"),
                );
                cx.notify();
            });
            None
        }
    }
}

/// Move: `services.port` for the instance; the sheet closes once it is sent.
pub fn move_service(env: &AppEnv, name: &str, field: &Field, cx: &mut App) {
    let id = format!("service:{name}:port");
    let Some(port) = port_to_move_to(env, &id, field, cx) else { return };
    let (daemon, n) = (env.daemon.clone(), name.to_string());
    env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(id),
            async move {
                call(&*daemon, api::SERVICES_PORT, ServicePortParams { name: n, port }).await.map(|_| ())
            },
            cx,
        )
        .detach()
    });
    close(env, cx);
}

/// Move for a unit that is not a service instance: `ports.set`.
pub fn move_unit(env: &AppEnv, unit: &str, field: &Field, cx: &mut App) {
    let id = format!("port:{unit}:set");
    let Some(port) = port_to_move_to(env, &id, field, cx) else { return };
    let (daemon, u) = (env.daemon.clone(), unit.to_string());
    env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(id),
            async move {
                call(&*daemon, api::PORTS_SET, yard_proto::methods::PortSetParams { unit: u, port })
                    .await
                    .map(|_| ())
            },
            cx,
        )
        .detach()
    });
    close(env, cx);
}

/// Add: `services.add` as a job; a refused line offers the lines yardd has, and success closes
/// the sheet.
pub fn add_service(env: &AppEnv, engine: usize, line: &Field, port: &Field, cx: &mut App) {
    let Some(port) = port_or_say(env, "service:add", port, cx) else { return };
    let version = Some(line.read(cx).value().trim().to_string()).filter(|v| !v.is_empty());
    let params = ServiceAddParams { engine: ENGINES[engine].to_string(), version, port, path: None };
    let title = format!("Add {}", engine_display(ENGINES[engine]));
    let env = env.clone();
    let task = env
        .runner
        .update(cx, |r, cx| r.run_job(ActionId("service:add".into()), api::SERVICES_ADD, params, title, cx));
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
}

pub fn sheet(env: &AppEnv, sheet: &Sheet, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
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
            form.field(port.clone()).error(error(env, "service:add", cx)).submit(
                "Add",
                false,
                move |_, _, cx| add_service(&submit, engine, &line, &port, cx),
            )
        }
        Sheet::Port { name, port } => {
            let (submit, name, port) = (env.clone(), name.clone(), port.clone());
            Form::new("sheet:port", format!("Move {name} to another port"))
                .field(port.clone())
                .error(error(env, &format!("service:{name}:port"), cx))
                .submit("Move", false, move |_, _, cx| move_service(&submit, &name, &port, cx))
        }
        Sheet::UnitPort { unit, port } => {
            let (submit, unit, port) = (env.clone(), unit.clone(), port.clone());
            Form::new("sheet:unit-port", format!("Move {unit} to another port"))
                .field(port.clone())
                .error(error(env, &format!("port:{unit}:set"), cx))
                .submit("Move", false, move |_, _, cx| move_unit(&submit, &unit, &port, cx))
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
