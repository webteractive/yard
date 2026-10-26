//! The shim binary end to end: a temp data root and brew prefix whose `php` is a script.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
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

#[test]
fn an_unusable_committed_file_is_an_error_not_a_fall_through() {
    let s = setup();
    write_managed(&s, "php = \"7.3\"\n");
    symlink(s.project.join("gone"), s.project.join(".yard")).unwrap();
    let out = run(&s, &[], false);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let file = s.project.join(".yard");
    assert!(stderr.contains(&format!("yard: {} is a symlink to nothing", file.display())), "{stderr}");
    assert!(stderr.contains(&format!("  hint: replace {} with a text file", file.display())), "{stderr}");
}

#[test]
fn a_directory_named_yard_is_not_a_project_file() {
    let s = setup();
    std::fs::create_dir(s.project.join("app/.yard")).unwrap();
    let out = run(&s, &[], false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("php@7.1/bin/php"));
}

/// Runs the shim as `name` in the project's `app/`.
fn run_as(s: &Setup, name: &str, args: &[&str]) -> Output {
    let link = s.tmp.path().join("shims").join(name);
    if !link.exists() {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(SHIM, &link).unwrap();
    }
    Command::new(&link)
        .args(args)
        .current_dir(s.project.join("app"))
        .env("YARD_ROOT", &s.root)
        .env("YARD_BREW_PREFIX", &s.brew)
        .env("PATH", format!("{}:/usr/bin:/bin", s.herd_bin.display()))
        .output()
        .unwrap()
}

fn write_sleeping(s: &Setup, text: &str) {
    std::fs::create_dir_all(s.root.join("run")).unwrap();
    std::fs::write(s.root.join("run/sleeping"), text).unwrap();
}

/// A wake.sock that records each request line and answers 204.
fn fake_wake(s: &Setup) -> Arc<Mutex<Vec<String>>> {
    std::fs::create_dir_all(s.root.join("run")).unwrap();
    let listener = UnixListener::bind(s.root.join("run/wake.sock")).unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut line = String::new();
            let _ = BufReader::new(&stream).read_line(&mut line);
            record.lock().unwrap().push(line.trim_end().to_string());
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        }
    });
    seen
}

#[test]
fn php_wakes_a_listed_site() {
    let s = setup();
    write_sleeping(&s, "old\n");
    let seen = fake_wake(&s);
    let out = run_as(&s, "php", &["-v"]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{stderr}");
    assert_eq!(*seen.lock().unwrap(), ["GET /wake/old?wait=30 HTTP/1.1"]);
    assert!(stdout.contains("php@7.1/bin/php") && stdout.contains("arg:-v"), "{stdout}");
    assert!(stderr.contains("yard: waking the services old uses"), "{stderr}");
}

#[test]
fn php_does_not_connect_when_the_site_is_not_listed() {
    let s = setup();
    write_sleeping(&s, "other\n");
    let seen = fake_wake(&s);
    let out = run_as(&s, "php", &["-v"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(!stderr.contains("waking"), "{stderr}");
}

#[test]
fn a_dead_socket_still_runs_php() {
    let s = setup();
    write_sleeping(&s, "old\n");
    let out = run_as(&s, "php", &["-v"]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("php@7.1/bin/php"), "{stdout}");
    assert!(stderr.contains("yard: could not wake old's services ("), "{stderr}");
    assert!(stderr.contains("); running anyway"), "{stderr}");
}

#[test]
fn node_never_wakes() {
    let s = setup();
    write_sleeping(&s, "old\n");
    let seen = fake_wake(&s);
    let out = run_as(&s, "node", &["-v"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("is not installed"), "{stderr}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(!stderr.contains("waking"), "{stderr}");
}
