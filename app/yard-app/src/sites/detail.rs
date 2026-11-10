//! A site's detail (M6 design §8.2 Sites): each setting with where it comes from, and its controls.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::platform::open::Opener;
use crate::runner::ActionId;
use crate::services::table::copy_env;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::group::Group;
use crate::widgets::picker::Picker;
use crate::widgets::text;
use crate::widgets::theme::Theme;
use crate::widgets::theme::Typography;
use crate::widgets::toggle::Toggle;
use gpui::{AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, px};
use std::path::{Path, PathBuf};
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

/// `PHP 8.4.3 — from the managed .yard`, and the same for the docroot, HTTPS and Node.
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
    if let Some(node) = &d.site.node {
        let source = match &node.file {
            Some(file) => format!("from {file}"),
            None => from(Some(&node.source)).to_string(),
        };
        lines.push(match (&node.error, &node.version, &node.spec) {
            (Some(e), _, _) => format!("Node: {}", e.message),
            (None, Some(version), _) => format!("Node {version} — {source}"),
            (None, None, spec) => {
                format!("Node {} — not installed, {source}", spec.as_deref().unwrap_or("?"))
            }
        });
    }
    lines
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
    // PHP, HTTPS, where each setting comes from and the services, in one group box (spec §4).
    let mut group = Group::new(format!("site:{name}:settings"), None);
    if let Some(d) = &detail {
        group = group.row_below(
            "sources",
            "Settings",
            div().flex().flex_col().children(
                source_lines(d)
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| div().child(text(format!("site:{name}:source:{i}"), l))),
            ),
        );
        if let SiteKind::Php { php, .. } = &site.kind {
            let mut options: Vec<SharedString> = vec!["Default".into()];
            options.extend(php_options.iter().map(|v| SharedString::from(v.clone())));
            let selected = php_options.iter().position(|v| v == php).map(|i| i + 1);
            let (e, n, versions) = (env.clone(), name.to_string(), php_options.clone());
            let open_yard =
                match (project_lock(&site), site.project_file.clone()) {
                    (Some(_), Some(file)) => {
                        let e = env.clone();
                        Some(Button::new(format!("site:{name}:open-yard"), "Open .yard").small().on_click(
                            move |_, _, cx| e.perform(HintAction::OpenFile(PathBuf::from(&file)), cx),
                        ))
                    }
                    _ => None,
                };
            group = group.row(
                "php",
                "PHP",
                crate::widgets::bar()
                    .child(
                        Picker::new(format!("site:{name}:php"), format!("PHP for {name}"), options, selected)
                            .disabled_because(project_lock(&site).map(Into::into))
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
                    )
                    .children(open_yard),
            );
        }
        if !d.services.is_empty() {
            let lines = d.services.iter().map(|svc| {
                let met = if svc.service.is_some() { "met" } else { "missing" };
                let want = svc.want.as_deref().map(|w| format!(" {w}")).unwrap_or_default();
                div().child(text(
                    format!("site:{name}:service:{}", svc.engine),
                    format!("{}{want} — {met}", yard_proto::engine_display(&svc.engine)),
                ))
            });
            group = group.row_below("services", "Services", div().flex().flex_col().children(lines));
        }
    }
    let lock = project_lock(&site);
    let (e, n, secure) = (env.clone(), name.to_string(), site.secure);
    group = group.row(
        "https",
        "HTTPS",
        Toggle::new(format!("site:{name}:secure"), format!("HTTPS for {name}"), secure)
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
    );
    pane = pane.child(group);
    if node_pinned_by_yard(&site) {
        let (e, n) = (env.clone(), name.to_string());
        pane = pane.child(Button::new(format!("site:{name}:node-unuse"), "Use default Node").on_click(
            move |_, _, cx| {
                let (daemon, site) = (e.daemon.clone(), n.clone());
                quick(
                    &e,
                    format!("site:{n}:node"),
                    async move { call(&*daemon, api::NODE_UNUSE, SiteParams { site }).await.map(|_| ()) },
                    cx,
                )
            },
        ));
    }
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
        pane = pane.child(
            div()
                .type_body()
                .text_color(t.c.danger_text)
                .child(text(format!("site:{name}:error"), e.message.clone())),
        );
    }
    let mut actions = crate::widgets::bar();
    let url = site.url.clone();
    actions = actions.child(
        Button::new(format!("site:{name}:browser"), "Open in browser")
            .on_click(move |_, _, cx| cx.open_url(&url)),
    );
    if let SiteKind::Php { path, .. } = &site.kind {
        let path = PathBuf::from(path);
        let (e1, p1) = (env.clone(), path.clone());
        actions = actions.child(Button::new(format!("site:{name}:editor"), "Open in editor").on_click(
            move |_, _, cx| {
                let id = ActionId(format!("{}folder", crate::window::main::OPEN_ERRORS));
                let result = Opener::open_folder(cx, &p1, e1.settings.editor_app().as_deref());
                e1.runner.update(cx, |r, cx| {
                    match result {
                        Ok(()) => r.errors.clear(&id),
                        Err(e) => r.errors.record(id, e),
                    }
                    cx.notify();
                });
            },
        ));
        let p2 = path.clone();
        actions = actions.child(
            Button::new(format!("site:{name}:reveal"), "Reveal in Finder")
                .on_click(move |_, _, cx| Opener::reveal(cx, &p2)),
        );
        let (e3, p3) = (env.clone(), path.clone());
        actions = actions.child(Button::new(format!("site:{name}:terminal"), "Open in Terminal").on_click(
            move |_, _, cx| {
                let app = e3
                    .settings
                    .terminal_app()
                    .unwrap_or_else(|| PathBuf::from("/System/Applications/Utilities/Terminal.app"));
                let id = ActionId(format!("{}terminal", crate::window::main::OPEN_ERRORS));
                let result = Opener::open_with(&p3, Some(&app));
                e3.runner.update(cx, |r, cx| {
                    match result {
                        Ok(()) => r.errors.clear(&id),
                        Err(e) => r.errors.record(id, e),
                    }
                    cx.notify();
                });
            },
        ));
        let (e4, n4) = (env.clone(), name.to_string());
        actions = actions.child(
            Button::new(format!("site:{name}:detect"), "Detect docroot").disabled(lock.is_some()).on_click(
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
            ),
        );
        let (e5, n5, p5) = (env.clone(), name.to_string(), path.clone());
        actions = actions.child(
            Button::new(format!("site:{name}:init"), "Save settings to .yard")
                .disabled(lock.is_some())
                .on_click(move |_, window, cx| {
                    save_settings(&e5, &n5, &p5.display().to_string(), window, cx)
                }),
        );
    }
    let (e6, file) = (env.clone(), env.paths().nginx_override(name));
    actions = actions.child(
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
    actions = actions.child(
        Button::new(format!("site:{name}:unlink"), if proxy { "Remove proxy…" } else { "Unlink…" })
            .style(ButtonStyle::Destructive)
            .on_click(move |_, window, cx| {
                let (question, detail) = unlink_question(&n8, proxy);
                let answer =
                    window.prompt(PromptLevel::Warning, &question, Some(detail), &["Remove", "Cancel"], cx);
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
