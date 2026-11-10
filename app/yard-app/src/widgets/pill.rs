//! A status pill: a dot and a word on its state's wash (spec §3.5). The word goes through
//! `widgets::text`, so VoiceOver reads it as the cell's text, as before pills.

use super::text;
use super::theme::{Theme, Tone, Typography};
use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px};

/// The tone a state word carries. Prefixes count, so `waiting for Install PHP 7.1` is a warning
/// and `failed: pecl failed` a danger; a word the app does not know is neutral.
pub fn tone_for(word: &str) -> Tone {
    const SUCCESS: [&str; 8] = ["running", "serving", "listening", "ready", "trusted", "done", "ok", "yard"];
    const WARNING: [&str; 6] = ["needs setup", "starting", "waiting", "deprecated", "warn", "stopping"];
    const DANGER: [&str; 4] = ["failed", "error", "in use", "lost"];
    let w = word.trim().to_lowercase();
    let starts = |list: &[&str]| {
        list.iter().any(|p| w == *p || w.starts_with(&format!("{p} ")) || w.starts_with(&format!("{p}:")))
    };
    if starts(&DANGER) {
        Tone::Danger
    } else if starts(&WARNING) {
        Tone::Warning
    } else if starts(&SUCCESS) {
        Tone::Success
    } else if w == "default" {
        Tone::Accent
    } else {
        Tone::Neutral
    }
}

#[derive(IntoElement)]
pub struct Pill {
    id: SharedString,
    word: SharedString,
    tone: Tone,
}

impl Pill {
    pub fn new(id: impl Into<SharedString>, word: impl Into<SharedString>, tone: Tone) -> Pill {
        Pill { id: id.into(), word: word.into(), tone }
    }

    /// A pill whose tone comes from its word.
    pub fn of(id: impl Into<SharedString>, word: impl Into<SharedString>) -> Pill {
        let word = word.into();
        let tone = tone_for(&word);
        Pill::new(id, word, tone)
    }
}

impl RenderOnce for Pill {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (dot, fg, wash) = Theme::of(window).state(self.tone);
        let selector = format!("pill:{}", self.id);
        div()
            .debug_selector(move || selector)
            // It shrinks, and its word truncates, before it pushes past its container.
            .flex_shrink(1.)
            .min_w(px(0.))
            .max_w_full()
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(18.))
            .px(px(7.))
            .rounded_full()
            .bg(wash)
            .text_color(fg)
            .type_caption()
            .overflow_hidden()
            .child(div().size(px(6.)).flex_none().rounded_full().bg(dot))
            .child(div().min_w(px(0.)).truncate().child(text(self.id, self.word)))
    }
}
