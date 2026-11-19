use super::recipe::{RecipeError, RuntimeRecipe, parse_runtime_recipe};
use super::*;
use crate::services::engines::LoadOptions;
use crate::shim::{NODE_SHIMS, PHP_SHIMS, SHIMS, WAKE_SHIMS};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const HASH: &str = "241a3d13b7c6d35d970b1f69a000298b66943541302148a438a932f5dd44a9fc";

fn bun() -> String {
    format!(
        r#"schema = 1
key = "bun"
name = "Bun"
pin_files = [".bun-version"]
uses_when = ["bun.lock"]

[shims.bun]
exec = ["{{home}}/bin/bun"]

[source]
kind = "download"
url = "https://example.com/bun-{{version}}.zip"
strip = 1

[versions."1.2.4"]
sha256 = "{HASH}"
"#
    )
}

fn user(text: &str) -> Result<RuntimeRecipe, RecipeError> {
    user_named(text, "bun.toml")
}

fn user_named(text: &str, file: &str) -> Result<RuntimeRecipe, RecipeError> {
    let file = PathBuf::from("/Users/me/.yard/runtimes").join(file);
    parse_runtime_recipe(text, &file, Origin::User(file.clone()), &LoadOptions::default())
}

fn err(text: &str) -> String {
    user(text).unwrap_err().message
}

#[test]
fn every_carried_recipe_loads() {
    for (key, text) in carried::texts() {
        parse_runtime_recipe(
            text,
            Path::new(&format!("{key}.toml")),
            Origin::Carried,
            &LoadOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{key}: {e:?}"));
    }
    let all = Runtimes::carried();
    let keys: Vec<&str> = all.iter().map(|r| r.key()).collect();
    assert_eq!(keys, ["composer", "node", "php"]);
    assert_eq!(all.get("php").unwrap().kind(), RuntimeKind::Php);
    assert_eq!(all.get("node").unwrap().kind(), RuntimeKind::Node);
    assert_eq!(all.get("composer").unwrap().kind(), RuntimeKind::Recipe);
    assert_eq!(all.by_shim("npx").unwrap().key(), "node");
    assert_eq!(all.by_shim("composer").unwrap().key(), "composer");
    assert!(all.by_shim("bun").is_none());
    assert_eq!(all.get("node").unwrap().pin_key(), PinKey::TopLevel("node"));
    assert_eq!(all.get("composer").unwrap().pin_key(), PinKey::Runtimes("composer".into()));
}

#[test]
fn carried_shims_match_the_shim_constants() {
    let all = Runtimes::carried();
    let union: BTreeSet<String> = all.iter().flat_map(|r| r.shims()).collect();
    let want: BTreeSet<String> = SHIMS.iter().map(|s| s.to_string()).collect();
    assert_eq!(union, want);
    let php: BTreeSet<String> = all.get("php").unwrap().shims().into_iter().collect();
    let want: BTreeSet<String> =
        PHP_SHIMS.iter().filter(|s| **s != "composer").map(|s| s.to_string()).collect();
    assert_eq!(php, want);
    let node: BTreeSet<String> = all.get("node").unwrap().shims().into_iter().collect();
    assert_eq!(node, NODE_SHIMS.iter().map(|s| s.to_string()).collect());
    let waking: BTreeSet<String> =
        all.iter().flat_map(|r| r.shims().into_iter().filter(|s| r.wakes(s)).collect::<Vec<_>>()).collect();
    assert_eq!(waking, WAKE_SHIMS.iter().map(|s| s.to_string()).collect());
}

#[test]
fn a_user_recipe_cannot_name_a_driver() {
    let text = bun().replace("name = \"Bun\"", "name = \"Bun\"\ndriver = \"php\"");
    assert!(err(&text).contains("driver"), "{}", err(&text));
}

#[test]
fn a_user_recipe_cannot_have_an_index() {
    let (_, text) = carried::texts().into_iter().find(|(k, _)| *k == "composer").unwrap();
    let e = user_named(text, "composer.toml").unwrap_err().message;
    assert!(e.contains("index"), "{e}");
}

#[test]
fn fnm_and_brew_sources_are_carried_only() {
    for kind in ["fnm", "brew"] {
        let text = bun().replace(
            "kind = \"download\"\nurl = \"https://example.com/bun-{version}.zip\"\nstrip = 1",
            &format!("kind = \"{kind}\""),
        );
        let text = text.split("[versions").next().unwrap().to_string();
        assert!(err(&text).contains(kind), "{}", err(&text));
    }
}

#[test]
fn unknown_fields_fail_with_their_line() {
    let text = bun().replace("name = \"Bun\"\n", "name = \"Bun\"\ncolour = \"red\"\n");
    let e = user(&text).unwrap_err();
    assert_eq!(e.line, Some(4), "{e:?}");
    assert!(e.message.contains("colour"), "{e:?}");
}

#[test]
fn a_bad_placeholder_fails_at_load() {
    let e = user(&bun().replace("{home}/bin/bun", "{hoem}/bin/bun")).unwrap_err();
    assert!(e.message.contains("{hoem}"), "{e:?}");
    assert_eq!(e.line, Some(8));
    // {php} has no value without runs_on = "php".
    assert!(err(&bun().replace("[\"{home}/bin/bun\"]", "[\"{php}\", \"{home}/x\"]")).contains("{php}"));
}

#[test]
fn exec_must_start_under_home_or_at_php() {
    let text = bun().replace("[\"{home}/bin/bun\"]", "[\"/bin/sh\", \"-c\", \"x\"]");
    assert!(err(&text).contains("exec"), "{}", err(&text));
    let text = bun().replace("[\"{home}/bin/bun\"]", "[\"{home}/../../bin/sh\"]");
    assert!(user(&text).is_err());
    let ok = bun()
        .replace("name = \"Bun\"", "name = \"Bun\"\nruns_on = \"php\"")
        .replace("[\"{home}/bin/bun\"]", "[\"{php}\", \"{home}/bun.phar\"]");
    user(&ok).unwrap();
}

#[test]
fn php_below_and_requires_need_runs_on() {
    let rule =
        bun().replace("[shims.bun]", "[[default]]\nspec = \"1\"\nphp_below = \"7.2.5\"\n\n[shims.bun]");
    assert!(err(&rule).contains("runs_on"), "{}", err(&rule));
    let req = bun().replace("[shims.bun]", "[[requires]]\nfrom = \"1.1.0\"\nphp = \"7.2.5\"\n\n[shims.bun]");
    assert!(err(&req).contains("runs_on"), "{}", err(&req));
    let both = req.replace("name = \"Bun\"", "name = \"Bun\"\nruns_on = \"php\"");
    let r = user(&both).unwrap();
    assert_eq!(r.requires[0].php.to_string(), "7.2.5");
    let plain = bun().replace("[shims.bun]", "[[default]]\nspec = \"1\"\n\n[shims.bun]");
    assert_eq!(user(&plain).unwrap().defaults[0].spec.to_string(), "1");
}

#[test]
fn a_versions_entry_needs_a_hash_or_a_path() {
    assert!(err(&bun().replace(HASH, "abc")).contains("sha256"));
    user(&bun()).unwrap();
    let path = bun().replace(&format!("sha256 = \"{HASH}\""), "path = \"relative\"");
    assert!(err(&path).contains("absolute"), "{}", err(&path));
    let path = bun().replace(&format!("sha256 = \"{HASH}\""), "path = \"/opt/bun/1.2.4\"");
    assert!(matches!(user(&path).unwrap().versions.values().next(), Some(recipe::VersionEntry::Path(_))));
    let neither = bun().replace(&format!("sha256 = \"{HASH}\""), "");
    assert!(user(&neither).is_err());
    let bad_key = bun().replace("[versions.\"1.2.4\"]", "[versions.\"latest\"]");
    assert!(user(&bad_key).is_err());
}

#[test]
fn http_urls_are_refused_and_file_urls_only_when_allowed() {
    let http = bun().replace("https://example.com", "http://example.com");
    assert!(err(&http).contains("https://"), "{}", err(&http));
    let file = bun().replace("https://example.com", "file:///tmp/fixtures");
    assert!(user(&file).is_err());
    let path = PathBuf::from("bun.toml");
    parse_runtime_recipe(&file, &path, Origin::User(path.clone()), &LoadOptions { allow_file_urls: true })
        .unwrap();
}

#[test]
fn the_stem_must_be_the_key() {
    let e = user_named(&bun(), "bunx.toml").unwrap_err().message;
    assert!(e.contains("bun.toml"), "{e}");
}

#[test]
fn php_and_node_keys_are_reserved_for_user_recipes() {
    for key in ["php", "node"] {
        let text = bun().replace("key = \"bun\"", &format!("key = \"{key}\""));
        let e = user_named(&text, &format!("{key}.toml")).unwrap_err().message;
        assert!(e.contains(key) && e.contains("reserved"), "{e}");
    }
}

#[test]
fn shim_names_are_checked() {
    let bad = bun().replace("[shims.bun]", "[shims.\"Bad Name\"]");
    assert!(err(&bad).contains("Bad Name"), "{}", err(&bad));
    for name in ["yard", "yardd", "yard-shim", "npm", "composer"] {
        let text = bun().replace("[shims.bun]", &format!("[shims.{name}]"));
        assert!(user(&text).is_err(), "{name}");
    }
}

#[test]
fn a_shims_wake_overrides_the_recipes() {
    let php = Runtimes::carried();
    let php = php.get("php").unwrap();
    assert!(php.wakes("php"));
    assert!(!php.wakes("pecl"));
}
