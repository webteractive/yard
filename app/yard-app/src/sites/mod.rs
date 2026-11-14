//! Sites (M6 design §8.2): the list, a site's detail, linking a folder or adding a proxy.

pub mod detail;
pub mod forms;

use crate::env::{AppEnv, Sheet};
use crate::runner::ActionId;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::error_view::ErrorView;
use crate::widgets::table::{Column, Row, Table};
use crate::widgets::text_field::TextField;
use gpui::{
    AnyElement, App, AppContext, IntoElement, ParentElement, PathPromptOptions, Styled, Window, div, px,
};
use yard_app_model::hint::{HintAction, Screen};
use yard_proto::SiteKind;

/// The actions on a site's detail whose errors the Sites screen shows beside it, as
/// `site:<name>:<verb>`; with `site:link` and `site:proxy`.
pub const DETAIL_VERBS: [&str; 7] = ["php", "secure", "docroot", "init", "env", "unlink", "node"];

pub fn link_folder(env: &AppEnv, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Link".into()),
    });
    let env = env.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = picked.await else { return };
        let Some(folder) = paths.into_iter().next() else { return };
        let default = folder.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        cx.update(|cx| {
            let name = cx.new(|cx| {
                let mut f = TextField::new("sheet:link:name", "Site name", "the folder's name", cx);
                f.set_value(default, cx);
                f
            });
            env.nav.update(cx, |n, cx| {
                n.sheet = Some(Sheet::Link { folder, name });
                cx.notify();
            });
        });
    })
    .detach();
}

pub fn sites(env: &AppEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    let selected = match &env.nav.read(cx).screen {
        Screen::Sites(s) => s.clone(),
        _ => None,
    };
    let sites = env.state.read(cx).sites.value.clone().unwrap_or_default();
    let rows = sites.iter().map(|s| {
        let target = match &s.kind {
            SiteKind::Php { php, .. } => format!("PHP {php}"),
            SiteKind::Proxy { target } => target.clone(),
            SiteKind::Other => String::new(),
        };
        let status = if s.project_error.is_some() {
            "error"
        } else if s.needs_setup() {
            "needs setup"
        } else {
            "serving"
        };
        Row::new(
            s.name.clone(),
            vec![
                s.name.clone().into(),
                s.url.clone().into(),
                target.into(),
                (if s.secure { "on" } else { "off" }).into(),
                status.into(),
            ],
        )
    });
    let select = env.clone();
    let table = Table::new(
        "sites",
        [
            Column::flex("Name"),
            Column::flex("URL").grow(2.),
            Column::fixed("PHP or target", 110.),
            Column::fixed("HTTPS", 60.),
            Column::fixed("Status", 120.).pill(),
        ],
        rows.collect(),
    )
    .empty("No sites linked. Link a project folder to serve it.", None)
    .selected(selected.clone().map(Into::into))
    .on_select(move |key, _, cx| {
        crate::window::main::open(&select, Screen::Sites(Some(key.to_string())), cx)
    });
    let runner = env.runner.read(cx);
    let mut ids: Vec<String> = vec!["site:link".into(), "site:proxy".into()];
    if let Some(n) = &selected {
        ids.extend(DETAIL_VERBS.iter().map(|v| format!("site:{n}:{v}")));
    }
    let errors: Vec<_> = ids
        .into_iter()
        .filter_map(|id| {
            let e = runner.errors.get(&ActionId(id.clone()))?.clone();
            let env = env.clone();
            Some(
                ErrorView::new(format!("{id}:error"), &e, HintAction::parse(e.hint.as_deref().unwrap_or("")))
                    .on_action(move |a, _, cx| env.perform(a, cx)),
            )
        })
        .collect();
    let detail = selected.map(|n| detail::detail(env, &n, window, cx));
    div().flex().flex_col().gap(px(12.)).children(errors).child(table).children(detail).into_any_element()
}

/// The Sites toolbar's buttons.
pub fn actions(env: &AppEnv) -> Vec<AnyElement> {
    let (link, proxy) = (env.clone(), env.clone());
    vec![
        Button::new("site:proxy", "Add a proxy…")
            .on_click(move |_, _, cx| {
                let name = cx.new(|cx| TextField::new("sheet:proxy:name", "Name", "api", cx));
                let url =
                    cx.new(|cx| TextField::new("sheet:proxy:url", "Target URL", "http://127.0.0.1:3000", cx));
                proxy.nav.update(cx, |n, cx| {
                    n.sheet = Some(Sheet::Proxy { name, url });
                    cx.notify();
                });
            })
            .into_any_element(),
        Button::new("site:link", "Link a folder…")
            .style(ButtonStyle::Primary)
            .on_click(move |_, _, cx| link_folder(&link, cx))
            .into_any_element(),
    ]
}

#[cfg(test)]
mod tests;
