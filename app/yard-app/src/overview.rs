//! Overview (M6 design §8.2): which yardd and which app, the data root, the units, and starting,
//! stopping and restarting yardd.

use crate::env::AppEnv;
use crate::runner::ActionId;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::error_view::ErrorView;
use crate::widgets::table::{Row, Table};
use crate::widgets::theme::Theme;
use gpui::{
    AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, prelude::*,
    px,
};
use yard_app_model::hint::HintAction;
use yard_client::api;
use yard_proto::methods::UnitParams;

pub fn overview(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let s = env.state.read(cx);
    let line = |label: &str, value: String| {
        div()
            .flex()
            .gap(px(8.))
            .child(div().w(px(110.)).text_color(t.muted).child(label.to_string()))
            .child(value)
    };
    let mut facts = div().flex().flex_col().gap(px(4.)).text_sm();
    if let Some(st) = &s.status.value {
        facts = facts
            .child(line(
                "yardd",
                format!("{} ({})", st.version.daemon.clone().unwrap_or_default(), st.version.commit),
            ))
            .child(line("helper", st.version.helper.clone().unwrap_or_else(|| "not installed".into())))
            .child(line("Profile", format!("{} · .{}", st.profile.name(), st.tld)));
    }
    facts = facts
        .child(line("Yard.app", format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("YARD_BUILD_COMMIT"))))
        .child(line("Data root", env.location.root.display().to_string()));
    let (start, stop, restart) = (env.clone(), env.clone(), env.clone());
    let controls = div()
        .flex()
        .gap(px(8.))
        .child(Button::new("yardd:start", "Start yardd").on_click(move |_, _, cx| start.start_daemon(cx)))
        .child(
            Button::new("yardd:restart", "Restart yardd")
                .on_click(move |_, _, cx| restart.restart_daemon(cx)),
        )
        .child(Button::new("yardd:stop", "Stop yardd…").style(ButtonStyle::Destructive).on_click(
            move |_, window, cx| {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "Stop yardd?",
                    Some("Every site stops answering until yardd starts again."),
                    &["Stop yardd", "Cancel"],
                    cx,
                );
                let env = stop.clone();
                cx.spawn(async move |cx| {
                    if answer.await == Ok(0) {
                        cx.update(|cx| env.stop_daemon(cx));
                    }
                })
                .detach();
            },
        ));
    let rows = s.status.value.iter().flat_map(|st| st.units.iter()).map(|u| {
        let env = env.clone();
        let unit = u.name.clone();
        Row::new(
            u.name.clone(),
            vec![
                u.name.clone().into(),
                u.state.name().into(),
                u.pid.map_or("-".into(), |p| p.to_string()).into(),
                u.restarts.to_string().into(),
            ],
        )
        .extra(Button::new(format!("unit:{}:restart", u.name), "Restart").on_click(
            move |_, _, cx| {
                let daemon = env.daemon.clone();
                let unit = unit.clone();
                env.runner.update(cx, |r, cx| {
                    r.perform(
                        ActionId(format!("unit:{unit}:restart")),
                        async move {
                            crate::daemon::call(&*daemon, api::UNITS_RESTART, UnitParams { unit })
                                .await
                                .map(|_| ())
                        },
                        cx,
                    )
                    .detach()
                });
            },
        ))
    });
    let units = Table::new(
        "units",
        vec!["Unit".into(), "State".into(), "PID".into(), "Restarts".into()],
        rows.collect(),
    );
    let sleeping: Vec<SharedString> = s
        .status
        .value
        .iter()
        .flat_map(|st| st.sleeping.iter())
        .map(|x| SharedString::from(x.name.clone()))
        .collect();
    let errors = ["yardd:start", "yardd:stop", "yardd:restart"].into_iter().filter_map(|id| {
        let e = env.runner.read(cx).errors.get(&ActionId(id.into()))?.clone();
        let env = env.clone();
        Some(
            ErrorView::new(format!("{id}:error"), &e, HintAction::parse(e.hint.as_deref().unwrap_or("")))
                .on_action(move |a, _, cx| env.perform(a, cx)),
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(div().text_xl().child("Overview"))
        .child(facts)
        .child(controls)
        .children(errors)
        .child(units)
        .when(!sleeping.is_empty(), |d| {
            d.child(div().text_sm().text_color(t.muted).child(format!("Sleeping: {}", sleeping.join(", "))))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::testing::env;
    use gpui::{Context, IntoElement, Render, TestAppContext, Window};

    struct Host(crate::env::AppEnv);
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            super::overview(&self.0, window, cx)
        }
    }

    #[gpui::test]
    async fn overview_renders(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let (_v, cx) = cx.add_window_view(|_, _| Host(env));
        cx.run_until_parked();
    }
}
