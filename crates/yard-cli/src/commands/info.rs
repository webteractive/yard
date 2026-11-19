//! `yard info`: what one site runs on (spec §7.4).

use super::services::{account_parts, address, failure_line};
use super::{Ctx, Outcome, cwd_absolute};
use crate::output::{for_agents, table};
use yard_core::node::install_hint;
use yard_core::present::{missing_node, missing_runtimes, state_text};
use yard_proto::methods::{self, ExtListParams, SiteDetailParams};
use yard_proto::{
    ErrorCode, ExtInfo, Need, ProjectFileKind, ServiceState, SettingSource, SiteDetail, SiteKind, SiteNode,
    SiteRuntime, SiteSource, YardError,
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
    source_label(detail.sources.get(key))
}

/// How `yard info` and `yard node unuse` name where a setting comes from.
pub(super) fn source_label(source: Option<&SettingSource>) -> &'static str {
    match source {
        Some(SettingSource::ProjectFile) => ".yard",
        Some(SettingSource::ManagedFile) => "managed",
        Some(SettingSource::SitesToml) => "sites.toml",
        Some(SettingSource::Default) | None => "default",
        Some(SettingSource::Nvmrc) => ".nvmrc",
        Some(SettingSource::NodeVersionFile) => ".node-version",
        Some(SettingSource::PinFile) => "pin file",
        Some(SettingSource::YardToml) => "yard.toml",
        Some(SettingSource::ServicesToml) => "services.toml",
        Some(SettingSource::Other) => "elsewhere",
    }
}

/// Where a runtime's version comes from: as [`source_label`], except that a runtime's
/// default is the one written in `yard.toml` (explicit-defaults spec §2.5).
fn runtime_label(source: Option<&SettingSource>) -> &'static str {
    match source {
        Some(SettingSource::Default) | None => "default (yard.toml)",
        other => source_label(other),
    }
}

/// The `node` row: the version and where its spec comes from, or why there is none. An
/// unusable pin names no source here; `problems` shows its error, which names the file.
fn node_row(node: &SiteNode) -> (String, Option<&'static str>) {
    let source = Some(runtime_label(Some(&node.source)));
    match (&node.spec, &node.version, &node.error) {
        (_, _, Some(_)) | (None, _, _) => ("unusable pin".to_string(), None),
        (Some(spec), Some(v), None) if spec == v => (v.clone(), source),
        (Some(spec), Some(v), None) => (format!("{v} ({spec})"), source),
        (Some(spec), None, None) => (format!("{spec}, not installed"), source),
    }
}

/// A runtime's row (Composer): as Node's, plus a key no runtime defines and a version too
/// new for the site's PHP. `problems` shows the error itself.
fn runtime_row(rt: &SiteRuntime) -> (String, Option<&'static str>) {
    let source = Some(runtime_label(Some(&rt.source)));
    match (&rt.spec, &rt.version, &rt.error) {
        (_, _, Some(e)) if e.code == ErrorCode::RuntimeUnknown => ("unknown runtime".to_string(), None),
        (_, _, Some(e)) if e.data.as_ref().is_some_and(|d| d.get("needs_pin").is_some()) => {
            ("needs a pin in .yard".to_string(), None)
        }
        (Some(spec), _, Some(e)) if e.data.as_ref().and_then(|d| d.get("needs_php")).is_some() => {
            (format!("{spec}, needs a newer PHP"), source)
        }
        (_, _, Some(_)) | (None, _, _) => ("unusable pin".to_string(), None),
        (Some(spec), Some(v), None) if spec == v => (v.clone(), source),
        (Some(spec), Some(v), None) => (format!("{v} ({spec})"), source),
        (Some(spec), None, None) => (format!("{spec}, not installed"), source),
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
    let mut settings: Vec<(&str, String, Option<&'static str>)> = vec![];
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
            settings.push(("php", shown_php, Some(runtime_label(detail.sources.get("php")))));
            if let Some(node) = &site.node {
                let (value, source) = node_row(node);
                settings.push(("node", value, source));
            }
            for rt in &site.runtimes {
                let (value, source) = runtime_row(rt);
                settings.push((rt.key.as_str(), value, source));
            }
            settings.push(("docroot", shown_docroot, Some(source_name(detail, "docroot"))));
        }
        SiteKind::Proxy { target } => out.push_str(&line("target", target)),
        SiteKind::Other => out.push_str(&line("kind", "unknown")),
    }
    settings.push(("https", https.to_string(), Some(source_name(detail, "secure"))));
    let width = settings.iter().map(|(_, v, _)| v.chars().count()).max().unwrap_or(0).max(VALUE);
    for (label, value, source) in &settings {
        match source {
            Some(source) => out.push_str(&line(label, &format!("{value:<width$}  from {source}"))),
            None => out.push_str(&line(label, value)),
        }
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
    let mut sleeping: Option<&str> = None;
    let now = time::OffsetDateTime::now_utc();
    for wanted in &detail.services {
        match &wanted.service {
            Some(s) => {
                let (user, rest) = account_parts(&s.account);
                rows.push(vec![s.name.clone(), state_text(s, now), address(s), user, rest]);
                if s.state == ServiceState::Stopped && s.sleeping_since.is_some() {
                    sleeping.get_or_insert(&s.name);
                }
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
    if let Some(name) = sleeping {
        out.push_str(&format!("  wakes on the next request, or: yard service start {name}\n"));
    }
    for line in consoles.into_iter().chain(failures) {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

/// A `.yard` that does not parse and an unusable Node pin, then the unmet needs with the
/// command that fixes each; a missing Node is one of them, though it never stops serving.
fn problems(detail: &SiteDetail) -> String {
    let mut out = String::new();
    let node_error = detail.site.node.as_ref().and_then(|n| n.error.as_ref());
    let runtime_errors: Vec<&YardError> =
        detail.site.runtimes.iter().filter_map(|r| r.error.as_ref()).collect();
    for e in detail.site.project_error.iter().chain(node_error).chain(runtime_errors.iter().copied()) {
        out.push_str(&line("error", &e.message));
        if let Some(hint) = &e.hint {
            out.push_str(&line("", &format!("→ {hint}")));
        }
    }
    let node =
        missing_node(detail.site.node.as_ref()).map(|spec| format!("Node {spec}  ({})", install_hint(&spec)));
    let runtimes = missing_runtimes(&detail.site.runtimes)
        .into_iter()
        .map(|(rt, spec)| format!("{} {spec}  (yard runtime install {} {spec})", rt.name, rt.key));
    let mut needs = detail
        .site
        .needs
        .iter()
        .map(|n: &Need| format!("{}  ({})", n.describe(), n.command()))
        .chain(node)
        .chain(runtimes);
    match needs.next() {
        None if detail.site.project_error.is_none() && node_error.is_none() && runtime_errors.is_empty() => {
            out.push_str(&line("needs", "nothing"))
        }
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
