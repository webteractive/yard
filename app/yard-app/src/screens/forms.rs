//! The sheets that replaced the screens' inline forms (spec §5 step 15): Tunnels' Share… and Add
//! domain…, Ports' Range… and Node's Install…. Their fields keep the ids and labels they had
//! inline, so their values survive between openings and the checklist types into them as before.

use super::{job, value};
use crate::env::{AppEnv, Nav, Sheet};
use crate::widgets::button::Button;
use crate::widgets::form::Form;
use crate::widgets::text_field::TextField;
use gpui::{AnyElement, App, Entity, IntoElement, ParentElement, Window};
use yard_client::api;
use yard_proto::methods::{NodeInstallParams, TunnelDomainAddParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inline {
    Share,
    AddDomain,
    Range,
    NodeInstall,
}

/// Opens `kind`'s sheet.
pub fn open(env: &AppEnv, kind: Inline, cx: &mut App) {
    env.nav.update(cx, |n, cx| {
        n.sheet = Some(Sheet::Inline(kind));
        cx.notify();
    });
}

fn close(env: &AppEnv, cx: &mut App) {
    env.nav.update(cx, |n, cx| {
        n.sheet = None;
        cx.notify();
    });
}

/// A screen's field, shown as a sheet's: full width with its label above it.
fn field(env: &AppEnv, id: &str, label: &str, placeholder: &str, cx: &mut App) -> Entity<TextField> {
    let f = Nav::field(env, id, label, placeholder, cx);
    f.update(cx, |f, cx| {
        f.in_sheet();
        cx.notify();
    });
    f
}

pub fn sheet(env: &AppEnv, sheet: &Sheet, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let Sheet::Inline(kind) = sheet else { return None };
    let cancel = env.clone();
    let form = match kind {
        Inline::Share => {
            let site = field(env, "tunnel:share:site", "Site to share", "blog", cx);
            let host = field(
                env,
                "tunnel:share:host",
                "Host on your domain",
                "blog.example.com",
                cx,
            );
            let (e1, s1) = (env.clone(), site.clone());
            let (e2, s2, h2) = (env.clone(), site.clone(), host.clone());
            Form::new("sheet:share", "Share a site")
                .field(site)
                .field(host)
                .field(crate::widgets::bar().child(
                    Button::new("tunnel:share:named", "Share on my domain").on_click(move |_, window, cx| {
                        if let (Some(site), Some(host)) = (value(&s2, cx), value(&h2, cx)) {
                            super::tunnels::share(&e2, site, Some(host), false, window, cx);
                            close(&e2, cx);
                        }
                    }),
                ))
                .submit("Share (quick tunnel)", false, move |_, window, cx| {
                    if let Some(site) = value(&s1, cx) {
                        super::tunnels::share(&e1, site, None, false, window, cx);
                        close(&e1, cx);
                    }
                })
        }
        Inline::AddDomain => {
            let domain = field(env, "tunnel:domain:add", "Domain to add", "example.com", cx);
            let (e, d) = (env.clone(), domain.clone());
            Form::new("sheet:add-domain", "Add a domain for named tunnels").field(domain).submit(
                "Add domain",
                false,
                move |_, _, cx| {
                    if let Some(d) = value(&d, cx) {
                        job(
                            &e,
                            format!("domain:{d}:add"),
                            api::TUNNELS_DOMAIN_ADD,
                            TunnelDomainAddParams { provider: None, domain: d.clone() },
                            format!("Sign in to Cloudflare for {d}"),
                            cx,
                        );
                        close(&e, cx);
                    }
                },
            )
        }
        Inline::Range => {
            let range = field(env, "port:range", "Auto range", "29000-29099", cx);
            let (e, r) = (env.clone(), range.clone());
            Form::new("sheet:range", "Ports Yard picks from").field(range).submit(
                "Set range",
                false,
                move |_, _, cx| {
                    if let Some(r) = value(&r, cx) {
                        super::ports::set_range(&e, r, cx);
                        close(&e, cx);
                    }
                },
            )
        }
        Inline::NodeInstall => {
            let spec = field(env, "node:install:spec", "Node version to install", "22, 22.17, lts/*", cx);
            let (e, s) = (env.clone(), spec.clone());
            Form::new("sheet:node-install", "Install Node").field(spec).submit(
                "Install",
                false,
                move |_, _, cx| {
                    if let Some(spec) = value(&s, cx) {
                        job(
                            &e,
                            "node:install".into(),
                            api::NODE_INSTALL,
                            NodeInstallParams { spec: Some(spec.clone()), path: None },
                            format!("Install Node {spec}"),
                            cx,
                        );
                        close(&e, cx);
                    }
                },
            )
        }
    };
    Some(form.on_cancel(move |_, _, cx| close(&cancel, cx)).into_any_element())
}
