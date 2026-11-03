//! `sites.toml`: links, proxies and docroot overrides, one `[sites.<name>]` table each.

use super::{config_error, read_optional};
use crate::Paths;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yard_proto::YardError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SitesConfig {
    pub sites: BTreeMap<String, SiteEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteEntry {
    /// Present for a linked site.
    pub path: Option<PathBuf>,
    /// Relative to the project, or absolute.
    pub docroot: Option<PathBuf>,
    /// Present for a proxied site.
    pub proxy: Option<String>,
    /// `yard isolate`: the PHP version this site runs instead of the default.
    pub php: Option<crate::PhpVersion>,
    /// `yard secure` / `unsecure`; absent follows `secure_by_default` in `yard.toml`.
    pub secure: Option<bool>,
    /// `yard node use`: the Node spec this site runs, unless a `.yard` or `.nvmrc` says.
    pub node: Option<crate::node::NodeSpec>,
    /// `[sites.<name>.tunnel]`: the site is shared through a named tunnel (tunnels spec §6).
    pub tunnel: Option<TunnelEntry>,
}

/// A named tunnel: `host` on a domain set up with `yard tunnels domain add`, through the
/// Cloudflare tunnel `tunnel` (absent: `yard-<site>`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelEntry {
    pub host: toml::Spanned<String>,
    pub tunnel: Option<String>,
    /// Absent means `cloudflared`, the only provider built.
    pub provider: Option<toml::Spanned<String>>,
}

impl TunnelEntry {
    pub fn host(&self) -> &str {
        self.host.get_ref()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    sites: BTreeMap<String, SiteEntry>,
}

pub fn load_sites(paths: &Paths) -> Result<SitesConfig, YardError> {
    let file = paths.sites_toml();
    parse_sites(read_optional(&file)?.as_deref(), &file)
}

pub fn parse_sites(text: Option<&str>, file: &Path) -> Result<SitesConfig, YardError> {
    let Some(text) = text else {
        return Ok(SitesConfig::default());
    };
    let raw: Raw = toml::from_str(text).map_err(|e| config_error(file, text, e.span(), e.message()))?;
    check_tunnels(&raw.sites, text, file)?;
    Ok(SitesConfig { sites: raw.sites })
}

/// Each tunnel's host is a DNS name no other site's tunnel uses, and its provider is built.
fn check_tunnels(sites: &BTreeMap<String, SiteEntry>, text: &str, file: &Path) -> Result<(), YardError> {
    let mut tunnels: Vec<(&str, &TunnelEntry)> =
        sites.iter().filter_map(|(name, s)| Some((name.as_str(), s.tunnel.as_ref()?))).collect();
    tunnels.sort_by_key(|(_, t)| t.host.span().start);
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, t) in tunnels {
        let error = |span, message: String| Err(config_error(file, text, Some(span), &message));
        if !crate::tunnel::valid_host(t.host()) {
            return error(
                t.host.span(),
                format!("tunnel host {:?} must be a lowercase DNS name with at least two labels", t.host()),
            );
        }
        if let Some(p) = t.provider.as_ref().filter(|p| p.get_ref() != "cloudflared") {
            return error(
                p.span(),
                format!("no tunnel provider {:?}; the providers are: cloudflared", p.get_ref()),
            );
        }
        if let Some(other) = seen.insert(t.host(), name) {
            return error(t.host.span(), format!("tunnel host {} is already {other}'s", t.host()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use yard_proto::ErrorCode;

    #[test]
    fn parses_sites_tables() {
        let text = "[sites.api]\nproxy = \"http://127.0.0.1:3000\"\n\n[sites.blog]\npath = \"/code/blog\"\ndocroot = \"web\"\n";
        let c = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap();
        assert_eq!(c.sites["api"].proxy.as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(c.sites["blog"].path, Some(PathBuf::from("/code/blog")));
        assert_eq!(c.sites["blog"].docroot, Some(PathBuf::from("web")));
    }

    #[test]
    fn secure_key_is_parsed() {
        let c = parse_sites(
            Some("[sites.a]\npath = \"/a\"\nsecure = true\n\n[sites.b]\npath = \"/b\"\n"),
            Path::new("/r/sites.toml"),
        )
        .unwrap();
        assert_eq!((c.sites["a"].secure, c.sites["b"].secure), (Some(true), None));
    }

    #[test]
    fn site_node_is_parsed() {
        let c = parse_sites(
            Some("[sites.old]\npath = \"/o\"\nnode = \"lts/iron\"\n"),
            Path::new("/r/sites.toml"),
        )
        .unwrap();
        assert_eq!(c.sites["old"].node, Some(crate::node::NodeSpec::Lts("iron".into())));
        let e = parse_sites(Some("[sites.old]\npath = \"/o\"\nnode = \"^20\"\n"), Path::new("/r/sites.toml"))
            .unwrap_err();
        assert_eq!(e.code, yard_proto::ErrorCode::ConfigInvalid);
        assert_eq!(e.data.unwrap()["line"], 3);
    }

    #[test]
    fn site_php_is_parsed() {
        let c = parse_sites(Some("[sites.old]\npath = \"/o\"\nphp = \"7.1\"\n"), Path::new("/r/sites.toml"))
            .unwrap();
        assert_eq!(c.sites["old"].php, Some("7.1".parse().unwrap()));
    }

    #[test]
    fn unsupported_site_php_is_config_invalid() {
        let e = parse_sites(Some("[sites.old]\npath = \"/o\"\nphp = \"5.6\"\n"), Path::new("/r/sites.toml"))
            .unwrap_err();
        assert_eq!(e.code, yard_proto::ErrorCode::ConfigInvalid);
        assert_eq!(e.data.unwrap()["line"], 3);
        assert!(e.message.contains("7.0–8.5"), "{}", e.message);
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(parse_sites(None, Path::new("/r/sites.toml")).unwrap().sites.is_empty());
    }

    #[test]
    fn unknown_site_key_is_rejected() {
        let e =
            parse_sites(Some("[sites.a]\nphp_version = \"8.4\"\n"), Path::new("/r/sites.toml")).unwrap_err();
        assert!(e.message.contains("php_version"), "{}", e.message);
    }

    #[test]
    fn site_tunnel_table_is_parsed() {
        let text = "[sites.blog]\npath = \"/b\"\n\n[sites.blog.tunnel]\nhost = \"blog.example.com\"\ntunnel = \"blog-shared\"\n";
        let c = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap();
        let t = c.sites["blog"].tunnel.as_ref().unwrap();
        assert_eq!((t.host(), t.tunnel.as_deref()), ("blog.example.com", Some("blog-shared")));
    }

    #[test]
    fn bad_tunnel_host_is_config_invalid_at_its_line() {
        for host in ["Blog.example.com", "localhost", "a..b", "-a.b"] {
            let text = format!("[sites.blog]\npath = \"/b\"\n\n[sites.blog.tunnel]\nhost = {host:?}\n");
            let e = parse_sites(Some(&text), Path::new("/r/sites.toml")).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfigInvalid, "{host}");
            assert_eq!(e.data.unwrap()["line"], 5, "{host}");
        }
    }

    #[test]
    fn a_host_two_sites_share_is_config_invalid_at_the_later_one() {
        let text = "[sites.a.tunnel]\nhost = \"x.example.com\"\n\n[sites.b]\npath = \"/b\"\n\n[sites.b.tunnel]\nhost = \"x.example.com\"\n";
        let e = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap_err();
        assert_eq!(e.data.unwrap()["line"], 8);
    }

    #[test]
    fn an_unbuilt_provider_is_config_invalid() {
        let text =
            "[sites.b]\npath = \"/b\"\n\n[sites.b.tunnel]\nhost = \"b.example.com\"\nprovider = \"ngrok\"\n";
        let e = parse_sites(Some(text), Path::new("/r/sites.toml")).unwrap_err();
        assert!(e.message.contains("cloudflared"), "{}", e.message);
    }
}
