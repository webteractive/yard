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
/// a Herd `bin` with a `php` that prints HERD, and the defaults yardd writes (8.4, 22).
fn setup() -> Setup {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, brew, project) = (base.join("root"), base.join("brew"), base.join("code/old"));
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("yard.toml"), "default_php = \"8.4\"\ndefault_node = \"22\"\n").unwrap();
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
fn an_unpinned_project_with_no_default_names_the_install() {
    let s = setup();
    std::fs::write(s.root.join("yard.toml"), "").unwrap();
    std::fs::write(s.project.join(".yard"), "docroot = \"public\"\n").unwrap();
    let out = run(&s, &[], false);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("yard: no default PHP is written in yard.toml"), "{stderr}");
    assert!(stderr.contains("  hint: yard php install 8.4"), "{stderr}");
}

#[test]
fn a_broken_yard_toml_runs_the_last_good_defaults() {
    let s = setup();
    script(&s.brew.join("opt/php@7.4/bin/php"), "echo \"argv0:$0\"");
    std::fs::create_dir_all(s.root.join("run")).unwrap();
    std::fs::write(s.root.join("run/defaults.toml"), "default_php = \"7.4\"\n").unwrap();
    std::fs::write(s.root.join("yard.toml"), "default_php = \n").unwrap();
    std::fs::write(s.project.join(".yard"), "docroot = \"public\"\n").unwrap();
    let out = run(&s, &[], false);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("php@7.4/bin/php"), "the last good default, not 8.4: {stdout}");
    assert!(stderr.contains("using the last good defaults from"), "{stderr}");
}

#[test]
fn a_broken_yard_toml_with_no_last_good_fails_an_unpinned_project() {
    let s = setup();
    script(&s.brew.join("opt/php@8.4/bin/php"), "echo \"argv0:$0\"");
    std::fs::write(s.root.join("yard.toml"), "default_php = \n").unwrap();
    std::fs::write(s.project.join(".yard"), "docroot = \"public\"\n").unwrap();
    let out = run(&s, &[], false);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(!stdout.contains("argv0:"), "PHP ran: {stdout}");
    assert!(stderr.contains("yard.toml line "), "the config error itself: {stderr}");
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
    fake_wake_answering(s, "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
}

/// A wake.sock that records each request line and answers `answer`.
fn fake_wake_answering(s: &Setup, answer: &'static str) -> Arc<Mutex<Vec<String>>> {
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
            let _ = stream.write_all(answer.as_bytes());
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

/// yardd refuses when another program holds a port the site's services need: PHP does not
/// run against it, and the shim says why.
#[test]
fn a_refused_wake_does_not_run_php() {
    let s = setup();
    write_sleeping(&s, "old\n");
    fake_wake_answering(
        &s,
        "HTTP/1.1 403 Forbidden\r\nContent-Length: 39\r\nConnection: close\r\n\r\nredis: port 6380 is held by nc (pid 9)\n",
    );
    let out = run_as(&s, "php", &["-v"]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(!stdout.contains("php@7.1/bin/php"), "PHP ran: {stdout}");
    assert!(stderr.contains("redis: port 6380 is held by nc (pid 9)"), "{stderr}");
    assert!(stderr.contains("yard doctor"), "{stderr}");
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

// ---- composer (runtimes spec §9) ----

/// A Composer version as `runtime.install` leaves it, `record` false leaving it half there.
fn put_composer(s: &Setup, version: &str, record: bool) -> PathBuf {
    let root = s.root.join("installs/composer").join(version);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("composer.phar"), "<?php\n").unwrap();
    if record {
        let requires = if version.starts_with("2.2.") { "" } else { "requires_php = \"7.2.5\"\n" };
        std::fs::write(
            root.join(".yard-install.toml"),
            format!(
                "version = \"{version}\"\nurl = \"https://getcomposer.org/download/{version}/composer.phar\"\nsha256 = \"{}\"\n{requires}installed_at = \"2026-11-19T00:00:00Z\"\n",
                "a".repeat(64)
            ),
        )
        .unwrap();
    }
    root
}

/// Pins site `old`'s Composer in `sites.toml`, as yardd's fill-in does for a PHP below 7.2.5.
fn pin_composer(s: &Setup, version: &str) {
    let mut sites = std::fs::read_to_string(s.root.join("sites.toml")).unwrap();
    sites.push_str(&format!("[sites.old.runtimes]\ncomposer = \"{version}\"\n"));
    std::fs::write(s.root.join("sites.toml"), sites).unwrap();
}

fn texts(out: &Output) -> (String, String) {
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn composer_runs_the_lts_under_the_projects_php() {
    let s = setup();
    let home = put_composer(&s, "2.2.32", true);
    put_composer(&s, "2.11.2", true);
    pin_composer(&s, "2.2.32");
    let out = run_as(&s, "composer", &["install", "--no-dev"]);
    let (stdout, stderr) = texts(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("argv0:") && stdout.contains("php@7.1/bin/php"), "{stdout}");
    let phar = format!("arg:{}", home.join("composer.phar").display());
    let lines: Vec<&str> = stdout.lines().filter(|l| l.starts_with("arg:")).collect();
    assert_eq!(lines, [phar.as_str(), "arg:install", "arg:--no-dev"], "{stdout}");
    assert!(!stdout.contains("Herd"), "{stdout}");
}

#[test]
fn a_version_without_its_record_is_not_run() {
    let s = setup();
    put_composer(&s, "2.2.32", false);
    pin_composer(&s, "2.2.32");
    let out = run_as(&s, "composer", &["install"]);
    let (stdout, stderr) = texts(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stderr.contains("Composer 2.2.32 is not installed"), "{stderr}");
    assert!(stderr.contains("hint: yard runtime install composer 2.2.32"), "{stderr}");
}

#[test]
fn self_update_is_refused_in_every_spelling() {
    let s = setup();
    put_composer(&s, "2.2.32", true);
    pin_composer(&s, "2.2.32");
    for args in [
        &["self-update"][..],
        &["-n", "self-update"],
        &["-d", "/tmp", "self-update"],
        &["--working-dir", "/tmp", "selfupdate", "--2.2"],
        &["--working-dir=/tmp", "self-update"],
        &["self-u"],
        &["selfu"],
        &["SELF-UPDATE"],
        &["global", "self-update"],
        &["global", "-n", "self-up"],
        // A `-d` value that names the command is refused: when an option Composer does not bind
        // globally (`-r`) makes its first pass fail, Composer runs it as the command.
        &["-d", "self-update", "install"],
        &["-rd", "self-update"],
        // Symfony skips the word after a cluster ending in `d` when it equals `-d`'s value,
        // even when that value came from the cluster itself (`-dd` binds `d`).
        &["-dd", "d", "self-update"],
        &["-d/tmp/d", "/tmp/d", "self-update"],
        // `global` re-parses its arguments as one string, splitting an option word at spaces.
        &["global", "-n self-update"],
        &["global", "-n self-update --x=y"],
        // Composer's second parse after `global` drops quotes and backslashes, and a skipped
        // `-d` value may be `global` itself; any word that abbreviates the command is refused.
        &["-d/dev/fd", "global", "-n self-update"],
        &["global", "-n 'self-update'"],
        &["global", "-n \"self-update\""],
        &["global", "-n self\"-up\"date"],
        &["global", "-n \\self-update"],
        &["global", "-n global '-n self-update'"],
        &["--working-dir=x", "sel"],
        // ...and decodes backslash escapes as PHP's stripcslashes does, once per `global`.
        &["global", "\\x73elf-update"],
        &["global", "\\123ELF"],
        &["global", "-n '\\x73elf-update'"],
        &["global", "global '\\x5cx73elf-update'"],
        // An option word before `global` is written out as typed for the second parse too.
        &["-n self-update", "global"],
        &["-n 'self-update'", "global"],
    ] {
        let out = run_as(&s, "composer", args);
        let (stdout, stderr) = texts(&out);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stdout}");
        assert!(stderr.contains("is managed by Yard"), "{args:?}: {stderr}");
        assert!(stderr.contains("hint: yard runtime install composer <version>"), "{args:?}: {stderr}");
    }
    // `global` deletes the first `g`…`global` it finds in the rebuilt line, even inside a word.
    let out = run_as(&s, "composer", &["-n sel\"g\"f-update -nd", "global"]);
    assert_eq!(out.status.code(), Some(1), "sel\"g\"f: {}", texts(&out).0);
    // A word decoded past MAX_DECODES rounds while `global` is named is refused, and a long word
    // costs linear time, with or without `global`.
    let mut deeper: Vec<String> = vec!["global".to_string(); 2];
    deeper.push(format!("\\x5c{}x73elf-update", "x5c".repeat(40)));
    let deeper: Vec<&str> = deeper.iter().map(String::as_str).collect();
    let out = run_as(&s, "composer", &deeper);
    assert_eq!(out.status.code(), Some(1), "deeper: {}", texts(&out).0);
    let long = format!("\\x5c{}", "x5c".repeat(30_000));
    let started = std::time::Instant::now();
    let out = run_as(&s, "composer", &["require", &long]);
    assert!(out.status.success(), "{}", texts(&out).1);
    let out = run_as(&s, "composer", &["global", "require", &long]);
    assert_eq!(out.status.code(), Some(1), "a long undecodable word under global: {}", texts(&out).0);
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "took {:?}", started.elapsed());
    // Each nested `global` decodes once more; however deep, the shim decodes as far.
    let mut deep: Vec<String> = vec!["global".to_string(); 9];
    deep.push(format!("\\x5c{}x73elf-update", "x5c".repeat(7)));
    let deep: Vec<&str> = deep.iter().map(String::as_str).collect();
    let out = run_as(&s, "composer", &deep);
    assert_eq!(out.status.code(), Some(1), "{deep:?}: {}", texts(&out).0);
    for args in [
        &["search", "x"][..],
        &["-d", "./self-update", "install"],
        &["global", "require", "x/y"],
        &["require", "self-update/x"],
        &["show", "se"],
        // Outside `global`, an argument is only a command where Composer reads one.
        &["init", "--description", "Self hosted app"],
        &["run-script", "self"],
        &["require", "symfony/console", "--working-dir=./app"],
    ] {
        let out = run_as(&s, "composer", args);
        assert!(out.status.success(), "{args:?}: {}", texts(&out).1);
    }
}

#[test]
fn the_yard_toml_default_is_a_plain_prefix() {
    let s = setup();
    put_composer(&s, "2.2.32", true);
    std::fs::write(s.root.join("yard.toml"), "[runtimes.composer]\ndefault = \"2\"\n").unwrap();
    let out = run_as(&s, "composer", &["-V"]);
    let (stdout, stderr) = texts(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("installs/composer/2.2.32/composer.phar"), "{stdout}");
}

#[test]
fn the_written_default_never_follows_a_newer_install() {
    let s = setup();
    script(
        &s.brew.join("opt/php@8.4/bin/php"),
        "echo \"argv0:$0\"; for a in \"$@\"; do echo \"arg:$a\"; done",
    );
    std::fs::write(s.project.join(".yard"), "php = \"8.4\"\n").unwrap();
    put_composer(&s, "2.11.2", true);
    put_composer(&s, "2.12.0", true);
    std::fs::write(s.root.join("yard.toml"), "[runtimes.composer]\ndefault = \"2.11.2\"\n").unwrap();
    let out = run_as(&s, "composer", &["-V"]);
    let (stdout, stderr) = texts(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("installs/composer/2.11.2/composer.phar"), "{stdout}");
}

#[test]
fn brews_composer_runs_while_yard_has_none() {
    let s = setup();
    script(
        &s.brew.join("opt/php@8.4/bin/php"),
        "echo \"argv0:$0\"; for a in \"$@\"; do echo \"arg:$a\"; done",
    );
    std::fs::write(s.project.join(".yard"), "php = \"8.4\"\n").unwrap();
    script(&s.brew.join("bin/composer"), "echo brew");
    let out = run_as(&s, "composer", &["-V"]);
    let (stdout, stderr) = texts(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("yard: using brew's composer"), "{stderr}");
    assert!(stdout.contains("php@8.4/bin/php"), "{stdout}");
    assert!(stdout.contains(&format!("arg:{}", s.brew.join("bin/composer").display())), "{stdout}");
    // Not on PHP 7.1, which brew's Composer cannot run.
    std::fs::remove_file(s.project.join(".yard")).unwrap();
    let out = run_as(&s, "composer", &["-V"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = texts(&out).1;
    assert!(stderr.contains("Composer has no pin for this project"), "{stderr}");
    assert!(stderr.contains("hint: yard runtime install composer 2.2"), "{stderr}");
}

#[test]
fn composer_wakes_a_listed_site() {
    let s = setup();
    put_composer(&s, "2.2.32", true);
    pin_composer(&s, "2.2.32");
    write_sleeping(&s, "old\n");
    let seen = fake_wake(&s);
    let out = run_as(&s, "composer", &["install"]);
    assert!(out.status.success(), "{}", texts(&out).1);
    assert_eq!(*seen.lock().unwrap(), ["GET /wake/old?wait=30 HTTP/1.1"]);
}

#[test]
fn composer_in_an_unpinned_old_project_names_the_pin() {
    let s = setup();
    put_composer(&s, "2.2.32", true);
    put_composer(&s, "2.11.2", true);
    std::fs::write(
        s.root.join("yard.toml"),
        "default_php = \"8.4\"\n\n[runtimes.composer]\ndefault = \"2.11.2\"\n",
    )
    .unwrap();
    std::fs::write(s.project.join(".yard"), "php = \"7.1\"\n").unwrap();
    let out = run_as(&s, "composer", &["install"]);
    let (stdout, stderr) = texts(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stderr.contains("Composer 2.11.2 needs PHP 7.2.5 or later"), "{stderr}");
    let line = format!("hint: add [runtimes] composer = \"2.2.32\" to {}", s.project.join(".yard").display());
    assert!(stderr.contains(&line), "{stderr}");
}
