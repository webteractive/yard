//! Tunnels (M6 design §8.2): sites shared through Cloudflare, and the domains for named tunnels.

use super::{errors, heading, job, quick, value};
use crate::env::{AppEnv, Nav};
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::table::{Row, Table};
use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use yard_client::api;
use yard_proto::methods::{TunnelAddParams, TunnelDomainAddParams, TunnelDomainParams, TunnelRemoveParams};

pub fn tunnels(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let s = env.state.read(cx);
    let tunnels = s.tunnels.value.clone().unwrap_or_default();
    let domains = s.tunnel_domains.value.clone().unwrap_or_default();
    let rows = tunnels.iter().map(|t| {
        let (e, site) = (env.clone(), t.site.clone());
        let url = t.url.clone().unwrap_or_else(|| "-".into());
        let mut actions = div().flex().gap(px(4.));
        if let Some(u) = t.url.clone() {
            actions = actions.child(
                Button::new(format!("tunnel:{}:open", t.site), "Open")
                    .on_click(move |_, _, cx| cx.open_url(&u)),
            );
        }
        actions = actions.child(
            Button::new(format!("tunnel:{}:stop", t.site), "Stop sharing")
                .style(ButtonStyle::Destructive)
                .on_click(move |_, _, cx| {
                    job(
                        &e,
                        format!("tunnel:{site}:stop"),
                        api::TUNNELS_REMOVE,
                        TunnelRemoveParams { site: site.clone(), keep_tunnel: false },
                        format!("Stop sharing {site}"),
                        cx,
                    )
                }),
        );
        Row::new(
            t.site.clone(),
            vec![
                t.site.clone().into(),
                format!("{:?}", t.kind).to_lowercase().into(),
                url.into(),
                format!("{:?}", t.state).to_lowercase().into(),
            ],
        )
        .extra(actions)
    });
    let table = Table::new(
        "tunnels",
        ["Site", "Kind", "URL", "State"].iter().map(|c| SharedString::from(*c)).collect(),
        rows.collect(),
    );
    let site = Nav::field(env, "tunnel:share:site", "Site to share", "blog", cx);
    let (e1, site1) = (env.clone(), site.clone());
    let domain_rows = domains.iter().map(|d| {
        let (e, name) = (env.clone(), d.domain.clone());
        Row::new(
            d.domain.clone(),
            vec![d.domain.clone().into(), (if d.logged_in { "signed in" } else { "not signed in" }).into()],
        )
        .extra(Button::new(format!("domain:{}:remove", d.domain), "Remove").on_click(
            move |_, _, cx| {
                quick(
                    &e,
                    format!("domain:{name}:remove"),
                    api::TUNNELS_DOMAIN_REMOVE,
                    TunnelDomainParams { domain: name.clone() },
                    cx,
                )
            },
        ))
    });
    let domain_table =
        Table::new("domains", vec!["Domain".into(), "Cloudflare".into()], domain_rows.collect());
    let domain = Nav::field(env, "tunnel:domain:add", "Domain to add", "example.com", cx);
    let (e2, domain2) = (env.clone(), domain.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading("Tunnels"))
        .children(errors(env, "tunnel:", cx))
        .children(errors(env, "domain:", cx))
        .child(table)
        .child(div().flex().gap(px(6.)).child(site.clone()).child(
            Button::new("tunnel:share", "Share (quick tunnel)").style(ButtonStyle::Primary).on_click(
                move |_, _, cx| {
                    if let Some(site) = value(&site1, cx) {
                        let params = TunnelAddParams {
                            site: site.clone(),
                            provider: None,
                            host: None,
                            tunnel: None,
                            overwrite_dns: false,
                        };
                        job(
                            &e1,
                            format!("tunnel:{site}:share"),
                            api::TUNNELS_ADD,
                            params,
                            format!("Share {site}"),
                            cx,
                        );
                    }
                },
            ),
        ))
        .child(div().text_lg().child("Domains for named tunnels"))
        .child(domain_table)
        .child(div().flex().gap(px(6.)).child(domain.clone()).child(
            Button::new("domain:add", "Add domain").on_click(move |_, _, cx| {
                if let Some(d) = value(&domain2, cx) {
                    job(
                        &e2,
                        format!("domain:{d}:add"),
                        api::TUNNELS_DOMAIN_ADD,
                        TunnelDomainAddParams { provider: None, domain: d.clone() },
                        format!("Sign in to Cloudflare for {d}"),
                        cx,
                    );
                }
            }),
        ))
        .into_any_element()
}
