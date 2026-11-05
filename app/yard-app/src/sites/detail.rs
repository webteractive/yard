//! A site's detail (M6 design §8.2 Sites): each setting with where it comes from, and its controls.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::platform::open::Opener;
use crate::runner::ActionId;
use crate::services::table::copy_env;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::picker::Picker;
use crate::widgets::theme::Theme;
use crate::widgets::toggle::Toggle;
use gpui::{AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, px};
use std::path::{Path, PathBuf};
use yard_app_model::hint::HintAction;
use yard_app_model::settle::SettlePlan;
use yard_client::api;
use yard_proto::methods::{DocrootParams, EnvParams, InitParams, IsolateParams, NameParams, SiteParams};
use yard_proto::{SettingSource, SiteDetail, SiteKind, SiteSource};

fn from(source: Option<&SettingSource>) -> &'static str {
    match source {
        Some(SettingSource::ProjectFile) => "from the project's .yard",
        Some(SettingSource::ManagedFile) => "from the managed .yard",
        Some(SettingSource::SitesToml) => "from sites.toml",
        Some(SettingSource::YardToml) => "from yard.toml",
        Some(SettingSource::Default) | None => "the default",
        Some(_) => "set elsewhere",
    }
}

/// `PHP 8.4.3 — from the managed .yard`, and the same for the docroot and HTTPS.
pub fn source_lines(d: &SiteDetail) -> Vec<String> {
    let mut lines = vec![];
    if let SiteKind::Php { php, docroot, .. } = &d.site.kind {
        lines.push(format!(
            "PHP {} — {}",
            d.php_full.clone().unwrap_or_else(|| php.clone()),
            from(d.sources.get("php"))
        ));
        lines.push(format!("Docroot {docroot} — {}", from(d.sources.get("docroot"))));
    }
    lines.push(format!(
        "HTTPS {} — {}",
        if d.site.secure { "on" } else { "off" },
        from(d.sources.get("secure"))
    ));
    lines
}

/// Why the PHP picker is disabled: a committed `.yard` owns the version, and only an edit of that
/// file changes it. A managed `.yard` is edited for you, so it does not lock.
pub fn php_lock(d: &SiteDetail) -> Option<String> {
    (d.sources.get("php") == Some(&SettingSource::ProjectFile))
        .then(|| format!("set in {}", d.site.project_file.clone().unwrap_or_else(|| ".yard".into())))
}

fn quick(
    env: &AppEnv,
    id: String,
    work: impl std::future::Future<Output = Result<(), yard_proto::YardError>> + 'static,
    cx: &mut App,
) {
    env.runner.update(cx, |r, cx| r.perform(ActionId(id), work, cx).detach());
}

pub fn detail(env: &AppEnv, name: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let s = env.state.read(cx);
    let site = s.sites.value.iter().flatten().find(|x| x.name == name).cloned();
    let Some(site) = site else { return div().child(format!("{name} is not linked")).into_any_element() };
    let detail = s.site_detail.get(name).and_then(|r| r.value.clone());
    let php_options: Vec<String> =
        s.php.value.iter().flatten().filter(|p| p.installed).map(|p| p.version.clone()).collect();
    let mut pane = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border)
        .bg(t.surface);
    pane = pane
        .child(div().text_lg().child(site.name.clone()))
        .child(div().text_sm().text_color(t.accent).child(site.url.clone()));
    if let Some(d) = &detail {
        pane = pane.children(source_lines(d).into_iter().map(|l| div().text_sm().child(l)));
        if let SiteKind::Php { php, .. } = &site.kind {
            let mut options: Vec<SharedString> = vec!["Default".into()];
            options.extend(php_options.iter().map(|v| SharedString::from(v.clone())));
            let selected = php_options.iter().position(|v| v == php).map(|i| i + 1);
            let (e, n, versions) = (env.clone(), name.to_string(), php_options.clone());
            pane = pane.child(
                Picker::new(format!("site:{name}:php"), format!("PHP for {name}"), options, selected)
                    .disabled_because(php_lock(d).map(Into::into))
                    .on_select(move |i, _, cx| {
                        let php = if i == 0 { None } else { versions.get(i - 1).cloned() };
                        let (daemon, site) = (e.daemon.clone(), n.clone());
                        quick(
                            &e,
                            format!("site:{n}:php"),
                            async move {
                                call(&*daemon, api::SITES_ISOLATE, IsolateParams { site, php })
                                    .await
                                    .map(|_| ())
                            },
                            cx,
                        )
                    }),
            );
            if let (Some(_), Some(file)) = (php_lock(d), site.project_file.clone()) {
                let e = env.clone();
                pane = pane.child(
                    Button::new(format!("site:{name}:open-yard"), "Open .yard")
                        .on_click(move |_, _, cx| e.perform(HintAction::OpenFile(PathBuf::from(&file)), cx)),
                );
            }
        }
        for svc in &d.services {
            let met = if svc.service.is_some() { "met" } else { "missing" };
            pane = pane.child(div().text_sm().child(format!(
                "{} {} — {met}",
                yard_proto::engine_display(&svc.engine),
                svc.want.clone().unwrap_or_default()
            )));
        }
    }
    let (e, n, secure) = (env.clone(), name.to_string(), site.secure);
    pane = pane.child(
        Toggle::new(format!("site:{name}:secure"), format!("HTTPS for {name}"), secure).on_toggle(
            move |on, _, cx| {
                let (daemon, site) = (e.daemon.clone(), n.clone());
                let m = if on { api::SITES_SECURE } else { api::SITES_UNSECURE };
                quick(
                    &e,
                    format!("site:{n}:secure"),
                    async move { call(&*daemon, m, SiteParams { site }).await.map(|_| ()) },
                    cx,
                )
            },
        ),
    );
    if SettlePlan::new(&site).is_some() {
        let (e, n) = (env.clone(), name.to_string());
        pane = pane.child(
            Button::new(format!("site:{name}:settle"), "Install what it needs…")
                .style(ButtonStyle::Primary)
                .on_click(move |_, _, cx| {
                    e.nav.update(cx, |nav, cx| {
                        nav.sheet = Some(Sheet::Settle(n.clone()));
                        cx.notify();
                    })
                }),
        );
    }
    if let Some(e) = &site.project_error {
        pane = pane.child(div().text_sm().text_color(t.danger).child(e.message.clone()));
    }
    let mut actions = div().flex().flex_wrap().gap(px(6.));
    let url = site.url.clone();
    actions = actions.child(
        Button::new(format!("site:{name}:browser"), "Open in browser")
            .on_click(move |_, _, cx| cx.open_url(&url)),
    );
    if let SiteKind::Php { path, .. } = &site.kind {
        let path = PathBuf::from(path);
        let (e1, p1) = (env.clone(), path.clone());
        actions = actions.child(Button::new(format!("site:{name}:editor"), "Open in editor").on_click(
            move |_, _, _| {
                let _ = Opener::open_with(&p1, e1.settings.editor_app().as_deref());
            },
        ));
        let p2 = path.clone();
        actions = actions.child(
            Button::new(format!("site:{name}:reveal"), "Reveal in Finder")
                .on_click(move |_, _, cx| Opener::reveal(cx, &p2)),
        );
        let (e3, p3) = (env.clone(), path.clone());
        actions = actions.child(Button::new(format!("site:{name}:terminal"), "Open in Terminal").on_click(
            move |_, _, _| {
                let app = e3
                    .settings
                    .terminal_app()
                    .unwrap_or_else(|| PathBuf::from("/System/Applications/Utilities/Terminal.app"));
                let _ = Opener::open_with(&p3, Some(&app));
            },
        ));
        let (e4, n4) = (env.clone(), name.to_string());
        actions = actions.child(Button::new(format!("site:{name}:detect"), "Detect docroot").on_click(
            move |_, _, cx| {
                let (daemon, site) = (e4.daemon.clone(), n4.clone());
                quick(
                    &e4,
                    format!("site:{n4}:docroot"),
                    async move {
                        call(&*daemon, api::SITES_DOCROOT, DocrootParams { site, path: None })
                            .await
                            .map(|_| ())
                    },
                    cx,
                )
            },
        ));
        let (e5, n5, p5) = (env.clone(), name.to_string(), path.clone());
        actions = actions.child(Button::new(format!("site:{name}:init"), "Save settings to .yard").on_click(
            move |_, _, cx| {
                let (daemon, site, path) = (e5.daemon.clone(), n5.clone(), p5.display().to_string());
                quick(
                    &e5,
                    format!("site:{n5}:init"),
                    async move {
                        call(
                            &*daemon,
                            api::SITES_INIT,
                            InitParams { path, name: Some(site), force: false, replace: false, local: false },
                        )
                        .await
                        .map(|_| ())
                    },
                    cx,
                )
            },
        ));
    }
    let (e6, root, n6) = (env.clone(), env.location.root.clone(), name.to_string());
    actions = actions.child(Button::new(format!("site:{name}:override"), "Edit nginx override").on_click(
        move |_, _, cx| {
            e6.perform(HintAction::OpenFile(root.join("nginx/overrides").join(format!("{n6}.conf"))), cx)
        },
    ));
    let (e7, n7) = (env.clone(), name.to_string());
    actions =
        actions.child(Button::new(format!("site:{name}:env"), "Copy .env").on_click(move |_, _, cx| {
            copy_env(
                &e7,
                EnvParams { path: None, site: Some(n7.clone()), service: None },
                format!("site:{n7}:env"),
                cx,
            )
        }));
    let proxy = site.source == SiteSource::Proxy;
    let (e8, n8) = (env.clone(), name.to_string());
    actions = actions.child(
        Button::new(format!("site:{name}:unlink"), if proxy { "Remove proxy…" } else { "Unlink…" })
            .style(ButtonStyle::Destructive)
            .on_click(move |_, window, cx| {
                let question =
                    if proxy { format!("Remove the proxy {n8}?") } else { format!("Unlink {n8}?") };
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &question,
                    Some("Its project stays where it is."),
                    &["Remove", "Cancel"],
                    cx,
                );
                let (env, name) = (e8.clone(), n8.clone());
                cx.spawn(async move |cx| {
                    if answer.await == Ok(0) {
                        cx.update(|cx| {
                            let daemon = env.daemon.clone();
                            let m = if proxy { api::SITES_UNPROXY } else { api::SITES_UNLINK };
                            let n = name.clone();
                            quick(
                                &env,
                                format!("site:{name}:unlink"),
                                async move { call(&*daemon, m, NameParams { name: n }).await.map(|_| ()) },
                                cx,
                            )
                        });
                    }
                })
                .detach();
            }),
    );
    let _ = Path::new("");
    pane.child(actions).into_any_element()
}
