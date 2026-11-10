//! Overview (M6 design §8.2): which yardd and which app, the data root, the units, and starting,
//! stopping and restarting yardd.

use crate::env::AppEnv;
use crate::runner::ActionId;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::error_view::ErrorView;
use crate::widgets::group::Group;
use crate::widgets::table::{Column, Row, Table};
use crate::widgets::text;
use crate::widgets::theme::Theme;
use crate::widgets::theme::Typography;
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
    let value = |label: &str, v: String| text(format!("{label}:value"), v);
    let mut facts = Group::new("facts", None);
    if let Some(st) = &s.status.value {
        facts = facts
            .row(
                "yardd",
                "yardd",
                value(
                    "yardd",
                    format!("{} ({})", st.version.daemon.clone().unwrap_or_default(), st.version.commit),
                ),
            )
            .row(
                "helper",
                "helper",
                value("helper", st.version.helper.clone().unwrap_or_else(|| "not installed".into())),
            )
            .row("Profile", "Profile", value("Profile", format!("{} · .{}", st.profile.name(), st.tld)));
    }
    if let Some(ca) = &s.ca.value {
        let (v, verb) = ca_text(ca);
        let env2 = env.clone();
        let id = format!("ca:{}", verb.to_lowercase());
        let button = Button::new(id.clone(), verb).small().on_click(move |_, _, cx| {
            env2.run_cli_refreshing(&id, vec![verb.to_lowercase()], vec![ResourceKey::Ca], cx)
        });
        facts = facts.row(
            "CA",
            "CA",
            div().flex_1().flex().items_center().justify_between().child(value("CA", v)).child(button),
        );
    }
    let mut update_line = None;
    if let Some(item) = s.update.as_ref().and_then(yard_app_model::update::item) {
        let env2 = env.clone();
        let latest = s.update.as_ref().map(|c| c.latest.clone()).unwrap_or_default();
        let button = Button::new("update:install", item.title)
            .small()
            .style(ButtonStyle::Primary)
            .description(format!("installs Yard {latest} and relaunches Yard.app"))
            .on_click(move |_, _, cx| env2.update_now(cx));
        update_line = Some(
            div()
                .debug_selector(|| "update".into())
                .flex_1()
                .flex()
                .items_center()
                .justify_between()
                .child(value("Update", format!("Yard {latest} is available")))
                .child(button),
        );
    }
    if let Some(line) = update_line {
        facts = facts.row("Update", "Update", line);
    }
    facts = facts
        .row(
            "Yard.app",
            "Yard.app",
            value("Yard.app", format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("YARD_BUILD_COMMIT"))),
        )
        .row("Data root", "Data root", value("Data root", env.location.root.display().to_string()));
    // Start while yardd cannot be reached, Restart and Stop while it answers (G12).
    let offline = matches!(crate::menubar::link(&s.connection), yard_app_model::health::Link::Offline(_));
    let (start, stop, restart, setup) = (env.clone(), env.clone(), env.clone(), env.clone());
    let controls = crate::widgets::bar()
        .when(offline, |d| {
            d.child(
                Button::new("yardd:start", "Start yardd")
                    .style(ButtonStyle::Primary)
                    .on_click(move |_, _, cx| start.start_daemon(cx)),
            )
        })
        .when(!offline, |d| {
            d.child(
                Button::new("yardd:restart", "Restart yardd")
                    .on_click(move |_, _, cx| restart.restart_daemon(cx)),
            )
        })
        .child(
            Button::new("setup:open", "Set up Yard…")
                .on_click(move |_, _, cx| crate::onboarding::open(&setup, cx)),
        )
        .when(!offline, |d| {
            d.child(Button::new("yardd:stop", "Stop yardd…").style(ButtonStyle::Destructive).on_click(
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
            ))
        });
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
        .extra(Button::new(format!("unit:{}:restart", u.name), "Restart").small().on_click(
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
        [
            Column::flex("Unit").grow(2.),
            Column::fixed("State", 110.).pill(),
            Column::fixed("PID", 70.).end(),
            Column::fixed("Restarts", 70.).end(),
        ],
        rows.collect(),
    )
    .actions(90.)
    .empty("No units: yardd is not running.", None);
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
        .child(div().type_title().child("Overview"))
        .child(facts)
        .child(controls)
        .children(errors)
        .child(units)
        .when(!sleeping.is_empty(), |d| {
            d.child(div().type_body().text_color(t.muted).child(format!("Sleeping: {}", sleeping.join(", "))))
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

    /// Start yardd is offered only while yardd is unreachable, Restart and Stop only while it
    /// answers (G12: Start was enabled while yardd ran).
    #[gpui::test]
    async fn start_is_offered_only_while_offline(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        let (_v, vcx) = cx.add_window_view(|_, _| Host(env));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("yardd:start").is_none(), "Start while yardd answers");
        assert!(vcx.debug_bounds("yardd:stop").is_some() && vcx.debug_bounds("yardd:restart").is_some());
        fake.push(yard_client::session::Update::State(yard_client::session::ConnectionState::Disconnected(
            yard_proto::YardError::internal("yardd is not running"),
        )));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("yardd:start").is_some(), "no Start while offline");
        assert!(vcx.debug_bounds("yardd:stop").is_none() && vcx.debug_bounds("yardd:restart").is_none());
    }

    /// Every fact row is one height, the CA's with its button too (G14).
    #[gpui::test]
    async fn overview_fact_rows_are_one_height(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        env.state.update(cx, |s, cx| {
            s.status.value = Some(serde_json::from_value(crate::testing::status()).unwrap());
            s.ca.value = Some(yard_proto::CaInfo {
                cert: "/r/ca/yard-ca.pem".into(),
                sha256: "AB".into(),
                trusted: false,
                expires: "2036-11-06".into(),
            });
            cx.notify();
        });
        cx.run_until_parked();
        let (_v, vcx) = cx.add_window_view(|_, _| Host(env));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("facts:row:Yard.app").is_some(), "the Yard.app row");
        let helper = vcx.debug_bounds("facts:row:helper").expect("the helper row");
        let ca = vcx.debug_bounds("facts:row:CA").expect("the CA row");
        assert_eq!(ca.size.height, helper.size.height);
    }
}
