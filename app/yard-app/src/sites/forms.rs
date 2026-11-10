//! The Sites screen's sheets: link a folder, add a proxy, install what a site needs.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::widgets::form::Form;
use crate::widgets::theme::Theme;
use crate::widgets::theme::Typography;
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div};
use yard_app_model::settle::SettlePlan;
use yard_client::api;
use yard_proto::methods::ProxyParams;

fn close(env: &AppEnv, cx: &mut App) {
    env.nav.update(cx, |n, cx| {
        n.sheet = None;
        cx.notify();
    });
}

pub fn sheet(env: &AppEnv, sheet: &Sheet, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let t = Theme::of(window);
    let cancel = env.clone();
    let form = match sheet {
        Sheet::Link { folder, name } => {
            let (submit, folder, name) = (env.clone(), folder.clone(), name.clone());
            Form::new("sheet:link", "Link a folder")
                .field(
                    div()
                        .type_body()
                        .text_color(t.c.text_secondary)
                        .child(crate::widgets::text("sheet:link:folder", folder.display().to_string())),
                )
                .field(name.clone())
                .submit("Link", false, move |_, _, cx| {
                    let site = name.read(cx).value().trim().to_string();
                    let env = submit.clone();
                    let task = env.runner.update(cx, |r, cx| r.link(folder.clone(), site, cx));
                    cx.spawn(async move |cx| {
                        if let Some(site) = task.await {
                            cx.update(|cx| {
                                close(&env, cx);
                                crate::window::main::open(
                                    &env,
                                    yard_app_model::hint::Screen::Sites(Some(site.name)),
                                    cx,
                                );
                            });
                        }
                    })
                    .detach();
                })
        }
        Sheet::Proxy { name, url } => {
            let (submit, name, url) = (env.clone(), name.clone(), url.clone());
            Form::new("sheet:proxy", "Add a proxy").field(name.clone()).field(url.clone()).submit(
                "Add",
                false,
                move |_, _, cx| {
                    let params = ProxyParams {
                        name: name.read(cx).value().trim().to_string(),
                        url: url.read(cx).value().trim().to_string(),
                    };
                    let daemon = submit.daemon.clone();
                    submit.runner.update(cx, |r, cx| {
                        r.perform(
                            ActionId("site:proxy".into()),
                            async move { call(&*daemon, api::SITES_PROXY, params).await.map(|_| ()) },
                            cx,
                        )
                        .detach()
                    });
                    close(&submit, cx);
                },
            )
        }
        Sheet::Settle(site) => {
            let found = env.state.read(cx).sites.value.iter().flatten().find(|s| &s.name == site).cloned()?;
            let plan = SettlePlan::new(&found)?;
            let mut form = Form::new("sheet:settle", format!("{} needs", found.name));
            for (i, line) in plan.lines.iter().enumerate() {
                form = form.field(
                    div()
                        .type_body()
                        .child(crate::widgets::text(format!("sheet:settle:line:{i}"), line.clone())),
                );
            }
            for (i, need) in plan.manual.iter().enumerate() {
                form = form.field(div().type_secondary().text_color(t.c.text_secondary).child(
                    crate::widgets::text(
                        format!("sheet:settle:manual:{i}"),
                        format!("Not installed by Yard: {}", need.describe()),
                    ),
                ));
            }
            let submit = env.clone();
            let form = form.submit("Install", false, move |_, _, cx| {
                let task = submit.runner.update(cx, |r, cx| r.settle(found.clone(), cx));
                task.detach();
                close(&submit, cx);
            });
            if let Some(note) = plan.not_now {
                let (later, id) = (env.clone(), ActionId(format!("site:{site}:node")));
                return Some(
                    form.cancel_label("Not now")
                        .on_cancel(move |_, _, cx| {
                            not_now(&later, id.clone(), note.clone(), cx);
                            close(&later, cx);
                        })
                        .into_any_element(),
                );
            }
            form
        }
        _ => return None,
    };
    Some(form.on_cancel(move |_, _, cx| close(&cancel, cx)).into_any_element())
}

/// **Not now** on a settle whose Node is missing: the site serves, and the note stays beside it
/// with the install command (design §11.1).
pub fn not_now(env: &AppEnv, id: ActionId, note: yard_proto::YardError, cx: &mut App) {
    env.runner.update(cx, |r, cx| {
        r.errors.record(id, note);
        cx.notify();
    });
}
