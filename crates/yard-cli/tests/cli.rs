use assert_cmd::Command;

fn yard(args: &[&str]) -> std::process::Output {
    Command::cargo_bin("yard").unwrap().args(args).output().unwrap()
}

fn first_line(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string()
}

#[test]
fn version_flag_prints_version_and_commit() {
    let out = yard(&["--version"]);
    assert!(out.status.success());
    let line = first_line(&out);
    assert!(line.starts_with("yard 0.1.0 ("), "{line}");
    assert!(line.ends_with(')'), "{line}");
}

#[test]
fn short_flag_and_subcommand_match_long_flag() {
    let long = first_line(&yard(&["--version"]));
    assert!(long.starts_with("yard "), "{long}");
    let short = yard(&["-V"]);
    let sub = yard(&["version"]);
    assert!(short.status.success() && sub.status.success());
    assert_eq!(first_line(&short), long);
    assert_eq!(first_line(&sub), long);
}

#[test]
fn version_json_has_stable_shape() {
    let out = yard(&["version", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["cli"], "0.1.0");
    assert_eq!(v["protocol"], "1.0");
    assert!(v["commit"].is_string());
    assert!(v["build_date"].is_string());
    assert!(v["daemon"].is_null());
    assert!(v["helper"].is_null());
}
