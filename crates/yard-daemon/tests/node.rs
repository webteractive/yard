//! A site's Node and the `node.*` methods (M4c spec §5–§7).

mod support;

use serde_json::{Value, json};
use std::path::Path;
use support::daemon::{Client, prepared};

async fn site(c: &mut Client, name: &str) -> Value {
    let list = c.call("sites.list", Value::Null).await.unwrap();
    list.as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap_or_else(|| panic!("{list}"))
}

async fn info(c: &mut Client, name: &str) -> Value {
    c.call("sites.info", json!({"site": name})).await.unwrap()["site"].clone()
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

#[tokio::test]
async fn a_site_with_an_nvmrc_reports_its_node() {
    let d = prepared("").await;
    d.fx.add_node("16.14.0");
    let dir = d.project("maker");
    write(&dir, ".nvmrc", "16.14.0\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let node = info(&mut c, "maker").await["node"].clone();
    assert_eq!(node["spec"], "16.14.0", "{node}");
    assert_eq!(node["source"], "nvmrc", "{node}");
    assert!(node["file"].as_str().unwrap().ends_with("/maker/.nvmrc"), "{node}");
    assert_eq!(node["version"], "16.14.0", "{node}");
    // Read per request: no reconcile, no watcher.
    write(&dir, ".nvmrc", "22\n");
    let node = site(&mut c, "maker").await["node"].clone();
    assert_eq!((node["spec"].as_str(), node["version"].as_str()), (Some("22"), None), "{node}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_php_only_site_has_no_node() {
    let d = prepared("").await;
    let dir = d.project("plain");
    let d = d.start().await;
    let mut c = d.client().await;
    let s = site(&mut c, "plain").await;
    assert!(s.get("node").is_none(), "{s}");
    write(&dir, "package.json", "{}");
    let node = site(&mut c, "plain").await["node"].clone();
    assert_eq!((node["spec"].as_str(), node["source"].as_str()), (Some("22"), Some("default")), "{node}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_deleted_project_does_not_break_the_list() {
    let d = prepared("").await;
    let gone = d.project("gone");
    let kept = d.project("kept");
    write(&gone, ".nvmrc", "20\n");
    write(&kept, ".nvmrc", "20\n");
    let d = d.start().await;
    let mut c = d.client().await;
    std::fs::remove_dir_all(&gone).unwrap();
    let list = c.call("sites.list", Value::Null).await.unwrap();
    let find = |n: &str| list.as_array().unwrap().iter().find(|s| s["name"] == n).cloned().unwrap();
    assert!(find("gone").get("node").is_none(), "{list}");
    assert_eq!(find("kept")["node"]["spec"], "20", "{list}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unusable_nvmrc_is_the_sites_node_error() {
    let d = prepared("").await;
    let dir = d.project("maker");
    write(&dir, ".nvmrc", "latest\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let node = site(&mut c, "maker").await["node"].clone();
    assert_eq!(node["error"]["code"], "INVALID_ARGUMENT", "{node}");
    assert!(node["spec"].is_null(), "{node}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn use_writes_the_managed_file_or_sites_toml_and_unuse_removes_it() {
    let d = prepared("").await;
    let managed_dir = d.project("managed");
    let plain_dir = d.project("plain");
    write(&plain_dir, "package.json", "{}");
    let committed = d.project("committed");
    write(&committed, ".yard", "docroot = \".\"\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let file = d.fx.paths.managed_project_file("managed");
    c.call("sites.init", json!({"path": managed_dir.display().to_string()})).await.unwrap();
    // A node in sites.toml is not read while the managed file serves the site.
    let mut toml = yard_core::config::TomlFile::load(&d.fx.paths.sites_toml()).unwrap();
    toml.set(&["sites", "managed", "node"], toml_edit::value("18"));
    toml.save().unwrap();

    let s = c.call("node.use", json!({"site": "managed", "spec": "22"})).await.unwrap();
    assert_eq!(s["node"]["spec"], "22", "{s}");
    assert_eq!(s["node"]["source"], "managed_file", "{s}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("node = \"22\"") && text.starts_with("# "), "{text}");
    assert_eq!(info(&mut c, "managed").await["node"]["source"], "managed_file");

    let s = c.call("node.use", json!({"site": "plain", "spec": "22"})).await.unwrap();
    assert_eq!(s["node"]["source"], "sites_toml", "{s}");
    let sites = std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap();
    assert!(sites.contains("node = \"22\""), "{sites}");

    c.call("node.unuse", json!({"site": "managed"})).await.unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains("node ="));
    let s = c.call("node.unuse", json!({"site": "plain"})).await.unwrap();
    assert_eq!(s["node"]["source"], "default", "{s}");
    let sites: toml_edit::DocumentMut =
        std::fs::read_to_string(d.fx.paths.sites_toml()).unwrap().parse().unwrap();
    assert!(sites["sites"]["plain"].get("node").is_none(), "{sites}");

    let e = c.call("node.use", json!({"site": "committed", "spec": "22"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::SiteHasProjectFile);
    assert!(e.hint.as_deref().unwrap_or_default().contains("node = \"22\""), "{e:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn default_needs_an_installed_match() {
    let d = prepared("").await;
    d.fx.add_node("16.14.0");
    d.fx.add_node("22.18.0");
    let d = d.start().await;
    let mut c = d.client().await;
    let e = c.call("node.default", json!({"spec": "24"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::NodeMissing);
    assert_eq!(e.hint.as_deref(), Some("yard node install 24"));
    c.call("node.default", json!({"spec": "22"})).await.unwrap();
    let yard = std::fs::read_to_string(d.fx.paths.yard_toml()).unwrap();
    assert!(yard.contains("default_node = \"22\""), "{yard}");
    let e = c.call("node.default", json!({"spec": "latest"})).await.unwrap_err();
    assert_eq!(e.code, yard_proto::ErrorCode::InvalidArgument);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn list_shows_versions_default_and_sites() {
    let d = prepared("").await;
    d.fx.add_node("16.14.0");
    d.fx.add_node("22.18.0");
    let dir = d.project("maker");
    write(&dir, ".nvmrc", "16.14.0\n");
    let d = d.start().await;
    let mut c = d.client().await;
    let list = c.call("node.list", Value::Null).await.unwrap();
    assert_eq!(list["default"], "22", "{list}");
    let versions: Vec<&str> =
        list["versions"].as_array().unwrap().iter().map(|v| v["version"].as_str().unwrap()).collect();
    assert_eq!(versions, ["22.18.0", "16.14.0"]);
    assert_eq!(list["versions"][0]["default"], true);
    assert_eq!(list["versions"][1]["default"], false);
    assert_eq!(list["versions"][1]["sites"], json!(["maker"]));
    assert!(list.get("available").is_none(), "{list}");
    d.shutdown().await.unwrap();
}
