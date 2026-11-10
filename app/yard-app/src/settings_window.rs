//! The Settings window (M6 design §8.3): the yard to run, the editor and terminal apps. They live in
//! the app's own defaults; the data root comes from `--root` or the LaunchAgent and is shown here.

use crate::env::AppEnv;
use crate::platform::activation;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::checkbox::Checkbox;
use crate::widgets::text_field::TextField;
use crate::widgets::theme::Theme;
use gpui::{
    App, AppContext, Bounds, Context, Entity, Global, IntoElement, ParentElement, Pixels, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
};
use std::path::Path;

pub struct SettingsView {
    env: AppEnv,
    yard: Entity<TextField>,
    editor: Entity<TextField>,
    terminal: Entity<TextField>,
    saved: bool,
}

fn shown(p: Option<std::path::PathBuf>) -> String {
    p.map(|p| p.display().to_string()).unwrap_or_default()
}

impl SettingsView {
    pub fn new(env: AppEnv, cx: &mut Context<Self>) -> SettingsView {
        let s = env.settings.clone();
        let field = |id: &str, label: &str, placeholder: &str, value: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut f = TextField::new(id.to_string(), label.to_string(), placeholder.to_string(), cx);
                f.set_value(value, cx);
                f.full_width();
                f
            })
        };
        let yard = field(
            "settings:yard",
            "Path of yard",
            "found beside the LaunchAgent's yardd",
            shown(s.yard_path()),
            cx,
        );
        let editor = field(
            "settings:editor",
            "Editor app",
            "/Applications/Visual Studio Code.app",
            shown(s.editor_app()),
            cx,
        );
        let terminal = field(
            "settings:terminal",
            "Terminal app",
            "/System/Applications/Utilities/Terminal.app",
            shown(s.terminal_app()),
            cx,
        );
        SettingsView { env, yard, editor, terminal, saved: false }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let value = |f: &Entity<TextField>, cx: &App| {
            let v = f.read(cx).value().trim().to_string();
            (!v.is_empty()).then_some(v)
        };
        let s = self.env.settings.clone();
        self.env.set_yard_path(value(&self.yard, cx).as_deref().map(Path::new), cx);
        s.set_editor_app(value(&self.editor, cx).as_deref().map(Path::new));
        s.set_terminal_app(value(&self.terminal, cx).as_deref().map(Path::new));
        self.saved = true;
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(window);
        let row = |label: &str, field: Entity<TextField>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_xs().text_color(t.muted).child(label.to_string()))
                .child(field)
        };
        div()
            .size_full()
            .p(px(16.))
            .gap(px(12.))
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(div().text_lg().child(crate::widgets::text("heading", "Settings")))
            .child(row(
                "yard (a path, or empty to find it beside the LaunchAgent's yardd)",
                self.yard.clone(),
            ))
            .child(row("Editor app", self.editor.clone()))
            .child(row("Terminal app", self.terminal.clone()))
            .child({
                let settings = self.env.settings.clone();
                Checkbox::new("settings:auto-update", "Install updates automatically", settings.auto_update())
                    .on_toggle(move |on, window, _| {
                        settings.set_auto_update(on);
                        window.refresh();
                    })
            })
            .child(div().text_xs().text_color(t.muted).child(crate::widgets::text(
                "data-root",
                format!("Data root: {}", self.env.location.root.display()),
            )))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        Button::new("settings:save", "Save")
                            .style(ButtonStyle::Primary)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        div().text_xs().text_color(t.muted).child(crate::widgets::text(
                            "saved",
                            saved_text(self.saved, self.env.cli.yard()),
                        )),
                    ),
            )
            .child({
                let env = self.env.clone();
                Button::new("settings:uninstall", "Uninstall Yard…")
                    .style(ButtonStyle::Destructive)
                    .description("opens Terminal to run yard uninstall, which asks before removing anything")
                    .on_click(move |_, _, cx| env.uninstall(cx))
            })
    }
}

/// What Save says: the `yard` now in use, or that none was found.
fn saved_text(saved: bool, yard: Option<std::path::PathBuf>) -> String {
    match (saved, yard) {
        (false, _) => String::new(),
        (true, Some(yard)) => format!("Saved. Yard.app now runs {}.", yard.display()),
        (true, None) => {
            "Saved, but no executable yard was found there or beside the LaunchAgent's yardd.".into()
        }
    }
}

struct SettingsHandle(WindowHandle<SettingsView>);

impl Global for SettingsHandle {}

/// Opens the Settings window, or raises it when it is open.
pub fn open(env: &AppEnv, cx: &mut App) {
    if let Some(handle) = cx.try_global::<SettingsHandle>().map(|h| h.0)
        && crate::window::is_open(handle, cx)
    {
        crate::window::raise(handle, cx);
        activation::window_opened(cx);
        return;
    }
    let bounds = Bounds::centered(None, size(px(560.), px(360.)), cx);
    let options = window_options(bounds);
    let env = env.clone();
    if let Ok(handle) = cx.open_window(options, |_, cx| cx.new(|cx| SettingsView::new(env, cx))) {
        cx.set_global(SettingsHandle(handle));
        activation::window_opened(cx);
    }
}

/// The window's options: its title (for Mission Control and the Window menu) and its size.
pub(crate) fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Yard Settings".into()), ..Default::default() }),
        ..Default::default()
    }
}
#[cfg(test)]
mod tests {
    use crate::testing::env;
    use gpui::TestAppContext;

    /// Settings… from the menu raises the open Settings window rather than opening another
    /// (found by the checklist script: each click opened one more).
    #[gpui::test]
    async fn settings_opens_once(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        cx.update(|cx| super::open(&env, cx));
        cx.update(|cx| super::open(&env, cx));
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
    }

    #[gpui::test]
    async fn settings_renders(cx: &mut TestAppContext) {
        let (_fake, env) = env(cx);
        let (_v, cx) = cx.add_window_view(|_, cx| super::SettingsView::new(env, cx));
        cx.run_until_parked();
    }
}
