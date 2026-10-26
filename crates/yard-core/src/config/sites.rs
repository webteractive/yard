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
    Ok(SitesConfig { sites: raw.sites })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

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
}
