//! Activity (M6 design §9.3): every job this session saw, newest first, with its output and a
//! Cancel for one that runs.

use crate::env::AppEnv;
use crate::widgets::button::Button;
use crate::widgets::log_view::LogView;
use crate::widgets::theme::Theme;
use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::*, px};
use yard_app_model::jobs::{JobPhase, JobRecord};

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
            .border_color(t.border)
            .bg(t.surface)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_sm().child(j.title.clone()))
                    .child(div().text_xs().text_color(t.muted).child(phase_text(&j.phase))),
            )
            .when(!tail.is_empty(), |d| d.child(LogView::new(format!("{id}:log"), tail)))
            .when(running, |d| {
                d.child(Button::new(format!("{id}:cancel"), format!("Cancel {}", j.title)).on_click(
                    move |_, _, cx| {
                        let job = job_id.clone();
                        env.runner.update(cx, |r, cx| r.cancel_job(job, cx).detach());
                    },
                ))
            })
    });
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(div().text_xl().child("Activity"))
        .children(cards)
        .into_any_element()
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
}
