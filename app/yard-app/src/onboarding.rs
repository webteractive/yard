//! Onboarding (M6 design §8.4): a checklist computed from facts each time it is drawn.

use crate::env::AppEnv;
use crate::platform::activation;
use crate::platform::open::Opener;
use crate::platform::setup_probe::brew_installed;
use crate::screens::quick;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::pill::Pill;
use crate::widgets::theme::Typography;
use crate::widgets::theme::{Theme, Tone};
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Image, ImageFormat, IntoElement, ParentElement,
    PathPromptOptions, Pixels, Render, Styled, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    img, prelude::*, px, size,
};
use std::sync::Arc;
use yard_app_model::hint::{HintAction, TerminalCommand};
use yard_app_model::refresh::ResourceKey;
use yard_app_model::setup::{SetupFacts, SetupStep, SetupStepId, needs_onboarding, steps};
use yard_client::api;
use yard_client::session::ConnectionState;
use yard_proto::Profile;
use yard_proto::methods::ProfileParams;

/// The facts the checklist reads, from the location, the session and the last answers.
pub fn facts(env: &AppEnv, cx: &App) -> SetupFacts {
    let s = env.state.read(cx);
    let connected = matches!(s.connection, ConnectionState::Connected(_));
    SetupFacts {
        brew: brew_installed(),
        yard: env.cli.yard(),
        agent_installed: env.location.agent_installed,
        daemon_answers: connected,
        helper_version: s.status.value.as_ref().and_then(|st| st.version.helper.clone()),
        ca_trusted: s.ca.value.as_ref().map(|c| c.trusted),
        profile: s.status.value.as_ref().map(|st| st.profile),
        site_count: s.sites.value.as_ref().map(Vec::len),
    }
}

fn title(id: SetupStepId) -> &'static str {
    match id {
        SetupStepId::Brew => "Homebrew",
        SetupStepId::Cli => "The yard command",
        SetupStepId::Daemon => "yardd",
        SetupStepId::Helper => "Root helper (names under .yard, and 80/443 if wanted)",
        SetupStepId::Https => "HTTPS (optional)",
        SetupStepId::Shims => "Shims on PATH (optional)",
        SetupStepId::Ports => "Ports 80 and 443 (optional)",
        SetupStepId::FirstSite => "A first site (optional)",
    }
}

fn state(step: &SetupStep) -> &'static str {
    match (step.done, step.blocked) {
        (true, _) => "done",
        (false, true) => "waiting on an earlier step",
        (false, false) => "to do",
    }
}

/// The window's options: its title (for Mission Control and the Window menu) and its size.
pub(crate) fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Set up Yard".into()), ..Default::default() }),
        ..Default::default()
    }
}

/// `<step> — done | to do | waiting on an earlier step`, in order, as the tests read it.
#[cfg(test)]
pub fn step_lines(facts: &SetupFacts) -> Vec<String> {
    steps(facts).iter().map(|s| format!("{} — {}", title(s.id), state(s))).collect()
}

fn action(env: &AppEnv, step: &SetupStep) -> Option<AnyElement> {
    if step.done || step.blocked {
        return None;
    }
    let e = env.clone();
    let button = match step.id {
        SetupStepId::Brew => Button::new("setup:brew", "Copy the install command").on_click(|_, _, cx| {
            Opener::copy(cx, "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"")
        }),
        SetupStepId::Cli => Button::new("setup:cli", "Choose…").on_click(move |_, _, cx| {
            let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use".into()) });
            let env = e.clone();
            cx.spawn(async move |cx| {
                if let Ok(Ok(Some(paths))) = picked.await
                    && let Some(path) = paths.into_iter().next()
                {
                    cx.update(|cx| env.set_yard_path(Some(&path), cx));
                }
            })
            .detach();
        }),
        SetupStepId::Daemon => Button::new("setup:daemon", "Start yardd").style(ButtonStyle::Primary).on_click(move |_, _, cx| e.start_daemon(cx)),
        SetupStepId::Helper => Button::new("setup:helper", "Open in Terminal").style(ButtonStyle::Primary).on_click(move |_, _, cx| {
            e.perform(HintAction::Terminal(TerminalCommand::HelperInstall), cx)
        }),
        SetupStepId::Https => Button::new("setup:https", "Trust Yard's certificate").on_click(move |_, _, cx| e.run_cli("setup:https", vec!["trust".into()], false, cx)),
        SetupStepId::Shims => Button::new("setup:shims", "Copy yard install-shims").on_click(|_, _, cx| Opener::copy(cx, "yard install-shims")),
        SetupStepId::Ports => Button::new("setup:ports", "Use 80 and 443").on_click(move |_, _, cx| {
            quick(&e, "setup:ports".into(), api::PORTS_PROFILE, ProfileParams { profile: Profile::Privileged }, cx)
        }),
        SetupStepId::FirstSite => Button::new("setup:site", "Link a folder…").on_click(move |_, _, cx| crate::sites::link_folder(&e, cx)),
    };
    Some(button.into_any_element())
}

/// Yard's icon, 112 px for the 56 pt heading. The title beside it says the name, so the image adds
/// nothing for VoiceOver (an `img` has no accessibility role).
fn logo() -> Arc<Image> {
    static LOGO: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| {
        Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../macos/AppIcon-112.png").to_vec()))
    })
    .clone()
}

/// The checklist for `facts`, with each step's action while it is to do.
pub fn checklist(env: &AppEnv, facts: &SetupFacts, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let all = steps(facts);
    let ready = all.iter().filter(|s| !s.optional).all(|s| s.done);
    let rows = all.iter().enumerate().map(|(n, s)| {
        let color = if s.blocked { t.c.text_secondary } else { t.c.text };
        // Done is a glyph and a pill, not a colour alone (spec §4, Onboarding).
        let (mark, tone) =
            if s.done { ("✓".to_string(), Tone::Success) } else { ((n + 1).to_string(), Tone::Neutral) };
        let (_, mark_fg, mark_bg) = t.state(tone);
        let note = (s.id == SetupStepId::Helper && !s.done && !s.blocked).then(|| {
            div()
                .debug_selector(|| "helper:note".into())
                .type_secondary()
                .text_color(t.c.text_secondary)
                .child(crate::widgets::text(
                    "helper:note",
                    "macOS asks for your password in Terminal: the helper writes /etc/resolver.",
                ))
        });
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .p(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(t.c.separator)
            .min_w(px(0.))
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(mark_bg)
                    .text_color(mark_fg)
                    .type_caption()
                    .child(mark),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .debug_selector(move || format!("step:{:?}:title", s.id))
                            .type_body()
                            .text_color(color)
                            .child(crate::widgets::text(format!("step:{:?}:title", s.id), title(s.id))),
                    )
                    .children(note),
            )
            .child(Pill::of(format!("step:{:?}:state", s.id), state(s)))
            .children(action(env, s))
    });
    let check = env.clone();
    div()
        .id("onboarding")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(16.))
        .bg(t.c.window_bg)
        .text_color(t.c.text)
        .type_body()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(img(logo()).size(px(56.)).flex_none())
                .child(div().type_title().child(crate::widgets::text("heading", "Set up Yard"))),
        )
        .child(div().type_body().text_color(t.c.text_secondary).child(crate::widgets::text(
            "summary",
            if ready {
                "Yard is set up. The optional steps can wait."
            } else {
                "Each step shows done once it is; do one in Terminal and come back."
            },
        )))
        .children(rows)
        .children(step_errors(env, cx))
        .child(div().flex().gap(px(8.)).child(Button::new("setup:check", "Check again").on_click(
            move |_, _, cx| {
                check.daemon.reconnect_now();
                check.state.update(cx, |s, cx| {
                    s.refresh([ResourceKey::Status, ResourceKey::Ca, ResourceKey::Sites], cx)
                });
            },
        )))
        .into_any_element()
}

pub struct OnboardingView {
    env: AppEnv,
}

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let facts = facts(&self.env, cx);
        if steps(&facts).iter().filter(|s| !s.optional).all(|s| s.done) {
            self.env.settings.set_did_onboard();
        }
        checklist(&self.env, &facts, window, cx)
    }
}

/// What the steps' actions report when they fail: Start yardd, the helper's Terminal, Trust, the
/// ports profile and a first link, shown in the window they were pressed in.
const STEP_ERRORS: [&str; 4] = ["yardd:start", "terminal", "setup:", "site:link"];

pub fn step_errors(env: &AppEnv, cx: &App) -> Vec<AnyElement> {
    STEP_ERRORS.iter().flat_map(|prefix| crate::screens::errors(env, prefix, cx)).collect()
}

/// Opens the onboarding window.
struct OnboardingHandle(gpui::WindowHandle<OnboardingView>);

impl gpui::Global for OnboardingHandle {}

/// Opens the checklist, or raises it when it is open (from the menu's or the Overview's Set up
/// Yard…, or by itself at launch).
pub fn open(env: &AppEnv, cx: &mut App) {
    env.state.update(cx, |s, cx| s.refresh([ResourceKey::Status, ResourceKey::Ca, ResourceKey::Sites], cx));
    if let Some(handle) = cx.try_global::<OnboardingHandle>().map(|h| h.0)
        && crate::window::is_open(handle, cx)
    {
        crate::window::raise(handle, cx);
        activation::window_opened(cx);
        return;
    }
    let bounds = Bounds::centered(None, size(px(620.), px(640.)), cx);
    let options = window_options(bounds);
    let env = env.clone();
    let opened = cx.open_window(options, |_, cx| {
        cx.new(|cx| {
            cx.observe(&env.state, |_, _, cx| cx.notify()).detach();
            OnboardingView { env }
        })
    });
    if let Ok(handle) = opened {
        cx.set_global(OnboardingHandle(handle));
        activation::window_opened(cx);
    }
}

/// Once the session has tried to connect, opens onboarding on first launch or when there is
/// neither a LaunchAgent nor an answering yardd.
pub fn open_if_needed(env: &AppEnv, cx: &mut App) {
    let env = env.clone();
    let first_launch = !env.settings.did_onboard();
    let state = env.state.clone();
    let decided = std::rc::Rc::new(std::cell::Cell::new(false));
    cx.observe(&state, move |state, cx| {
        if decided.get() || matches!(state.read(cx).connection, ConnectionState::Connecting) {
            return;
        }
        decided.set(true);
        if needs_onboarding(&facts(&env, cx), first_launch) {
            open(&env, cx);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::env;
    use gpui::{Context, IntoElement, Render, TestAppContext, Window};
    use yard_app_model::setup::SetupFacts;
    use yard_proto::Profile;

    /// Set up Yard… raises the open checklist rather than opening another.
    #[gpui::test]
    async fn onboarding_opens_once(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        cx.update(|cx| super::open(&env, cx));
        cx.update(|cx| super::open(&env, cx));
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
    }

    fn fresh() -> SetupFacts {
        SetupFacts {
            brew: true,
            yard: None,
            agent_installed: false,
            daemon_answers: false,
            helper_version: None,
            ca_trusted: None,
            profile: None,
            site_count: None,
        }
    }

    /// A yard chosen in onboarding or Settings is run at once (it applied only at the next launch).
    #[gpui::test]
    async fn a_chosen_yard_is_used_at_once(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        assert_eq!(cx.update(|cx| facts(&env, cx).yard), None);
        cx.update(|cx| env.use_yard_setting(Some("/opt/yard/yard".into()), cx));
        assert_eq!(cx.update(|cx| facts(&env, cx).yard), Some("/opt/yard/yard".into()));
        assert_eq!(env.cli.yard(), Some("/opt/yard/yard".into()), "and the CLI runs it");
    }

    fn stopped() -> SetupFacts {
        SetupFacts { yard: Some("/r/yard".into()), agent_installed: true, ..fresh() }
    }

    fn set_up() -> SetupFacts {
        SetupFacts {
            brew: true,
            yard: Some("/r/yard".into()),
            agent_installed: true,
            daemon_answers: true,
            helper_version: Some("0.1.0".into()),
            ca_trusted: Some(true),
            profile: Some(Profile::Standard),
            site_count: Some(3),
        }
    }

    struct Host(crate::env::AppEnv, SetupFacts);

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            checklist(&self.0, &self.1, window, cx)
        }
    }

    #[test]
    fn the_checklist_reads_from_facts() {
        let lines = step_lines(&fresh());
        assert_eq!(lines[0], "Homebrew — done");
        assert_eq!(lines[1], "The yard command — to do");
        assert_eq!(lines[2], "yardd — waiting on an earlier step");
        assert_eq!(step_lines(&stopped())[2], "yardd — to do");
        let done = step_lines(&set_up());
        assert!(done[..4].iter().all(|l| l.ends_with("— done")), "{done:?}");
        assert_eq!(done[5], "Shims on PATH (optional) — to do", "shims are never checked");
    }

    #[gpui::test]
    async fn the_window_renders_for_three_macs(cx: &mut TestAppContext) {
        for facts in [fresh(), stopped(), set_up()] {
            let (_fake, env) = env(cx);
            let (_v, cx) = cx.add_window_view(|_, _| Host(env, facts));
            cx.run_until_parked();
        }
    }

    /// A step that fails says why in the onboarding window, where it was pressed (found by the
    /// checklist script: Start yardd's refusal showed only on the main window's Overview).
    #[gpui::test]
    async fn a_failed_step_says_why_in_onboarding(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        env.runner.update(cx, |r, _| {
            let e = yard_proto::YardError::new(
                yard_proto::ErrorCode::InvalidArgument,
                "the agent serves another root",
            );
            r.errors.record(crate::runner::ActionId("yardd:start".into()), e)
        });
        assert_eq!(cx.update(|cx| super::step_errors(&env, cx).len()), 1);
    }

    /// The helper step pending, with the room a 620×640 window gives.
    fn helper_pending() -> SetupFacts {
        SetupFacts { daemon_answers: true, ..stopped() }
    }

    fn onboarding_window(cx: &mut TestAppContext) -> &mut gpui::VisualTestContext {
        let (_fake, env) = env(cx);
        let (_v, vcx) = cx.add_window_view(|_, _| Host(env, helper_pending()));
        vcx.simulate_resize(gpui::size(gpui::px(620.), gpui::px(320.)));
        vcx.run_until_parked();
        vcx
    }

    /// The window scrolls, so Check again can be reached however short it is (G8: it was cut off).
    #[gpui::test]
    async fn onboarding_scrolls_to_check_again(cx: &mut TestAppContext) {
        let vcx = onboarding_window(cx);
        let height = vcx.update(|w, _| w.viewport_size().height);
        let before = vcx.debug_bounds("setup:check").expect("Check again is laid out");
        assert!(before.bottom() > height, "the test needs Check again below the fold");
        vcx.simulate_event(gpui::ScrollEvent {
            position: gpui::point(gpui::px(300.), gpui::px(100.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
            ..Default::default()
        });
        vcx.run_until_parked();
        let after = vcx.debug_bounds("setup:check").expect("Check again is laid out");
        assert!(
            after.bottom() <= height,
            "Check again ends at {:?}, the window at {height:?}",
            after.bottom()
        );
    }

    /// The helper's note sits under its step's title, not in a third column (G8: it ran out of its card).
    #[gpui::test]
    async fn the_helper_note_sits_under_its_title(cx: &mut TestAppContext) {
        let vcx = onboarding_window(cx);
        let title = vcx.debug_bounds("step:Helper:title").expect("the helper's title");
        let note = vcx.debug_bounds("helper:note").expect("the helper's note");
        assert_eq!(note.left(), title.left());
        assert!(note.top() >= title.bottom());
    }
}
