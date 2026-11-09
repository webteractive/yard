//! Overview (M6 design §8.2): which yardd and which app, the data root, the units, and starting,
//! stopping and restarting yardd.

use crate::env::AppEnv;
use crate::runner::ActionId;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::error_view::ErrorView;
use crate::widgets::table::{Row, Table};
use crate::widgets::text;
use crate::widgets::theme::Theme;
use gpui::{
    AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, prelude::*,
    px,
};
use yard_app_model::hint::HintAction;
use yard_app_model::refresh::ResourceKey;
use yard_client::api;
use yard_proto::methods::UnitParams;

/// The actions whose errors the Overview shows beside its controls.
pub const ERRORS: [&str; 5] = ["yardd:start", "yardd:stop", "yardd:restart", "ca:trust", "ca:untrust"];

/// The CA line's text and the button that changes it.
pub fn ca_text(ca: &yard_proto::CaInfo) -> (String, &'static str) {
    if ca.trusted {
        (format!("trusted, expires {}", ca.expires), "Untrust")
    } else {
        (format!("not trusted, expires {}", ca.expires), "Trust")
    }
}

pub fn overview(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let s = env.state.read(cx);
    let line = |label: &str, value: String| {
        div()
            .flex()
            .gap(px(8.))
            .child(
                div()
                    .w(px(110.))
                    .text_color(t.muted)
                    .child(text(format!("{label}:label"), label.to_string())),
            )
            .child(text(format!("{label}:value"), value))
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
    if let Some(ca) = &s.ca.value {
        let (value, verb) = ca_text(ca);
        let env2 = env.clone();
        let id = format!("ca:{}", verb.to_lowercase());
        let button = Button::new(id.clone(), verb).on_click(move |_, _, cx| {
            env2.run_cli_refreshing(&id, vec![verb.to_lowercase()], vec![ResourceKey::Ca], cx)
        });
        facts = facts.child(line("CA", value).child(button));
    }
    if let Some(item) = s.update.as_ref().and_then(yard_app_model::update::item) {
        let env2 = env.clone();
        let latest = s.update.as_ref().map(|c| c.latest.clone()).unwrap_or_default();
        let button = Button::new("update:install", item.title)
            .description(format!("installs Yard {latest} and relaunches Yard.app"))
            .on_click(move |_, _, cx| env2.update_now(cx));
        facts = facts.child(
            line("Update", format!("Yard {latest} is available"))
                .child(button)
                .debug_selector(|| "update".into()),
        );
    }
    facts = facts
        .child(line("Yard.app", format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("YARD_BUILD_COMMIT"))))
        .child(line("Data root", env.location.root.display().to_string()));
    let (start, stop, restart, setup) = (env.clone(), env.clone(), env.clone(), env.clone());
    let controls = div()
        .flex()
        .gap(px(8.))
        .child(Button::new("yardd:start", "Start yardd").on_click(move |_, _, cx| start.start_daemon(cx)))
        .child(
            Button::new("yardd:restart", "Restart yardd")
                .on_click(move |_, _, cx| restart.restart_daemon(cx)),
        )
        .child(
            Button::new("setup:open", "Set up Yard…")
                .on_click(move |_, _, cx| crate::onboarding::open(&setup, cx)),
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
    let errors = ERRORS.into_iter().filter_map(|id| {
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

    /// The Overview says whether this Mac trusts Yard's CA, with Trust or Untrust (design §8.2;
    /// M6 final review: only onboarding showed it).
    #[test]
    fn the_ca_line_says_whether_it_is_trusted() {
        let ca = |trusted| yard_proto::CaInfo {
            cert: "/r/ca/yard-ca.pem".into(),
            sha256: "AB".into(),
            trusted,
            expires: "2036-11-06".into(),
        };
        assert_eq!(super::ca_text(&ca(true)), ("trusted, expires 2036-11-06".to_string(), "Untrust"));
        assert_eq!(super::ca_text(&ca(false)), ("not trusted, expires 2036-11-06".to_string(), "Trust"));
        let keys = crate::window::main::visible_keys(&yard_app_model::hint::Screen::Overview);
        assert!(keys.contains(&yard_app_model::refresh::ResourceKey::Ca), "{keys:?}");
    }

    /// yard trust and untrust change no list yardd sends an event for: the CA is fetched again
    /// when they end, whatever their result.
    #[gpui::test]
    async fn a_cli_run_refetches_what_it_changed(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        let before = fake.count("ca.info");
        cx.update(|cx| {
            env.run_cli_refreshing(
                "ca:trust",
                vec!["trust".into()],
                vec![yard_app_model::refresh::ResourceKey::Ca],
                cx,
            )
        });
        cx.run_until_parked();
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(fake.count("ca.info"), before + 1);
    }

    #[gpui::test]
    async fn an_available_update_shows_on_the_overview(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        env.state.update(cx, |s, _| {
            s.update = Some(yard_app_model::update::Check {
                installed: "0.1.0".into(),
                latest: "0.2.0".into(),
                available: true,
                kind: "bundle".into(),
                notes_url: "x".into(),
            })
        });
        let (_v, vcx) = cx.add_window_view(|_, _| Host(env));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("update").is_some(), "the update line is drawn");
    }

    #[gpui::test]
    async fn overview_renders(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let (_v, cx) = cx.add_window_view(|_, _| Host(env));
        cx.run_until_parked();
    }
}
