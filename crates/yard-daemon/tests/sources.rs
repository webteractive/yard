//! Brew sources and the trust gate (spec M4b §8): yardd asks brew, never a file of its
//! own, whether a formula may be installed.

mod support;

use serde_json::{Value, json};
use support::daemon::{Daemon, prepared, run_job};
use yard_proto::ErrorCode;

fn brew_log(d: &Daemon) -> String {
    std::fs::read_to_string(d.fx.brew.path().join("fake-brew.log")).unwrap_or_default()
}

fn trust_file(d: &Daemon) -> Value {
    let text = std::fs::read_to_string(d.fx.brew.path().join("fake-trust.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn set_trust(d: &Daemon, taps: &[&str], formulae: &[&str]) {
    let doc = json!({"taps": taps, "formulae": formulae, "casks": [], "commands": []});
    std::fs::write(d.fx.brew.path().join("fake-trust.json"), doc.to_string()).unwrap();
}

/// The extensions tap as brew lays it out in the fixture's prefix, so `ext add` can tell
/// from its files whether brew lists a formula. Returns the tap's directory.
fn ext_tap(d: &Daemon) -> std::path::PathBuf {
    let tap = d.fx.brew.path().join("Library/Taps/shivammathur/homebrew-extensions");
    std::fs::create_dir_all(tap.join("Formula")).unwrap();
    tap
}

fn contains(list: &Value, wanted: &str) -> bool {
    list.as_array().unwrap().iter().any(|v| v == wanted)
}

#[tokio::test]
async fn php_install_refuses_untrusted_formula() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    let mut c = d.client().await;
    let err = c.call("php.install", json!({"php": "7.1"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    assert_eq!(
        err.message,
        "shivammathur/php/php@7.1 comes from the shivammathur/php tap, which brew does not trust yet"
    );
    assert_eq!(err.hint.as_deref(), Some("yard source trust shivammathur/php/php@7.1"));
    let data = err.data.unwrap();
    assert_eq!(data["formula"], "shivammathur/php/php@7.1");
    assert_eq!(data["tap"], "shivammathur/php");
    // No job ran: brew was only asked for its version and its trust.
    let log = brew_log(&d);
    assert!(log.lines().all(|l| l == "--version" || l == "trust --json=v1"), "{log}");
    assert!(log.contains("trust --json=v1"), "{log}");
    assert!(!d.fx.brew.path().join("opt/php@7.1").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn php_install_proceeds_when_tap_trusted() {
    let d = prepared("").await.start().await;
    set_trust(&d, &["shivammathur/php"], &[]);
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(d.fx.brew.path().join("opt/php@7.1/bin/php").exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn php_install_proceeds_when_the_formula_itself_is_trusted() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &["shivammathur/php/php@7.1"]);
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error, None, "{lines:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn old_brew_skips_the_gate() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    std::fs::write(d.fx.brew.path().join("fake-brew-version"), "5.1.0\n").unwrap();
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error, None, "{lines:?}");
    // Brew 5 has no `brew trust`: Yard never asks it.
    assert!(!brew_log(&d).contains("trust"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn unparsable_brew_version_keeps_the_gate() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    // `Homebrew garbage`: no major version to read, so the gate fails closed.
    std::fs::write(d.fx.brew.path().join("fake-brew-version"), "garbage\n").unwrap();
    let mut c = d.client().await;
    let err = c.call("php.install", json!({"php": "7.1"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    assert!(brew_log(&d).contains("trust --json=v1"), "{}", brew_log(&d));
    assert!(!brew_log(&d).contains("install"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn installing_an_installed_php_asks_brew_nothing() {
    let d = prepared("").await.start().await;
    // The fixture has php@8.4; an empty trust store would refuse it were it not there.
    set_trust(&d, &[], &[]);
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(!brew_log(&d).contains("trust"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_refuses_untrusted_formula() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "redis\n").unwrap();
    let mut c = d.client().await;
    let err = c.call("ext.add", json!({"name": "redis", "php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    assert_eq!(err.hint.as_deref(), Some("yard source trust shivammathur/extensions/redis@8.4"));
    assert_eq!(err.data.unwrap()["tap"], "shivammathur/extensions");
    assert!(!brew_log(&d).contains("install"), "{}", brew_log(&d));
    assert!(!d.fx.paths.yard_toml().exists());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_of_a_pecl_only_extension_is_not_gated() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    // The tap is there and none of its files name mongodb: brew has no formula for it, so
    // the job builds it with pecl and nothing is installed from the untrusted tap.
    let tap = ext_tap(&d);
    std::fs::write(tap.join("Formula/phpredis@8.4.rb"), "").unwrap();
    std::fs::write(tap.join("formula_renames.json"), r#"{"redis@8.4": "phpredis@8.4"}"#).unwrap();
    std::fs::create_dir_all(tap.join("Aliases")).unwrap();
    std::fs::write(tap.join("Aliases/pecl_redis@8.4"), "").unwrap();
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "ext.add", json!({"name": "mongodb", "php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("fake pecl") && l.contains("install mongodb")), "{lines:?}");
    assert!(!brew_log(&d).contains("install"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

/// `ext add` of `name` on PHP 8.4 with the extensions tap untrusted, after `lay` has put
/// files in the tap.
async fn untrusted_ext_add(name: &str, lay: impl FnOnce(&std::path::Path)) -> yard_proto::YardError {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    lay(&ext_tap(&d));
    let mut c = d.client().await;
    let err = c.call("ext.add", json!({"name": name, "php": "8.4"})).await.unwrap_err();
    assert!(!brew_log(&d).contains("install"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
    err
}

#[tokio::test]
async fn ext_add_is_gated_when_the_tap_lists_the_formula_as_a_rename() {
    // phpredis@8.4 is the real formula; `redis@8.4` is a rename brew resolves to it.
    let err = untrusted_ext_add("redis", |tap| {
        std::fs::write(tap.join("Formula/phpredis@8.4.rb"), "").unwrap();
        std::fs::write(tap.join("formula_renames.json"), r#"{"redis@8.4": "phpredis@8.4"}"#).unwrap();
    })
    .await;
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    assert_eq!(err.hint.as_deref(), Some("yard source trust shivammathur/extensions/redis@8.4"));
}

#[tokio::test]
async fn ext_add_is_gated_when_the_name_differs_only_in_case() {
    // brew lowercases the name before it looks, so `Redis` is the `redis@8.4` rename.
    let err = untrusted_ext_add("Redis", |tap| {
        std::fs::write(tap.join("Formula/phpredis@8.4.rb"), "").unwrap();
        std::fs::write(tap.join("formula_renames.json"), r#"{"redis@8.4": "phpredis@8.4"}"#).unwrap();
    })
    .await;
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
}

/// Puts a directory's mode back when it drops, so the temp prefix can be removed.
struct Restore(std::path::PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[tokio::test]
async fn ext_add_is_gated_when_the_tap_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    let formulae = ext_tap(&d).join("Formula");
    let _restore = Restore(formulae.clone());
    std::fs::set_permissions(&formulae, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Not a user who can read it anyway (root): the test would prove nothing.
    if std::fs::read_dir(&formulae).is_ok() {
        d.shutdown().await.unwrap();
        return;
    }
    let mut c = d.client().await;
    let err = c.call("ext.add", json!({"name": "mongodb", "php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_is_gated_when_the_renames_file_is_oversized_or_not_text() {
    // Valid JSON that does not name mongodb, over the 1 MiB Yard reads.
    let big = format!(r#"{{"a": "{}"}}"#, "x".repeat((1 << 20) + 16));
    let err =
        untrusted_ext_add("mongodb", |tap| std::fs::write(tap.join("formula_renames.json"), &big).unwrap())
            .await;
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    let err = untrusted_ext_add("mongodb", |tap| {
        std::fs::write(tap.join("formula_renames.json"), [0xff, 0xfe, b'{', b'}']).unwrap();
    })
    .await;
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
}

#[tokio::test]
async fn ext_add_is_gated_when_the_tap_lists_the_formula_as_an_alias() {
    let err = untrusted_ext_add("redis", |tap| {
        std::fs::create_dir_all(tap.join("Aliases")).unwrap();
        std::fs::write(tap.join("Aliases/redis@8.4"), "").unwrap();
    })
    .await;
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
}

#[tokio::test]
async fn ext_add_is_gated_when_the_tap_has_the_formula_file() {
    for file in ["Formula/redis@8.4.rb", "Formula/r/redis@8.4.rb", "redis@8.4.rb"] {
        let err = untrusted_ext_add("redis", |tap| {
            let path = tap.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        })
        .await;
        assert_eq!(err.code, ErrorCode::FormulaUntrusted, "{file}");
    }
}

#[tokio::test]
async fn ext_add_is_gated_when_the_tap_is_not_there_to_ask() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    assert!(!d.fx.brew.path().join("Library/Taps/shivammathur").exists());
    let mut c = d.client().await;
    let err = c.call("ext.add", json!({"name": "mongodb", "php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_is_gated_when_the_renames_file_cannot_be_read() {
    let err = untrusted_ext_add("mongodb", |tap| {
        std::fs::write(tap.join("formula_renames.json"), "{not json").unwrap();
    })
    .await;
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
}

#[tokio::test]
async fn ext_add_gates_the_formula_brew_has_and_lets_a_trusted_one_through() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &["shivammathur/extensions/xdebug@8.4"]);
    std::fs::write(d.fx.brew.path().join("fake-ext-formulas"), "redis\nxdebug\n").unwrap();
    let mut c = d.client().await;
    let err = c.call("ext.add", json!({"name": "redis", "php": "8.4"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::FormulaUntrusted);
    // A formula trusted by name goes through.
    let (lines, error) = run_job(&mut c, "ext.add", json!({"name": "xdebug", "php": "8.4"})).await;
    assert_eq!(error, None, "{lines:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn ext_add_of_a_file_needs_no_trust() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("thing.so");
    std::fs::write(&file, "not really a library").unwrap();
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "ext.add", json!({"name": file.display().to_string()})).await;
    assert_ne!(error.map(|e| e.code), Some(ErrorCode::FormulaUntrusted));
    assert!(!brew_log(&d).contains("trust"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_trust_runs_brew_trust() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    let mut c = d.client().await;
    let target = "shivammathur/php/php@7.1";
    let (lines, error) = run_job(&mut c, "sources.trust", json!({"target": target})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(brew_log(&d).contains(&format!("trust --formula {target}")), "{}", brew_log(&d));
    assert!(contains(&trust_file(&d)["formulae"], target), "{}", trust_file(&d));
    // Trusted now, the install goes through.
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error, None, "{lines:?}");
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_trust_tap() {
    let d = prepared("").await.start().await;
    set_trust(&d, &[], &[]);
    let mut c = d.client().await;
    let (lines, error) =
        run_job(&mut c, "sources.trust", json!({"target": "minio/stable", "tap": true})).await;
    assert_eq!(error, None, "{lines:?}");
    assert!(brew_log(&d).contains("trust --tap minio/stable"), "{}", brew_log(&d));
    assert!(contains(&trust_file(&d)["taps"], "minio/stable"));
    assert_eq!(trust_file(&d)["formulae"], json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_untrust_runs_brew_untrust() {
    let d = prepared("").await.start().await;
    set_trust(&d, &["minio/stable"], &["shivammathur/php/php@7.1"]);
    let mut c = d.client().await;
    let (_, error) = run_job(&mut c, "sources.untrust", json!({"target": "shivammathur/php/php@7.1"})).await;
    assert_eq!(error, None);
    let (_, error) = run_job(&mut c, "sources.untrust", json!({"target": "minio/stable", "tap": true})).await;
    assert_eq!(error, None);
    let log = brew_log(&d);
    assert!(log.contains("untrust --formula shivammathur/php/php@7.1"), "{log}");
    assert!(log.contains("untrust --tap minio/stable"), "{log}");
    assert_eq!(trust_file(&d)["taps"], json!([]));
    assert_eq!(trust_file(&d)["formulae"], json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_trust_refuses_a_target_brew_would_read_as_a_flag() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    for target in ["--tap", "", "two words"] {
        let err = c.call("sources.trust", json!({"target": target})).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{target:?}");
    }
    assert!(!brew_log(&d).contains("trust --"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn sources_list_reports_trust() {
    let d = prepared("").await.start().await;
    d.fx.add_php("7.1");
    // shivammathur/php is tapped; nothing else is.
    std::fs::create_dir_all(d.fx.brew.path().join("Library/Taps/shivammathur/homebrew-php")).unwrap();
    let mut c = d.client().await;
    let rows = c.call("sources.list", Value::Null).await.unwrap();
    let taps: Vec<&str> = rows.as_array().unwrap().iter().map(|r| r["tap"].as_str().unwrap()).collect();
    assert_eq!(taps, ["shivammathur/php", "shivammathur/extensions", "minio/stable"], "{rows}");
    let php = &rows[0];
    assert_eq!((php["tapped"].clone(), php["trusted"].clone()), (json!(true), json!(true)), "{rows}");
    let formulae = php["formulae"].as_array().unwrap();
    let old = formulae.iter().find(|f| f["name"] == "shivammathur/php/php@7.1").expect("php@7.1 is listed");
    assert_eq!((old["trusted"].clone(), old["installed"].clone()), (json!(true), json!(true)));
    assert_eq!((rows[1]["tapped"].clone(), rows[1]["trusted"].clone()), (json!(false), json!(true)));
    assert_eq!((rows[2]["tapped"].clone(), rows[2]["trusted"].clone()), (json!(false), json!(false)));
    assert_eq!(rows[2]["formulae"], json!([]));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn sources_list_shows_formulae_trusted_one_at_a_time() {
    let d = prepared("").await.start().await;
    set_trust(&d, &["shivammathur/extensions"], &["shivammathur/php/php@7.3"]);
    let mut c = d.client().await;
    let rows = c.call("sources.list", Value::Null).await.unwrap();
    let php = &rows[0];
    assert_eq!(php["trusted"], json!(false));
    let one = php["formulae"].as_array().unwrap().iter().find(|f| f["name"] == "shivammathur/php/php@7.3");
    let one = one.expect("a formula trusted by itself is listed");
    assert_eq!((one["trusted"].clone(), one["installed"].clone()), (json!(true), json!(false)));
    // php@8.4 is installed (the fixture's) and its tap is not trusted.
    let installed =
        php["formulae"].as_array().unwrap().iter().find(|f| f["name"] == "shivammathur/php/php@8.4");
    let installed = installed.expect("an installed formula is listed");
    assert_eq!((installed["trusted"].clone(), installed["installed"].clone()), (json!(false), json!(true)));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn no_homebrew_no_vars_are_set() {
    let d = prepared("").await.start().await;
    let mut c = d.client().await;
    let (lines, error) = run_job(&mut c, "php.install", json!({"php": "7.1"})).await;
    assert_eq!(error, None, "{lines:?}");
    let _ = c.call("sources.list", Value::Null).await.unwrap();
    let log = std::fs::read_to_string(d.fx.brew.path().join("fake-brew-env.log")).unwrap_or_default();
    assert!(!log.is_empty(), "the fake records the HOMEBREW_* keys of each call");
    // yardd inherits this process's environment, so a key the developer's shell sets is not
    // Yard's doing. Everything else Yard sets is the one variable `Brew::job_env` names.
    let inherited: Vec<String> =
        std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("HOMEBREW_")).collect();
    for key in log.split_whitespace() {
        assert!(
            key == "HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK" || inherited.iter().any(|k| k == key),
            "yardd set {key}"
        );
    }
    assert!(
        log.contains("HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK"),
        "the install job keeps its variable: {log}"
    );
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn fake_brew_describes_and_installs_service_formulae() {
    let d = prepared("").await.start().await;
    let prefix = d.fx.brew.path();
    let formulae = json!([
        {"name": "mysql@8.4", "full_name": "mysql@8.4", "tap": "homebrew/core", "versions": {"stable": "8.4.13"}},
        {"name": "minio", "full_name": "minio/stable/minio", "tap": "minio/stable", "versions": {"stable": "2025.1.1"}},
        {"name": "postgresql@17", "full_name": "postgresql@17", "tap": "homebrew/core", "versions": {"stable": "17.2"}},
        {"name": "redis", "full_name": "redis", "tap": "homebrew/core", "versions": {"stable": "8.0.1"}},
    ]);
    std::fs::write(prefix.join("fake-formulae.json"), formulae.to_string()).unwrap();
    let brew =
        |args: &[&str]| std::process::Command::new(prefix.join("bin/brew")).args(args).output().unwrap();
    let out = brew(&["--version"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "Homebrew 7.0.10");
    let out = brew(&["search", "--formula", "/^postgresql(@|$)/"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "postgresql@17");
    let out = brew(&["search", "--formula", "/^mysql(@|$)/"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "mysql@8.4");
    let out = brew(&["info", "--json=v2", "mysql@8.4"]);
    let info: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(info["formulae"][0]["versions"]["stable"], "8.4.13");
    assert_eq!(info["casks"], json!([]));
    assert!(!brew(&["info", "--json=v2", "mysql@9.9"]).status.success());
    for (formula, bins) in [
        ("mysql@8.4", vec!["opt/mysql@8.4/bin/mysqld"]),
        ("minio/stable/minio", vec!["opt/minio/bin/minio"]),
        ("postgresql@17", vec!["opt/postgresql@17/bin/postgres", "opt/postgresql@17/bin/initdb"]),
        ("redis", vec!["opt/redis/bin/redis-server"]),
    ] {
        assert!(brew(&["install", formula]).status.success(), "{formula}");
        for bin in bins {
            assert!(prefix.join(bin).exists(), "{formula} installs {bin}");
        }
    }
    d.shutdown().await.unwrap();
}

/// The formulae `sources.list` shows for the `minio/stable` tap.
async fn minio_rows(d: &Daemon) -> Vec<String> {
    let mut c = d.client().await;
    let rows = c.call("sources.list", Value::Null).await.unwrap();
    let minio = rows.as_array().unwrap().iter().find(|r| r["tap"] == "minio/stable").unwrap().clone();
    minio["formulae"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect()
}

fn describe_minio(d: &Daemon, full_name: &str, tap: &str) {
    let minio =
        json!([{"name": "minio", "full_name": full_name, "tap": tap, "versions": {"stable": "2025.9.7"}}]);
    std::fs::write(d.fx.brew.path().join("fake-formulae.json"), minio.to_string()).unwrap();
}

#[tokio::test]
async fn a_core_minio_is_not_an_installed_formula_of_the_minio_tap() {
    // homebrew/core's minio and the tap's share the keg `opt/minio`.
    let d = prepared("").await;
    d.fx.add_formula("minio");
    describe_minio(&d, "minio", "homebrew/core");
    let d = d.start().await;

    // No instance says which: brew does, and it names core.
    assert_eq!(minio_rows(&d).await, Vec::<String>::new());
    assert!(brew_log(&d).contains("info --json=v2 minio"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();

    // An instance that records the core formula says so without asking brew.
    let d = prepared("").await;
    d.fx.add_formula("minio");
    d.fx.add_service("minio", "minio", true);
    describe_minio(&d, "minio/stable/minio", "minio/stable");
    let d = d.start().await;
    assert_eq!(minio_rows(&d).await, Vec::<String>::new());
    assert!(!brew_log(&d).contains("info --json=v2"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_minio_tap_formula_is_listed_when_an_instance_or_brew_says_the_keg_is_its() {
    // An instance records the tap's formula.
    let d = prepared("").await;
    d.fx.add_formula("minio");
    d.fx.add_service("minio", "minio/stable/minio", true);
    let d = d.start().await;
    assert_eq!(minio_rows(&d).await, ["minio/stable/minio"]);
    assert!(!brew_log(&d).contains("info --json=v2"), "{}", brew_log(&d));
    d.shutdown().await.unwrap();

    // No instance, and brew says the installed keg came from the tap.
    let d = prepared("").await;
    d.fx.add_formula("minio");
    describe_minio(&d, "minio/stable/minio", "minio/stable");
    // brew describes a tap's formula only while it trusts the tap.
    set_trust(&d, &["minio/stable"], &[]);
    let d = d.start().await;
    assert_eq!(minio_rows(&d).await, ["minio/stable/minio"]);
    d.shutdown().await.unwrap();

    // No instance, and brew cannot say: nothing is claimed.
    let d = prepared("").await;
    d.fx.add_formula("minio");
    let d = d.start().await;
    assert_eq!(minio_rows(&d).await, Vec::<String>::new());
    d.shutdown().await.unwrap();

    // No keg at all, whatever an instance records.
    let d = prepared("").await;
    d.fx.add_service("minio", "minio/stable/minio", true);
    let d = d.start().await;
    assert_eq!(minio_rows(&d).await, Vec::<String>::new());
    d.shutdown().await.unwrap();
}

#[tokio::test]
async fn asking_brew_whether_minio_is_the_taps_is_bounded() {
    use std::time::{Duration, Instant};
    use yard_core::brew::Brew;
    use yard_core::config::load_yard;

    let d = prepared("").await;
    d.fx.add_formula("minio");
    describe_minio(&d, "minio/stable/minio", "minio/stable");
    set_trust(&d, &["minio/stable"], &[]);
    let brew = Brew::new(d.fx.brew.path());
    let (yard, _) = load_yard(&d.fx.paths).unwrap();

    // brew answers: the keg is the tap's.
    let found = yard_daemon::sources::installed_within(&brew, &yard, None, Duration::from_secs(10)).await;
    assert!(found.contains("minio/stable/minio"), "{found:?}");

    // brew hangs: the answer is "not the tap's", and the call returns when the bound does.
    std::fs::write(d.fx.brew.path().join("fake-brew-info-delay"), "20000").unwrap();
    let started = Instant::now();
    let found = yard_daemon::sources::installed_within(&brew, &yard, None, Duration::from_millis(300)).await;
    assert!(!found.contains("minio/stable/minio"), "{found:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "waited {:?}", started.elapsed());
}
