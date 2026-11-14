//! A site's detail (M6 design §8.2 Sites): each setting with where it comes from, and its controls.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::platform::open::Opener;
use crate::runner::ActionId;
use crate::services::table::copy_env;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::group::Group;
use crate::widgets::menu_button::MenuButton;
use crate::widgets::picker::Picker;
use crate::widgets::pill::Pill;
use crate::widgets::text;
use crate::widgets::theme::Typography;
use crate::widgets::theme::{Theme, Tone};
use crate::widgets::toggle::Toggle;
use gpui::{AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, px};
use std::path::PathBuf;
use yard_app_model::hint::HintAction;
use yard_app_model::settle::SettlePlan;
use yard_client::api;
use yard_proto::methods::{DocrootParams, EnvParams, InitParams, IsolateParams, NameParams, SiteParams};
use yard_proto::{ProjectFileKind, SettingSource, SiteDetail, SiteInfo, SiteKind, SiteSource};

/// Unlink's question, and what it leaves behind: a site's project, or what a proxy forwards to.
pub fn unlink_question(name: &str, proxy: bool) -> (String, &'static str) {
    if proxy {
        (format!("Remove the proxy {name}?"), "What it forwards to keeps running.")
    } else {
        (format!("Unlink {name}?"), "Its project stays where it is.")
    }
}

/// Save settings to .yard (`sites.init`): asks whether Yard keeps the file in its data root (the
/// repository stays clean) or writes it into the project, and when another directory's site has
/// the name (`SITE_NAME_TAKEN`) whether to replace it (design §8.2, §11.3).
pub fn save_settings(env: &AppEnv, site: &str, path: &str, window: &mut Window, cx: &mut App) {
    let answer = window.prompt(
        PromptLevel::Info,
        &format!("Save {site}'s settings to a .yard?"),
        Some("Yard keeps it in its data root, so the repository stays clean. Written into the project, it travels with the code, and it then owns PHP, HTTPS and the docroot."),
        &["Save", "Write it into the project", "Cancel"],
        cx,
    );
    let (env, site, path) = (env.clone(), site.to_string(), path.to_string());
    window
        .spawn(cx, async move |cx| {
            let local = match answer.await {
                Ok(0) => false,
                Ok(1) => true,
                _ => return,
            };
            let id = ActionId(format!("site:{site}:init"));
            let init = |replace: bool, cx: &mut gpui::AsyncWindowContext| {
                let daemon = env.daemon.clone();
                let params =
                    InitParams { path: path.clone(), name: Some(site.clone()), force: false, replace, local };
                let id = id.clone();
                env.runner.update(cx, |r, cx| {
                    r.perform(
                        id,
                        async move { call(&*daemon, api::SITES_INIT, params).await.map(|_| ()) },
                        cx,
                    )
                })
            };
            if init(false, cx).await.is_some() {
                return;
            }
            let Some(taken) = env.runner.update(cx, |r, _| r.errors.get(&id).cloned()) else { return };
            if taken.code != yard_proto::ErrorCode::SiteNameTaken {
                return;
            }
            let Ok(answer) = cx.update(|window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    "Replace it with this directory?",
                    Some(&taken.message),
                    &["Replace", "Cancel"],
                    cx,
                )
            }) else {
                return;
            };
            if answer.await == Ok(0) {
                init(true, cx).await;
            }
        })
        .detach();
}

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

/// A setting the detail shows on its own row: the row's id, the value, and where it comes from
/// (`8.4.3`, `from the managed .yard`).
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: &'static str,
    pub value: String,
    pub from: String,
}

/// The site's PHP, docroot, HTTPS and Node, each with its source.
pub fn settings(d: &SiteDetail) -> Vec<Setting> {
    let setting = |key, value: String, from: &str| Setting { key, value, from: from.to_string() };
    let mut out = vec![];
    if let SiteKind::Php { php, docroot, .. } = &d.site.kind {
        out.push(setting(
            "php",
            d.php_full.clone().unwrap_or_else(|| php.clone()),
            from(d.sources.get("php")),
        ));
        out.push(setting("docroot", docroot.clone(), from(d.sources.get("docroot"))));
    }
    out.push(setting(
        "https",
        if d.site.secure { "on" } else { "off" }.into(),
        from(d.sources.get("secure")),
    ));
    if let Some(node) = &d.site.node {
        let source = match &node.file {
            Some(file) => format!("from {file}"),
            None => from(Some(&node.source)).to_string(),
        };
        let value = match (&node.error, &node.version, &node.spec) {
            (Some(e), _, _) => e.message.clone(),
            (None, Some(version), _) => version.clone(),
            (None, None, spec) => format!("{} — not installed", spec.as_deref().unwrap_or("?")),
        };
        out.push(setting("node", value, &source));
    }
    out
}

/// Whether the site's Node is a pin Yard made (`yard node use`, kept in sites.toml or the managed
/// `.yard`), which Use default Node removes; a `.nvmrc` or a committed `.yard` is the project's.
pub fn node_pinned_by_yard(site: &SiteInfo) -> bool {
    site.node
        .as_ref()
        .is_some_and(|n| matches!(n.source, SettingSource::SitesToml | SettingSource::ManagedFile))
}

/// Why the PHP picker, the HTTPS switch, Detect docroot and Save settings to .yard are disabled: the
/// project has its own `.yard`, which yardd never edits, so it refuses `isolate`, `secure` and
/// `docroot` for the site whatever keys the file sets, and `init` finds it already there. A
/// managed `.yard` is edited for you, so it does not lock.
pub fn project_lock(site: &SiteInfo) -> Option<String> {
    match (&site.project_file, site.project_file_kind) {
        (Some(file), Some(ProjectFileKind::Project)) => Some(format!("set in {file}")),
        _ => None,
    }
}

fn quick(
    env: &AppEnv,
    id: String,
    work: impl std::future::Future<Output = Result<(), yard_proto::YardError>> + 'static,
    cx: &mut App,
) {
    env.runner.update(cx, |r, cx| r.perform(ActionId(id), work, cx).detach());
}

/// Unlinks `name`, or removes it when it is a proxy; its error stays beside Unlink….
pub fn unlink(env: &AppEnv, name: String, proxy: bool, cx: &mut App) {
    let (daemon, n) = (env.daemon.clone(), name.clone());
    let m = if proxy { api::SITES_UNPROXY } else { api::SITES_UNLINK };
    let removal = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(format!("site:{name}:unlink")),
            async move { call(&*daemon, m, NameParams { name: n }).await.map(|_| ()) },
            cx,
        )
    });
    crate::window::main::forget_after(env, name, removal, cx);
}

pub fn detail(env: &AppEnv, name: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(window);
    let s = env.state.read(cx);
    let site = s.sites.value.iter().flatten().find(|x| x.name == name).cloned();
    let Some(site) = site else {
        return div()
            .child(text(format!("site:{name}:missing"), format!("{name} is not linked")))
            .into_any_element();
    };
    let detail = s.site_detail.get(name).and_then(|r| r.value.clone());
    let php_options: Vec<String> =
        s.php.value.iter().flatten().filter(|p| p.installed).map(|p| p.version.clone()).collect();
    let mut pane = div().flex().flex_col().gap(px(12.)).min_w(px(0.));
    pane =
        pane.child(div().type_section().child(text(format!("site:{name}:name"), site.name.clone()))).child(
            div()
                .type_body()
                .text_color(t.c.accent)
                .child(text(format!("site:{name}:url"), site.url.clone())),
        );
    // PHP, the docroot, HTTPS, Node and the services, one row each with where the setting comes
    // from under its control (spec §4).
    let settings = detail.as_ref().map(settings).unwrap_or_default();
    let from_of = |key: &str| settings.iter().find(|s| s.key == key).cloned();
    let caption = |key: &str, s: String| {
        div()
            .type_secondary()
            .text_color(t.c.text_secondary)
            .min_w(px(0.))
            .truncate()
            .child(text(format!("site:{name}:{key}:from"), s))
    };
    let lock = project_lock(&site);
    let mut group = Group::new(format!("site:{name}:settings"), None);
    if let SiteKind::Php { php, docroot, .. } = &site.kind {
        let mut options: Vec<SharedString> = vec!["Default".into()];
        options.extend(php_options.iter().map(|v| SharedString::from(v.clone())));
        let selected = php_options.iter().position(|v| v == php).map(|i| i + 1);
        let (e, n, versions) = (env.clone(), name.to_string(), php_options.clone());
        let open_yard = match (&lock, site.project_file.clone()) {
            (Some(_), Some(file)) => {
                let e = env.clone();
                Some(
                    Button::new(format!("site:{name}:open-yard"), "Open .yard")
                        .small()
                        .on_click(move |_, _, cx| e.perform(HintAction::OpenFile(PathBuf::from(&file)), cx)),
                )
            }
            _ => None,
        };
        let picker = Picker::new(format!("site:{name}:php"), format!("PHP for {name}"), options, selected)
            .disabled_because(lock.clone().map(Into::into))
            .on_select(move |i, _, cx| {
                let php = if i == 0 { None } else { versions.get(i - 1).cloned() };
                let (daemon, site) = (e.daemon.clone(), n.clone());
                quick(
                    &e,
                    format!("site:{n}:php"),
                    async move {
                        call(&*daemon, api::SITES_ISOLATE, IsolateParams { site, php }).await.map(|_| ())
                    },
                    cx,
                )
            });
        group = group.row(
            "php",
            "PHP",
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .gap(px(2.))
                .child(crate::widgets::bar().child(picker).children(open_yard))
                .children(from_of("php").map(|s| caption("php", format!("{} — {}", s.value, s.from)))),
        );
        let (e4, n4) = (env.clone(), name.to_string());
        let detect = Button::new(format!("site:{name}:detect"), "Detect docroot")
            .small()
            .disabled(lock.is_some())
            .on_click(move |_, _, cx| {
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
            });
        group = group.row(
            "docroot",
            "Docroot",
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w(px(0.))
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .child(text(format!("site:{name}:docroot-path"), docroot.clone())),
                        )
                        .child(div().flex_none().child(detect)),
                )
                .children(from_of("docroot").map(|s| caption("docroot", s.from))),
        );
    }
    let (e, n, secure) = (env.clone(), name.to_string(), site.secure);
    group = group.row(
        "https",
        "HTTPS",
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                Toggle::new(format!("site:{name}:secure"), format!("HTTPS for {name}"), secure)
                    .label_hidden()
                    .disabled_because(lock.clone().map(Into::into))
                    .on_toggle(move |on, _, cx| {
                        let (daemon, site) = (e.daemon.clone(), n.clone());
                        let m = if on { api::SITES_SECURE } else { api::SITES_UNSECURE };
                        quick(
                            &e,
                            format!("site:{n}:secure"),
                            async move { call(&*daemon, m, SiteParams { site }).await.map(|_| ()) },
                            cx,
                        )
                    }),
            )
            .children(from_of("https").map(|s| caption("https", s.from))),
    );
    if let Some(node) = from_of("node") {
        let unuse = node_pinned_by_yard(&site).then(|| {
            let (e, n) = (env.clone(), name.to_string());
            Button::new(format!("site:{name}:node-unuse"), "Use default Node").small().on_click(
                move |_, _, cx| {
                    let (daemon, site) = (e.daemon.clone(), n.clone());
                    quick(
                        &e,
                        format!("site:{n}:node"),
                        async move { call(&*daemon, api::NODE_UNUSE, SiteParams { site }).await.map(|_| ()) },
                        cx,
                    )
                },
            )
        });
        group = group.row(
            "node",
            "Node",
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    crate::widgets::bar()
                        .child(text(format!("site:{name}:node"), node.value.clone()))
                        .children(unuse),
                )
                .child(caption("node", node.from.clone())),
        );
    }
    if let Some(d) = detail.as_ref().filter(|d| !d.services.is_empty()) {
        let lines = d.services.iter().map(|svc| {
            let met = svc.service.is_some();
            let want = svc.want.as_deref().map(|w| format!(" {w}")).unwrap_or_default();
            crate::widgets::bar()
                .child(text(
                    format!("site:{name}:service:{}", svc.engine),
                    format!("{}{want}", yard_proto::engine_display(&svc.engine)),
                ))
                .child(Pill::new(
                    format!("site:{name}:service:{}:state", svc.engine),
                    if met { "met" } else { "missing" },
                    if met { Tone::Success } else { Tone::Warning },
                ))
        });
        group = group.row("services", "Services", div().flex().flex_col().gap(px(4.)).children(lines));
    }
    pane = pane.child(group);
    if SettlePlan::new(&site).is_some() {
        let (e, n) = (env.clone(), name.to_string());
        pane = pane.child(
            div().flex().child(
                Button::new(format!("site:{name}:settle"), "Install what it needs…")
                    .style(ButtonStyle::Primary)
                    .on_click(move |_, _, cx| {
                        e.nav.update(cx, |nav, cx| {
                            nav.sheet = Some(Sheet::Settle(n.clone()));
                            cx.notify();
                        })
                    }),
            ),
        );
    }
    if let Some(e) = &site.project_error {
        pane = pane.child(
            div()
                .type_body()
                .text_color(t.c.danger_text)
                .child(text(format!("site:{name}:error"), e.message.clone())),
        );
    }
    // Opening the site and its project, then its files, then Unlink… on its own (spec §4, Sites).
    let mut actions = crate::widgets::bar();
    let mut files = crate::widgets::bar();
    let url = site.url.clone();
    actions = actions.child(
        Button::new(format!("site:{name}:browser"), "Open in browser")
            .on_click(move |_, _, cx| cx.open_url(&url)),
    );
    if let SiteKind::Php { path, .. } = &site.kind {
        let path = PathBuf::from(path);
        // The editor, Finder and Terminal are one menu (spec §4, Sites), each item named for the
        // app it opens; the items keep the ids their buttons had.
        let app_name = |app: &Option<PathBuf>, fallback: &str| {
            app.as_deref()
                .and_then(|a| a.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| fallback.to_string())
        };
        let editor = env.settings.editor_app();
        let terminal = env.settings.terminal_app();
        let (e1, p1, p2, e3, p3) = (env.clone(), path.clone(), path.clone(), env.clone(), path.clone());
        actions = actions.child(
            MenuButton::new(format!("site:{name}:open"), "Open in…")
                .item(format!("site:{name}:editor"), app_name(&editor, "Editor"), move |_, cx| {
                    let id = ActionId(format!("{}folder", crate::window::main::OPEN_ERRORS));
                    let result = Opener::open_folder(cx, &p1, e1.settings.editor_app().as_deref());
                    record(&e1, id, result, cx);
                })
                .item(format!("site:{name}:reveal"), "Finder", move |_, cx| Opener::reveal(cx, &p2))
                .item(format!("site:{name}:terminal"), app_name(&terminal, "Terminal"), move |_, cx| {
                    let app = e3
                        .settings
                        .terminal_app()
                        .unwrap_or_else(|| PathBuf::from("/System/Applications/Utilities/Terminal.app"));
                    let id = ActionId(format!("{}terminal", crate::window::main::OPEN_ERRORS));
                    let result = Opener::open_with(&p3, Some(&app));
                    record(&e3, id, result, cx);
                }),
        );
        let (e5, n5, p5) = (env.clone(), name.to_string(), path.clone());
        files = files.child(
            Button::new(format!("site:{name}:init"), "Save settings to .yard")
                .disabled(lock.is_some())
                .on_click(move |_, window, cx| {
                    save_settings(&e5, &n5, &p5.display().to_string(), window, cx)
                }),
        );
    }
    let (e6, file) = (env.clone(), env.paths().nginx_override(name));
    files = files.child(
        Button::new(format!("site:{name}:override"), "Edit nginx override")
            .on_click(move |_, _, cx| e6.perform(HintAction::OpenFile(file.clone()), cx)),
    );
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
    let unlink_button = div().flex().pt(px(8.)).child(
        Button::new(format!("site:{name}:unlink"), if proxy { "Remove proxy…" } else { "Unlink…" })
            .style(ButtonStyle::Destructive)
            .on_click(move |_, window, cx| {
                let (question, detail) = unlink_question(&n8, proxy);
                let answer =
                    window.prompt(PromptLevel::Warning, &question, Some(detail), &["Remove", "Cancel"], cx);
                let (env, name) = (e8.clone(), n8.clone());
                cx.spawn(async move |cx| {
                    if answer.await == Ok(0) {
                        cx.update(|cx| unlink(&env, name, proxy, cx));
                    }
                })
                .detach();
            }),
    );
    pane.child(div().flex().flex_col().gap(px(8.)).child(actions).child(files).child(unlink_button))
        .into_any_element()
}

/// Records an open's failure under `id`, or clears it on success, so the issues strip shows it.
fn record(env: &AppEnv, id: ActionId, result: Result<(), yard_proto::YardError>, cx: &mut App) {
    env.runner.update(cx, |r, cx| {
        match result {
            Ok(()) => r.errors.clear(&id),
            Err(e) => r.errors.record(id, e),
        }
        cx.notify();
    });
}
