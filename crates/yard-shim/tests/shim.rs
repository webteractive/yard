//! The shim binary end to end: a temp data root and brew prefix whose `php` is a script.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const SHIM: &str = env!("CARGO_BIN_EXE_yard-shim");

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Setup {
    tmp: TempDir,
    root: PathBuf,
    brew: PathBuf,
    project: PathBuf,
    herd_bin: PathBuf,
}

/// A site `old` isolated to 7.1, a 7.1 keg whose `php` prints its argv, PHPRC and PATH,
/// and a Herd `bin` with a `php` that prints HERD.
fn setup() -> Setup {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, brew, project) = (base.join("root"), base.join("brew"), base.join("code/old"));
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("sites.toml"),
        format!("[sites.old]\npath = {:?}\nphp = \"7.1\"\n", project.display().to_string()),
    )
    .unwrap();
    script(
        &brew.join("opt/php@7.1/bin/php"),
        "echo \"argv0:$0\"; for a in \"$@\"; do echo \"arg:$a\"; done; echo \"PHPRC:$PHPRC\"; echo \"PATH:$PATH\"",
    );
    let herd_bin = base.join("Library/Application Support/Herd/bin");
    script(&herd_bin.join("php"), "echo HERD");
    Setup { tmp, root, brew, project, herd_bin }
}

fn run(s: &Setup, args: &[&str], debug: bool) -> Output {
    let link = s.tmp.path().join("shims/php");
    if !link.exists() {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(SHIM, &link).unwrap();
    }
    let mut cmd = Command::new(&link);
    cmd.args(args)
        .current_dir(s.project.join("app"))
        .env("YARD_ROOT", &s.root)
        .env("YARD_BREW_PREFIX", &s.brew)
        .env("PATH", format!("{}:/usr/bin:/bin", s.herd_bin.display()));
    if debug {
        cmd.env("YARD_SHIM_DEBUG", "1");
    }
    cmd.output().unwrap()
}

#[test]
fn php_shim_runs_the_sites_version_with_the_args_unchanged() {
    let s = setup();
    let out = run(&s, &["-r", "echo 1;", "a b"], false);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("argv0:") && stdout.contains("php@7.1/bin/php"), "{stdout}");
    assert!(stdout.contains("arg:-r\narg:echo 1;\narg:a b\n"), "{stdout}");
}

#[test]
fn shim_child_path_reaches_brew_not_herd() {
    let s = setup();
    let out = run(&s, &[], false);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("HERD"), "{stdout}");
    assert!(!stdout.contains("/Herd/"), "{stdout}");
    let path = stdout.lines().find_map(|l| l.strip_prefix("PATH:")).expect("PATH printed");
    assert!(
        path.starts_with(&format!(
            "{}:{}",
            s.brew.join("opt/php@7.1/bin").display(),
            s.root.join("bin").display()
        )),
        "{path}"
    );
}

#[test]
fn debug_names_the_version_and_why() {
    let s = setup();
    let out = run(&s, &[], true);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("PHP 7.1") && stderr.contains("site old"), "{stderr}");
}

#[test]
fn a_missing_version_fails_with_a_hint() {
    let s = setup();
    std::fs::remove_dir_all(s.brew.join("opt/php@7.1")).unwrap();
    let out = run(&s, &[], false);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("yard: ") && stderr.contains("  hint: yard php install 7.1"), "{stderr}");
}

#[test]
fn a_broken_sites_toml_warns_and_runs_the_default() {
    let s = setup();
    script(&s.brew.join("opt/php@8.4/bin/php"), "echo \"argv0:$0\"");
    std::fs::write(s.root.join("sites.toml"), "[sites.old\n").unwrap();
    let out = run(&s, &[], false);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("php@8.4/bin/php"), "{stdout}");
    assert!(stderr.starts_with("yard: ") && stderr.contains("sites.toml"), "{stderr}");
}

#[test]
fn a_broken_yard_toml_warns_and_keeps_site_isolation() {
    let s = setup();
    std::fs::write(s.root.join("yard.toml"), "default_php = \n").unwrap();
    let out = run(&s, &[], false);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("php@7.1/bin/php"), "{stdout}");
    assert!(stderr.starts_with("yard: ") && stderr.contains("yard.toml"), "{stderr}");
}

#[test]
fn missing_project_php_hints_init() {
    let s = setup();
    std::fs::write(s.project.join(".yard"), "php = \"7.3\"\n").unwrap();
    let out = run(&s, &[], false);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let file = s.project.join(".yard");
    assert!(stderr.contains(&format!("yard: PHP 7.3 is not installed ({})", file.display())), "{stderr}");
    assert!(stderr.contains("  hint: yard init"), "{stderr}");
}

fn write_managed(s: &Setup, text: &str) -> PathBuf {
    let file = s.root.join("sites/old/.yard");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, format!("{text}project = {:?}\n", s.project.display().to_string())).unwrap();
    file
}

#[test]
fn managed_file_php_is_run() {
    let s = setup();
    script(&s.brew.join("opt/php@7.3/bin/php"), "echo \"argv0:$0\"");
    let file = write_managed(&s, "php = \"7.3\"\n");
    let out = run(&s, &[], true);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("php@7.3/bin/php"), "the managed file beats the 7.1 isolation: {stdout}");
    assert!(stderr.contains(&file.display().to_string()), "{stderr}");
}

#[test]
fn missing_managed_php_hints_init() {
    let s = setup();
    let file = write_managed(&s, "php = \"7.3\"\n");
    let out = run(&s, &[], false);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&format!("yard: PHP 7.3 is not installed ({})", file.display())), "{stderr}");
    assert!(stderr.contains("  hint: yard init"), "{stderr}");
}

#[test]
fn project_file_php_is_run() {
    let s = setup();
    script(&s.brew.join("opt/php@8.4/bin/php"), "echo \"argv0:$0\"");
    std::fs::write(s.project.join(".yard"), "docroot = \"public\"\n").unwrap();
    let out = run(&s, &[], true);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("php@8.4/bin/php"), "the file has no php, so the default: {stdout}");
    assert!(stderr.contains(&s.project.join(".yard").display().to_string()), "{stderr}");
}
