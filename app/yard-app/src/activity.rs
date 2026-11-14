//! Activity (M6 design §9.3): every job this session saw, newest first, with its output and a
//! Cancel for one that runs.

use crate::env::AppEnv;
use crate::widgets::button::Button;
use crate::widgets::error_view::ErrorView;
use crate::widgets::log_view::LogView;
use crate::widgets::pill::Pill;
use crate::widgets::theme::Theme;
use crate::widgets::theme::Typography;
use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::*, px};
use yard_app_model::hint::HintAction;
use yard_app_model::jobs::{JobPhase, JobRecord};

/// The phase's word for its pill: why a job failed is in the error under it.
pub fn phase_word(phase: &JobPhase) -> String {
    match phase {
        JobPhase::Failed(_) => "failed".into(),
        other => phase_text(other),
    }
}

pub fn phase_text(phase: &JobPhase) -> String {
    match phase {
        JobPhase::Waiting { behind } => format!("waiting for {behind}"),
        JobPhase::Running => "running".into(),
        JobPhase::Succeeded(_) => "done".into(),
        JobPhase::Failed(e) => format!("failed: {}", e.message),
        JobPhase::Cancelled => "cancelled".into(),
        JobPhase::Lost => "lost: yardd's connection was lost while this ran; its result is unknown".into(),
    }
}

/// What a failed job's tool printed last: `data.tail` (a list of lines) or `data.output` (text).
pub fn failure_log(e: &yard_proto::YardError) -> Vec<String> {
    let Some(data) = &e.data else { return vec![] };
    match (data.get("tail"), data.get("output")) {
        (Some(serde_json::Value::Array(lines)), _) => {
            lines.iter().filter_map(|l| l.as_str().map(str::to_string)).collect()
        }
        (_, Some(serde_json::Value::String(text))) => text.lines().map(str::to_string).collect(),
        (_, Some(serde_json::Value::Array(lines))) => {
            lines.iter().filter_map(|l| l.as_str().map(str::to_string)).collect()
        }
        _ => vec![],
    }
}

fn newest_first(env: &AppEnv, cx: &App) -> Vec<JobRecord> {
    env.state.read(cx).jobs.jobs().iter().rev().cloned().collect()
}

/// `<title> — <phase>` per job, newest first, as the tests read the screen.
#[cfg(test)]
pub fn rows(env: &AppEnv, cx: &App) -> Vec<String> {
    newest_first(env, cx).iter().map(|j| format!("{} — {}", j.title, phase_text(&j.phase))).collect()
}

pub fn activity(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let jobs = newest_first(env, cx);
    let cards = jobs.into_iter().map(|j| {
        let running = !j.phase.is_finished();
        let failure = match &j.phase {
            JobPhase::Failed(e) => Some(e.clone()),
            _ => None,
        };
        let id = format!("job:{}:{}", j.key.epoch, j.key.id);
        let env = env.clone();
        let job_id = j.key.id.clone();
        let tail: Vec<SharedString> =
            j.lines.iter().rev().take(20).rev().map(|l| SharedString::from(l.clone())).collect();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.c.separator)
            .bg(t.c.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child({
                        let selector = format!("{id}:title");
                        div()
                            .debug_selector(move || selector)
                            .min_w(px(0.))
                            .type_body_strong()
                            .child(crate::widgets::text(format!("{id}:title"), j.title.clone()))
                    })
                    .child(Pill::of(format!("{id}:phase"), phase_word(&j.phase))),
            )
            .when(!tail.is_empty(), |d| d.child(LogView::new(format!("{id}:log"), tail)))
            .when_some(failure.clone(), |d, e| {
                let env = env.clone();
                let log: Vec<SharedString> = failure_log(&e).into_iter().map(SharedString::from).collect();
                d.child(
                    ErrorView::new(
                        format!("{id}:error"),
                        &e,
                        HintAction::parse(e.hint.as_deref().unwrap_or("")),
                    )
                    .on_action(move |a, _, cx| env.perform(a, cx)),
                )
                .when(!log.is_empty(), |d| d.child(LogView::new(format!("{id}:failure"), log)))
            })
            .when(running, |d| {
                d.child(crate::widgets::bar().child(
                    Button::new(format!("{id}:cancel"), format!("Cancel {}", j.title)).on_click(
                        move |_, _, cx| {
                            let job = job_id.clone();
                            env.runner.update(cx, |r, cx| r.cancel_job(job, cx).detach());
                        },
                    ),
                ))
            })
    });
    let cards: Vec<_> = cards.collect();
    if cards.is_empty() {
        return crate::widgets::empty_state(
            "activity:empty",
            "Nothing running. Installs and other long tasks show their progress here.",
            &t,
        )
        .into_any_element();
    }
    div().flex().flex_col().gap(px(10.)).children(cards).into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::testing::{env, finished};
    use gpui::{Context, IntoElement, Render, TestAppContext, Window};
    use yard_proto::{ErrorCode, Event, YardError};

    struct Host(crate::env::AppEnv);
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            super::activity(&self.0, window, cx)
        }
    }

    /// A failed job shows what brew, pecl or cloudflared printed last: `data.tail` (`JOB_FAILED`)
    /// or `data.output` (`EXT_VERIFY_FAILED`, `TUNNEL_DNS_FAILED`) (design §9.3; M6 final review).
    #[test]
    fn a_failure_shows_its_tail_or_output() {
        let tail = YardError::new(ErrorCode::JobFailed, "brew exited with 1")
            .with_data(serde_json::json!({"tail": ["==> Pouring", "Error: no bottle"]}));
        assert_eq!(super::failure_log(&tail), ["==> Pouring", "Error: no bottle"]);
        let output = YardError::new(ErrorCode::ExtVerifyFailed, "redis did not load in PHP 8.4")
            .with_data(serde_json::json!({"output": "Warning: cannot load\nPHP Startup"}));
        assert_eq!(super::failure_log(&output), ["Warning: cannot load", "PHP Startup"]);
        assert!(super::failure_log(&YardError::internal("x")).is_empty());
    }

    /// With no jobs, Activity says what it will show rather than staying blank.
    #[gpui::test]
    async fn an_empty_activity_says_what_it_shows(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let (_v, cx) = cx.add_window_view(|_, _| Host(env));
        cx.run_until_parked();
        assert!(cx.debug_bounds("activity:empty").is_some(), "an empty state");
    }

    #[gpui::test]
    async fn activity_renders_running_and_failed_jobs(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        fake.event(Event::JobProgress { job_id: "php-install-1".into(), line: "==> Downloading".into() });
        fake.event(finished("ext-add-2", Some(YardError::new(ErrorCode::JobFailed, "pecl failed"))));
        cx.run_until_parked();
        let titles = cx.update(|cx| super::rows(&env, cx));
        assert_eq!(
            titles,
            [
                "Add an extension (started elsewhere) — failed: pecl failed",
                "Install PHP (started elsewhere) — running"
            ]
        );
        let (_v, cx) = cx.add_window_view(|_, _| Host(env));
        cx.run_until_parked();
    }

    /// A job's status pill is centred on its title (G17: "done" sat higher than its title).
    #[gpui::test]
    async fn a_job_status_is_centred_on_its_title(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        fake.event(Event::JobProgress { job_id: "php-install-1".into(), line: "==> Downloading".into() });
        cx.run_until_parked();
        let (_v, vcx) = cx.add_window_view(|_, _| Host(env));
        vcx.run_until_parked();
        let title = vcx.debug_bounds("job:1:php-install-1:title").expect("the title");
        let pill = vcx.debug_bounds("pill:job:1:php-install-1:phase").expect("the status pill");
        let (a, b) = (title.center().y, pill.center().y);
        assert!((a - b).abs() <= gpui::px(1.), "title centre {a:?}, pill centre {b:?}");
    }

    /// A long failure keeps the card's status short: the pill says "failed" and the message is in
    /// the error under it (final review: a 200-character message ran the pill off the card).
    #[gpui::test]
    async fn a_long_failure_keeps_the_status_inside_its_card(cx: &mut TestAppContext) {
        let (fake, env) = env(cx);
        fake.event(finished("ext-add-2", Some(YardError::new(ErrorCode::JobFailed, "x ".repeat(100)))));
        cx.run_until_parked();
        let (_v, vcx) = cx.add_window_view(|_, _| Host(env));
        vcx.simulate_resize(gpui::size(gpui::px(600.), gpui::px(400.)));
        vcx.run_until_parked();
        let pill = vcx.debug_bounds("pill:job:1:ext-add-2:phase").expect("the status pill");
        assert!(pill.right() <= gpui::px(600.), "the pill ends at {:?}", pill.right());
        assert!(pill.size.width < gpui::px(120.), "the pill is {:?} wide", pill.size.width);
    }
}
