//! Tunnels (M6 design §8.2): sites shared through Cloudflare, and the domains for named tunnels.

use super::{errors, heading, job, quick, value};
use crate::env::{AppEnv, Nav};
use crate::platform::open::Opener;
use crate::runner::ActionId;
use crate::widgets::button::{Button, ButtonStyle};
use crate::widgets::table::{Row, Table};
use gpui::{AnyElement, App, IntoElement, ParentElement, PromptLevel, SharedString, Styled, Window, div, px};
use yard_app_model::jobs::JobPhase;
use yard_client::api;
use yard_proto::methods::{TunnelAddParams, TunnelDomainAddParams, TunnelDomainParams, TunnelRemoveParams};
use yard_proto::{ErrorCode, TunnelKind, TunnelRemoved, YardError};

/// What to delete in Cloudflare after a stop left DNS records, as `yard untunnel` says it.
pub fn dns_left_note(removed: &TunnelRemoved) -> Option<YardError> {
    if removed.dns_left.is_empty() {
        return None;
    }
    let records: Vec<String> = removed
        .dns_left
        .iter()
        .map(|r| {
            format!(
                "Delete the CNAME {} (→ {}) in the Cloudflare dashboard, under the zone's DNS records",
                r.host, r.target
            )
        })
        .collect();
    Some(
        YardError::new(
            ErrorCode::TunnelDnsFailed,
            format!(
                "The DNS record is still there. {}; until then visitors get a Cloudflare error page.",
                records.join("; ")
            ),
        )
        .with_hint("yard doctor"),
    )
}

/// Stops sharing `site` (`keep_tunnel` keeps a named tunnel's Cloudflare tunnel); DNS records the
/// stop left stay beside the tunnels as what to delete (`tunnel:<site>:dns`).
pub fn stop_sharing(env: &AppEnv, site: String, keep_tunnel: bool, cx: &mut App) {
    let task = env.runner.update(cx, |r, cx| {
        r.run_job(
            ActionId(format!("tunnel:{site}:stop")),
            api::TUNNELS_REMOVE,
            TunnelRemoveParams { site: site.clone(), keep_tunnel },
            format!("Stop sharing {site}"),
            cx,
        )
    });
    let runner = env.runner.clone();
    cx.spawn(async move |cx| {
        let JobPhase::Succeeded(Some(result)) = task.await else { return };
        let Ok(removed) = serde_json::from_value::<TunnelRemoved>(result) else { return };
        if let Some(note) = dns_left_note(&removed) {
            runner.update(cx, |r, cx| {
                r.errors.record(ActionId(format!("tunnel:{}:dns", removed.site)), note);
                cx.notify();
            });
        }
    })
    .detach();
}

/// Shares `site`: a quick tunnel, or a named one at `host` under a set-up domain. A host that
/// already has a DNS record (`TUNNEL_DNS_EXISTS`) asks whether to overwrite it, and shares again
/// with `overwrite_dns`.
pub fn share(
    env: &AppEnv,
    site: String,
    host: Option<String>,
    overwrite_dns: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let title = match &host {
        Some(h) => format!("Share {site} as {h}"),
        None => format!("Share {site}"),
    };
    let params = TunnelAddParams {
        site: site.clone(),
        provider: None,
        host: host.clone(),
        tunnel: None,
        overwrite_dns,
    };
    let task = env.runner.update(cx, |r, cx| {
        r.run_job(ActionId(format!("tunnel:{site}:share")), api::TUNNELS_ADD, params, title, cx)
    });
    let env = env.clone();
    window
        .spawn(cx, async move |cx| {
            let JobPhase::Failed(e) = task.await else { return };
            let Some(h) = host.clone().filter(|_| e.code == ErrorCode::TunnelDnsExists && !overwrite_dns)
            else {
                return;
            };
            let Ok(answer) = cx.update(|window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    &format!("Overwrite the DNS record for {h}?"),
                    Some(&e.message),
                    &["Overwrite", "Cancel"],
                    cx,
                )
            }) else {
                return;
            };
            if answer.await == Ok(0) {
                let _ = cx.update(|window, cx| share(&env, site, host, true, window, cx));
            }
        })
        .detach();
}

/// The table's column titles, each read out by VoiceOver.
pub const COLUMNS: [&str; 4] = ["Site", "Kind", "URL", "State"];

pub fn tunnels(env: &AppEnv, _window: &mut Window, cx: &mut App) -> AnyElement {
    let s = env.state.read(cx);
    let tunnels = s.tunnels.value.clone().unwrap_or_default();
    let domains = s.tunnel_domains.value.clone().unwrap_or_default();
    let rows = tunnels.iter().map(|t| {
        let (e, site, named) = (env.clone(), t.site.clone(), t.kind == TunnelKind::Named);
        let url = t.url.clone().unwrap_or_else(|| "-".into());
        let mut actions = div().flex().gap(px(4.));
        if let Some(u) = t.url.clone() {
            let copied = u.clone();
            actions = actions
                .child(
                    Button::new(format!("tunnel:{}:copy", t.site), "Copy URL")
                        .on_click(move |_, _, cx| Opener::copy(cx, &copied)),
                )
                .child(
                    Button::new(format!("tunnel:{}:open", t.site), "Open")
                        .on_click(move |_, _, cx| cx.open_url(&u)),
                );
        }
        actions = actions.child(
            Button::new(format!("tunnel:{}:stop", t.site), "Stop sharing")
                .style(ButtonStyle::Destructive)
                .on_click(move |_, window, cx| {
                    if !named {
                        stop_sharing(&e, site.clone(), false, cx);
                        return;
                    }
                    let answer = window.prompt(
                        PromptLevel::Warning,
                        &format!("Stop sharing {site}?"),
                        Some("Yard deletes the Cloudflare tunnel it made for it, unless you keep it for later."),
                        &["Stop sharing", "Keep the Cloudflare tunnel", "Cancel"],
                        cx,
                    );
                    let (env, site) = (e.clone(), site.clone());
                    cx.spawn(async move |cx| {
                        let keep = match answer.await {
                            Ok(0) => false,
                            Ok(1) => true,
                            _ => return,
                        };
                        cx.update(|cx| stop_sharing(&env, site, keep, cx));
                    })
                    .detach();
                }),
        );
        Row::new(
            t.site.clone(),
            vec![
                t.site.clone().into(),
                yard_core::present::wire_name(&t.kind).into(),
                url.into(),
                yard_core::present::wire_name(&t.state).into(),
            ],
        )
        .extra(actions)
    });
    let table =
        Table::new("tunnels", COLUMNS.iter().map(|c| SharedString::from(*c)).collect(), rows.collect());
    let site = Nav::field(env, "tunnel:share:site", "Site to share", "blog", cx);
    let host = Nav::field(env, "tunnel:share:host", "Host on your domain", "blog.example.com", cx);
    let (e1, site1) = (env.clone(), site.clone());
    let (e3, site3, host3) = (env.clone(), site.clone(), host.clone());
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
        .child(crate::widgets::bar().child(site.clone()).child(
            Button::new("tunnel:share", "Share (quick tunnel)").style(ButtonStyle::Primary).on_click(
                move |_, window, cx| {
                    if let Some(site) = value(&site1, cx) {
                        share(&e1, site, None, false, window, cx);
                    }
                },
            ),
        ))
        .child(crate::widgets::bar().child(host.clone()).child(
            Button::new("tunnel:share:named", "Share on my domain").on_click(move |_, window, cx| {
                if let (Some(site), Some(host)) = (value(&site3, cx), value(&host3, cx)) {
                    share(&e3, site, Some(host), false, window, cx);
                }
            }),
        ))
        .child(div().text_lg().child(crate::widgets::text("domains:heading", "Domains for named tunnels")))
        .child(domain_table)
        .child(crate::widgets::bar().child(domain.clone()).child(
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
