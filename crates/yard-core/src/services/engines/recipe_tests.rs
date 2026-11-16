//! Recipes: parsing, the load checks (each with its line), and expansion (recipes spec §6).

use super::recipe::{LoadOptions, RecipeEngine, parse_recipe};
use super::*;
use crate::Paths;
use crate::services::render::{SIGINT, SIGTERM};
use std::path::Path;
use yard_proto::PortRef;

fn load(name: &str) -> RecipeEngine {
    let text = match name {
        "brewed" => include_str!("testdata/brewed.toml"),
        "pathed" => include_str!("testdata/pathed.toml"),
        "downloaded" => include_str!("testdata/downloaded.toml"),
        "npmed" => include_str!("testdata/npmed.toml"),
        _ => unreachable!(),
    };
    let file = PathBuf::from(format!("/r/{name}.toml"));
    parse_recipe(text, &file, Origin::User(file.clone()), &LoadOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

fn port(name: &str, port: u16) -> PortRef {
    PortRef { name: name.into(), port }
}

fn strs(v: &[std::ffi::OsString]) -> Vec<String> {
    v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
}

#[test]
fn each_source_parses() {
    assert_eq!(load("brewed").source(), SourceSpec::Brew { formula: "acme/tap/brewed".into() });
    assert_eq!(load("pathed").source(), SourceSpec::Path { program: "/usr/local/bin/pathed".into() });
    assert_eq!(
        load("downloaded").source(),
        SourceSpec::Download {
            version: "30.1".into(),
            url: "https://example.com/downloaded-30.1.tar.gz".into(),
            sha256: "a".repeat(64),
            strip: 1,
        }
    );
    assert_eq!(
        load("npmed").source(),
        SourceSpec::Npm { package: "@acme/npmed".into(), version: "1.6.1".into(), node: "18".into() }
    );
    assert_eq!(load("downloaded").version(), Some("30.1"));
    assert_eq!(load("npmed").version(), Some("1.6.1"));
    assert_eq!(load("brewed").version(), None);
    let b = load("brewed");
    assert_eq!(b.key(), "brewed");
    assert_eq!(b.display(), "Brewed");
    assert_eq!(b.description(), "A brew engine with every field");
    assert_eq!(b.origin(), Origin::User("/r/brewed.toml".into()));
    assert_eq!(b.lines(), None);
}

#[test]
fn brewed_expands_every_field() {
    let b = load("brewed");
    let paths = Paths::new("/u/.yard");
    let ports = [port("brewed", 27700), port("brewed-console", 27701)];
    let ctx = InstanceCtx {
        instance: "brewed",
        home: Path::new("/opt/homebrew/opt/brewed"),
        paths: &paths,
        ports: &ports,
        macos_user: "me",
        node: None,
    };
    let cmd = b.command(&ctx);
    assert_eq!(cmd.program, PathBuf::from("/opt/homebrew/opt/brewed/bin/brewed"));
    assert_eq!(
        strs(&cmd.args),
        [
            "--listen",
            "127.0.0.1:27700",
            "--data",
            "/u/.yard/services/brewed/data",
            "--conf",
            "/u/.yard/services/brewed/brewed.conf"
        ]
    );
    assert_eq!(cmd.env, [("BREWED_HOME".to_string(), "/u/.yard/services/brewed".to_string())]);
    assert_eq!(cmd.stop_signal, SIGINT);
    let init = b.init(&ctx).unwrap();
    assert_eq!(init.program, PathBuf::from("/opt/homebrew/opt/brewed/bin/brewed"));
    assert_eq!(strs(&init.args), ["init", "/u/.yard/services/brewed/data"]);
    let (file, text) = b.config_file(&ctx).unwrap();
    assert_eq!(file, PathBuf::from("/u/.yard/services/brewed/brewed.conf"));
    assert_eq!(
        text,
        "port 27700\nconsole 27701\ndir /u/.yard/services/brewed/data\njson {\"a\": 1}\n\
         include /u/.yard/services/brewed/overrides.conf\n"
    );
    assert_eq!(b.user_overrides().as_deref(), Some("# yours\n"));
    assert_eq!(b.start_timeout(), Some(std::time::Duration::from_secs(45)));
    assert!(!b.may_sleep());
    assert_eq!(b.dirs(), ["data", "logs"]);
    assert_eq!(b.health_port(), "");
    assert_eq!(
        b.console(),
        Some(Console { port: "console".into(), host_header: HostHeader::Upstream, nginx_overrides: false })
    );
    assert_eq!(b.env_lines(&ctx), ["BREWED_URL=http://127.0.0.1:27700"]);
    assert_eq!(
        b.account("me"),
        yard_proto::Account { user: Some("admin".into()), password: Some("secret".into()) }
    );
    let c = b.connect(&ctx).unwrap();
    assert_eq!(c.url, "brewed://127.0.0.1:27700");
    assert_eq!(strs(c.shell.as_deref().unwrap()), ["/opt/homebrew/opt/brewed/bin/brewed-cli", "-p", "27700"]);
    assert_eq!(c.clients, ["Any client: the URL above"]);
    assert_eq!(c.socket, None);
    assert_eq!(b.databases(), None);
}

#[test]
fn defaults_for_a_lean_recipe() {
    let p = load("pathed");
    let paths = Paths::new("/u/.yard");
    let ports = [port("pathed", 27800)];
    let ctx = InstanceCtx {
        instance: "pathed",
        home: Path::new("/usr/local/bin"),
        paths: &paths,
        ports: &ports,
        macos_user: "me",
        node: None,
    };
    let cmd = p.command(&ctx);
    assert_eq!(cmd.program, PathBuf::from("/usr/local/bin/pathed"));
    assert_eq!(strs(&cmd.args), ["--port", "27800"]);
    assert_eq!(cmd.stop_signal, SIGTERM);
    assert_eq!(p.init(&ctx), None);
    assert_eq!(p.config_file(&ctx), None);
    assert_eq!(p.user_overrides(), None);
    assert_eq!(p.start_timeout(), None);
    assert!(p.may_sleep());
    assert!(p.dirs().is_empty());
    assert_eq!(p.console(), None);
    assert!(p.env_lines(&ctx).is_empty());
    assert_eq!(p.account("me"), yard_proto::Account { user: None, password: None });
    assert_eq!(p.connect(&ctx), None);
}

#[test]
fn npm_runs_on_yards_node() {
    let n = load("npmed");
    let paths = Paths::new("/u/.yard");
    let ports = [port("npmed", 26001)];
    let ctx = InstanceCtx {
        instance: "npmed",
        home: Path::new("/u/.yard/programs/npmed/1.6.1"),
        paths: &paths,
        ports: &ports,
        macos_user: "me",
        node: Some(Path::new("/u/.yard/node/node-versions/v18.20.0/installation/bin/node")),
    };
    let cmd = n.command(&ctx);
    assert_eq!(cmd.program, PathBuf::from("/u/.yard/node/node-versions/v18.20.0/installation/bin/node"));
    assert_eq!(
        strs(&cmd.args),
        ["/u/.yard/programs/npmed/1.6.1/node_modules/@acme/npmed/bin/server.js", "--port", "26001"]
    );
}

#[test]
fn ports_name_registry_entries() {
    assert_eq!(
        load("brewed").ports(),
        [PortSpec { suffix: "".into(), usual: 7700 }, PortSpec { suffix: "console".into(), usual: 7701 }]
    );
}

#[test]
fn placeholder_values_are_never_split() {
    let p = load("downloaded");
    let paths = Paths::new("/tmp/a b/\"q\" {port.main}");
    let ports = [port("downloaded", 28108)];
    let ctx = InstanceCtx {
        instance: "downloaded",
        home: Path::new("/tmp/My Tools"),
        paths: &paths,
        ports: &ports,
        macos_user: "me",
        node: None,
    };
    let cmd = p.command(&ctx);
    assert_eq!(cmd.program, PathBuf::from("/tmp/My Tools/downloaded-server"));
    assert_eq!(
        strs(&cmd.args),
        ["--data-dir", "/tmp/a b/\"q\" {port.main}/services/downloaded/data", "--api-port", "28108"]
    );
}

const BASE: &str = r#"schema = 1
key = "t"
name = "T"

[source]
kind = "brew"
formula = "t"

[ports]
main = 7000

[run]
command = ["{home}/bin/t", "{port.main}"]

[health]
tcp = "main"
"#;

/// The 1-based line of the first line containing `needle`.
fn line_of(text: &str, needle: &str) -> usize {
    text.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle:?} not in text")) + 1
}

#[test]
fn base_recipe_loads() {
    parse_recipe(BASE, Path::new("/r/t.toml"), Origin::Carried, &LoadOptions::default()).unwrap();
}

#[test]
fn broken_recipes_report_file_and_line() {
    let swap = |from: &str, to: &str| {
        assert!(BASE.contains(from), "{from}");
        BASE.replacen(from, to, 1)
    };
    let source = |body: &str| swap("kind = \"brew\"\nformula = \"t\"", body);
    let sha = "a".repeat(64);
    // (name, file stem, text, needle for the expected line, message fragment)
    let cases: Vec<(&str, &str, String, &str, &str)> = vec![
        ("syntax", "t", swap("name = \"T\"", "name = \"T"), "name = \"T", ""),
        ("unknown field", "t", swap("name = \"T\"", "name = \"T\"\ncolour = \"red\""), "colour", "colour"),
        ("schema", "t", swap("schema = 1", "schema = 2"), "schema", "schema 2"),
        ("stem", "other", BASE.to_string(), "key = ", "other.toml"),
        ("built-in key", "mysql", swap("key = \"t\"", "key = \"mysql\""), "key = ", "reserved"),
        ("yard command", "link", swap("key = \"t\"", "key = \"link\""), "key = ", "Yard command"),
        ("bad key", "T", swap("key = \"t\"", "key = \"T\""), "key = ", "engine key"),
        ("unknown placeholder", "t", swap("{port.main}\"]", "{bogus}\"]"), "command", "{bogus}"),
        ("unknown port", "t", swap("{port.main}\"]", "{port.api}\"]"), "command", "{port.api}"),
        ("program in brew", "t", swap("{port.main}\"]", "{program}\"]"), "command", "{program}"),
        ("node outside npm", "t", swap("{port.main}\"]", "{node}\"]"), "command", "{node}"),
        ("command not under home", "t", swap("\"{home}/bin/t\"", "\"/bin/sh\""), "command", "{home}"),
        (
            "empty command",
            "t",
            swap("command = [\"{home}/bin/t\", \"{port.main}\"]", "command = []"),
            "command",
            "empty",
        ),
        ("unclosed brace", "t", swap("{port.main}\"]", "{port.main\"]"), "command", "{"),
        ("zero ports", "t", swap("main = 7000\n", ""), "[ports]", "main"),
        (
            "nine ports",
            "t",
            swap(
                "main = 7000",
                "main = 7000\na = 7001\nb = 7002\nc = 7003\nd = 7004\ne = 7005\nf = 7006\ng = 7007\nh = 7008",
            ),
            "[ports]",
            "at most 8",
        ),
        ("no main port", "t", swap("main = 7000", "api = 7000"), "[ports]", "main"),
        ("low port", "t", swap("main = 7000", "main = 80"), "main = 80", "1024"),
        ("duplicate port", "t", swap("main = 7000", "main = 7000\nmain = 7001"), "main = 7001", ""),
        ("health names no port", "t", swap("tcp = \"main\"", "tcp = \"api\""), "tcp = ", "api"),
        (
            "console names no port",
            "t",
            format!("{BASE}\n[console]\nport = \"admin\"\n"),
            "port = \"admin\"",
            "admin",
        ),
        (
            "http download",
            "t",
            source(&format!(
                "kind = \"download\"\nversion = \"1\"\nurl = \"http://x/y.tgz\"\nsha256 = \"{sha}\""
            )),
            "[source]",
            "https://",
        ),
        (
            "file download",
            "t",
            source(&format!(
                "kind = \"download\"\nversion = \"1\"\nurl = \"file:///x/y.tgz\"\nsha256 = \"{sha}\""
            )),
            "[source]",
            "https://",
        ),
        (
            "short sha",
            "t",
            source("kind = \"download\"\nversion = \"1\"\nurl = \"https://x/y.tgz\"\nsha256 = \"abc\""),
            "[source]",
            "sha256",
        ),
        (
            "npm range",
            "t",
            source("kind = \"npm\"\npackage = \"x\"\nversion = \"^1.2\"\nnode = \"22\""),
            "[source]",
            "exact",
        ),
        (
            "relative path program",
            "t",
            source("kind = \"path\"\nprogram = \"bin/t\""),
            "[source]",
            "absolute",
        ),
        ("unknown source", "t", source("kind = \"cargo\""), "[source]", ""),
        (
            "stop signal",
            "t",
            swap("[health]", "[health]").replace("[run]\n", "[run]\nstop_signal = \"KILL\"\n"),
            "stop_signal",
            "KILL",
        ),
        (
            "start timeout",
            "t",
            BASE.replace("[run]\n", "[run]\nstart_timeout = \"forever\"\n"),
            "start_timeout",
            "forever",
        ),
        (
            "include outside config",
            "t",
            swap("{port.main}\"]", "{include_overrides}\"]"),
            "command",
            "{include_overrides}",
        ),
        ("conf without config", "t", swap("{port.main}\"]", "{conf}\"]"), "command", "{conf}"),
    ];
    for (name, stem, text, needle, fragment) in cases {
        let file = PathBuf::from(format!("/r/{stem}.toml"));
        let e = match parse_recipe(&text, &file, Origin::User(file.clone()), &LoadOptions::default()) {
            Ok(_) => panic!("{name}: loaded"),
            Err(e) => e,
        };
        assert_eq!(e.file, file, "{name}");
        assert_eq!(e.line, Some(line_of(&text, needle)), "{name}: {}", e.message);
        assert!(e.message.contains(fragment), "{name}: {:?} lacks {fragment:?}", e.message);
        let y = e.to_yard_error();
        assert_eq!(y.code, yard_proto::ErrorCode::EngineRecipeInvalid, "{name}");
        assert!(
            y.message.starts_with(&format!("/r/{stem}.toml:{}: ", line_of(&text, needle))),
            "{name}: {}",
            y.message
        );
        assert_eq!(y.hint.as_deref(), Some(format!("yard engines check /r/{stem}.toml").as_str()), "{name}");
    }
}

#[test]
fn file_urls_load_when_allowed() {
    let sha = "a".repeat(64);
    let text = BASE
        .replacen(
            "kind = \"brew\"\nformula = \"t\"",
            &format!("kind = \"download\"\nversion = \"1\"\nurl = \"file:///x/y.tgz\"\nsha256 = \"{sha}\""),
            1,
        )
        .replace("{home}/bin/t", "{home}/t");
    let opts = LoadOptions { allow_file_urls: true };
    parse_recipe(&text, Path::new("/r/t.toml"), Origin::Carried, &opts).unwrap();
}

#[test]
fn yard_commands_are_reserved_keys() {
    for name in ["link", "db", "pgsql", "engines", "service", "help"] {
        assert!(YARD_COMMANDS.contains(&name), "{name}");
    }
}
