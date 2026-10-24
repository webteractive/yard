//! `yard info`: what one site runs on (spec §7.4).

use super::services::{account_parts, address, failure_line, state_name};
use super::{Ctx, Outcome, cwd_absolute};
use crate::output::{for_agents, table};
use yard_proto::methods::{self, ExtListParams, SiteDetailParams};
use yard_proto::{
    ExtInfo, Need, ProjectFileKind, SettingSource, SiteDetail, SiteKind, SiteSource, YardError,
};

/// The label column: `extensions` is the longest.
const LABEL: usize = 10;
/// The width a setting's value is padded to before `from <source>`, at least.
const VALUE: usize = 11;

pub async fn info(ctx: &Ctx, site: Option<String>) -> Outcome {
    let params = match site {
        Some(site) => SiteDetailParams { site: Some(site), path: None },
        None => SiteDetailParams { path: Some(cwd_absolute()?), site: None },
    };
    let mut client = ctx.client().await?;
    let detail: SiteDetail = client.call(methods::SITES_INFO, params).await?;
    if ctx.json {
        ctx.emit(&detail, String::new);
        return Ok(0);
    }
    // `SiteDetail` carries no extensions, so the ones the site's PHP has enabled come from
    // `ext.list`. Without them the line is left out: that is no reason to fail `info`.
    let extensions = match (&detail.site.kind, &detail.php_full) {
        (SiteKind::Php { php, .. }, Some(_)) => {
            let listed: Result<Vec<ExtInfo>, YardError> =
                client.call(methods::EXT_LIST, ExtListParams { php: Some(php.clone()) }).await;
            listed.ok().map(|exts| enabled(&exts))
        }
        _ => None,
    };
    let home = std::env::var("HOME").ok();
    print!("{}", render(&detail, extensions.as_deref(), home.as_deref()));
    Ok(0)
}

/// The extensions Yard added and left enabled; the ones PHP bundles are not news.
fn enabled(exts: &[ExtInfo]) -> Vec<String> {
    exts.iter().filter(|e| e.enabled && e.source != "bundled").map(|e| e.name.clone()).collect()
}

fn source_name(detail: &SiteDetail, key: &str) -> &'static str {
    match detail.sources.get(key) {
        Some(SettingSource::ProjectFile) => ".yard",
        Some(SettingSource::ManagedFile) => "managed",
        Some(SettingSource::SitesToml) => "sites.toml",
        Some(SettingSource::Default) | None => "default",
    }
}

/// `path` with the home directory as `~`.
fn tilde(path: &str, home: Option<&str>) -> String {
    match home.filter(|h| !h.is_empty()) {
        Some(home) => match path.strip_prefix(home) {
            Some("") => "~".to_string(),
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_string(),
        },
        None => path.to_string(),
    }
}

fn line(label: &str, value: &str) -> String {
    format!("  {label:<LABEL$}  {value}\n").replace(" \n", "\n")
}

fn render(detail: &SiteDetail, extensions: Option<&[String]>, home: Option<&str>) -> String {
    let site = &detail.site;
    let serving = if site.needs_setup() { "needs setup" } else { "serving" };
    let url = site.url.trim_end_matches('/');
    let mut out = format!("{}   {url:<28}  {serving}\n", site.name);

    // Each setting: its value and where it came from, aligned.
    let https = if site.secure { "on" } else { "off" };
    let mut settings: Vec<(&str, String, &'static str)> = vec![];
    match &site.kind {
        SiteKind::Php { path, docroot, php } => {
            let shown_php = detail.php_full.clone().unwrap_or_else(|| php.clone());
            let relative = docroot.strip_prefix(path.as_str()).map(|r| r.trim_start_matches('/'));
            let shown_docroot = match relative {
                Some("") => ".".to_string(),
                Some(r) => r.to_string(),
                None => tilde(docroot, home),
            };
            out.push_str(&line("path", &tilde(path, home)));
            let config = match (&site.project_file, site.project_file_kind) {
                (Some(file), Some(ProjectFileKind::Managed)) => format!("managed ({})", tilde(file, home)),
                (Some(_), _) => ".yard".to_string(),
                (None, _) => "none".to_string(),
            };
            out.push_str(&line("config", &config));
            settings.push(("php", shown_php, source_name(detail, "php")));
            settings.push(("docroot", shown_docroot, source_name(detail, "docroot")));
        }
        SiteKind::Proxy { target } => out.push_str(&line("target", target)),
    }
    settings.push(("https", https.to_string(), source_name(detail, "secure")));
    let width = settings.iter().map(|(_, v, _)| v.chars().count()).max().unwrap_or(0).max(VALUE);
    for (label, value, source) in &settings {
        out.push_str(&line(label, &format!("{value:<width$}  from {source}")));
    }
    if let Some(exts) = extensions {
        let shown = if exts.is_empty() { "none".to_string() } else { exts.join(", ") };
        out.push_str(&line("extensions", &shown));
    }
    if site.source == SiteSource::Proxy {
        // A proxy has no .yard, services or needs of its own.
        out.push('\n');
        out.push_str(&for_agents("yard info"));
        return out;
    }

    if !detail.services.is_empty() {
        out.push('\n');
        out.push_str(&services_block(detail));
    }
    out.push('\n');
    out.push_str(&problems(detail));
    out.push('\n');
    out.push_str(&for_agents("yard info"));
    out
}

/// What the `.yard` asks for, each with its instance, state, address and account; an
/// instance that does not exist yet is `not added`, and the needs below name the fix.
fn services_block(detail: &SiteDetail) -> String {
    let mut rows: Vec<Vec<String>> = vec![];
    let mut consoles: Vec<String> = vec![];
    let mut failures: Vec<String> = vec![];
    for wanted in &detail.services {
        match &wanted.service {
            Some(s) => {
                let (user, rest) = account_parts(&s.account);
                rows.push(vec![s.name.clone(), state_name(&s.state).into(), address(s), user, rest]);
                if let Some(console) = &s.console {
                    consoles.push(format!("{} console  {console}", s.name));
                }
                failures.extend(failure_line(s));
            }
            None => {
                let name = match &wanted.want {
                    Some(v) => format!("{}-{v}", wanted.engine),
                    None => wanted.engine.clone(),
                };
                rows.push(vec![name, "not added".into(), String::new(), String::new(), String::new()]);
            }
        }
    }
    let mut out = String::new();
    for l in table(&["services", "", "", "account", ""], &rows).lines() {
        out.push_str(&format!("  {l}\n"));
    }
    for line in consoles.into_iter().chain(failures) {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

/// A `.yard` that does not parse, then the unmet needs with the command that fixes each.
fn problems(detail: &SiteDetail) -> String {
    let mut out = String::new();
    if let Some(e) = &detail.site.project_error {
        out.push_str(&line("error", &e.message));
        if let Some(hint) = &e.hint {
            out.push_str(&line("", &format!("→ {hint}")));
        }
    }
    let mut needs = detail.site.needs.iter().map(|n: &Need| format!("{}  ({})", n.describe(), n.command()));
    match needs.next() {
        None if detail.site.project_error.is_none() => out.push_str(&line("needs", "nothing")),
        None => {}
        Some(first) => {
            out.push_str(&line("needs", &first));
            for next in needs {
                out.push_str(&line("", &next));
            }
        }
    }
    out
}
