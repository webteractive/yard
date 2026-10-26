//! The `node`, `npm` and `npx` shims end to end: a temp data root whose installed Node
//! versions are scripts that print what they were run with.

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
    project: PathBuf,
    herd_bin: PathBuf,
}

const PRINT: &str = "echo \"argv0:$0\"; for a in \"$@\"; do echo \"arg:$a\"; done; echo \"PATH:$PATH\"; echo \"CA:$NODE_EXTRA_CA_CERTS\"";

/// Node 16.14.0 installed with `node` and `npm`, a project pinning it in an `.nvmrc` an
/// editor saved with CRLF, and a Herd `bin` with a `node` that prints HERD.
fn setup() -> Setup {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, project) = (base.join("root"), base.join("code/maker"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(".nvmrc"), "v16.14.0\r\n").unwrap();
    let bin = root.join("node/node-versions/v16.14.0/installation/bin");
    for name in ["node", "npm"] {
        script(&bin.join(name), PRINT);
    }
    let herd_bin = base.join("Library/Application Support/Herd/bin");
    script(&herd_bin.join("node"), "echo HERD");
    Setup { tmp, root, project, herd_bin }
}

fn run(s: &Setup, name: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
    let link = s.tmp.path().join("shims").join(name);
    if !link.exists() {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(SHIM, &link).unwrap();
    }
    let mut cmd = Command::new(&link);
    cmd.args(args)
        .current_dir(&s.project)
        .env("YARD_ROOT", &s.root)
        .env("PATH", format!("{}:/usr/bin:/bin", s.herd_bin.display()))
        .env_remove("NODE_EXTRA_CA_CERTS")
        .env_remove("YARD_SHIM_DEBUG")
        .envs(env.iter().copied());
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn node_shim_runs_the_pinned_version_with_args_unchanged() {
    let s = setup();
    let out = stdout(&run(&s, "node", &["-e", "x", "a b"], &[]));
    assert!(out.contains("argv0:") && out.contains("v16.14.0/installation/bin/node"), "{out}");
    assert!(out.contains("arg:-e\narg:x\narg:a b\n"), "{out}");
}

#[test]
fn npm_shim_runs_npm_with_the_version_bin_first() {
    let s = setup();
    let out = stdout(&run(&s, "npm", &[], &[]));
    assert!(out.contains("v16.14.0/installation/bin/npm"), "{out}");
    let path = out.lines().find_map(|l| l.strip_prefix("PATH:")).unwrap();
    let entries: Vec<&str> = path.split(':').collect();
    assert_eq!(entries[0], s.root.join("node/node-versions/v16.14.0/installation/bin").to_str().unwrap());
    assert_eq!(entries[1], s.root.join("bin").to_str().unwrap());
    assert!(!out.contains("HERD") && !path.contains("/Herd/"), "{out}");
}

#[test]
fn a_missing_pin_names_the_file_and_the_install() {
    let s = setup();
    std::fs::write(s.project.join(".nvmrc"), "16.20.2\n").unwrap();
    let out = run(&s, "node", &["-v"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "yard: Node 16.20.2 is not installed ({})\n  hint: yard node install 16.20.2\n",
            s.project.join(".nvmrc").display()
        )
    );
}

#[test]
fn a_half_installed_version_is_missing() {
    let s = setup();
    std::fs::create_dir_all(s.root.join("node/node-versions/v22.18.0")).unwrap();
    std::fs::write(s.project.join(".nvmrc"), "22\n").unwrap();
    let out = run(&s, "node", &["-v"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "yard: Node 22 is not installed ({})\n  hint: yard node install 22\n",
            s.project.join(".nvmrc").display()
        )
    );
}

#[test]
fn the_default_is_22_and_says_so() {
    let s = setup();
    std::fs::remove_file(s.project.join(".nvmrc")).unwrap();
    let out = run(&s, "npx", &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "yard: Node 22 is not installed (the default)\n  hint: yard node install 22\n"
    );
}

#[test]
fn a_users_extra_ca_is_kept() {
    let s = setup();
    let ca = s.root.join("ca/yard-ca.pem");
    std::fs::create_dir_all(ca.parent().unwrap()).unwrap();
    std::fs::write(&ca, "").unwrap();
    let out = stdout(&run(&s, "node", &[], &[("NODE_EXTRA_CA_CERTS", "/mine.pem")]));
    assert!(out.contains("CA:/mine.pem\n"), "{out}");
    let out = stdout(&run(&s, "node", &[], &[]));
    assert!(out.contains(&format!("CA:{}\n", ca.display())), "{out}");
}

#[test]
fn debug_names_the_spec_and_its_file() {
    let s = setup();
    let out = run(&s, "node", &[], &[("YARD_SHIM_DEBUG", "1")]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(&format!(
            "yard-shim: node runs Node 16.14.0 (16.14.0 from {})",
            s.project.join(".nvmrc").display()
        )),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
