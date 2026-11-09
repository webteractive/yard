//! `yard uninstall`: takes Yard off the Mac, its data included unless `--keep-data` (M7 spec §10).

use super::lifecycle::{agent_installed_here, agent_plist, stop_agent};
use super::{Ctx, Outcome, interactive};
use serde::Serialize;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use yard_core::brew::Brew;
use yard_core::install::{InstallKind, UninstallFacts, UninstallStep, install_kind, uninstall_plan};
use yard_core::paths::HELPER_BINARY;
use yard_proto::{ErrorCode, YardError};

const PREFS_DOMAIN: &str = "dev.more.yard.app";

#[derive(Debug, Serialize)]
struct StepOutcome {
    step: UninstallStep,
    outcome: &'static str,
    message: String,
}

pub async fn uninstall(ctx: &Ctx, keep_data: bool, yes: bool) -> Outcome {
    let exe = std::env::current_exe().map_err(|e| YardError::internal(format!("cannot locate yard: {e}")))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let brew = Brew::from_env(&|p: &Path| p.exists()).ok();
    let caskroom = brew.as_ref().is_some_and(|b| b.prefix.join("Caskroom/yard").exists());
    let ca = ctx.paths.ca_cert();
    let facts = UninstallFacts {
        kind: install_kind(&exe, caskroom),
        agent_installed: agent_installed_here(ctx),
        // Planned whenever the CA file is there unless security says it is not trusted: an
        // unanswered check must not skip it, since the file is about to be deleted with the data.
        ca_exists: ca.exists() && yard_daemon::tls::trusted(&ctx.security, &ca).await != Some(false),
        helper_installed: Path::new(HELPER_BINARY).exists(),
        keep_data,
        data_exists: ctx.paths.root().exists(),
    };
    let mut plan = uninstall_plan(&facts);
    // A scratch root (`--root`) owns only its data and its agent: the helper, the app and the
    // app's preferences belong to the real install.
    if ctx.root_overridden {
        plan.retain(|s| {
            matches!(s, UninstallStep::StopDaemon | UninstallStep::Untrust | UninstallStep::RemoveData)
        });
    }

    if !yes {
        if !interactive(ctx) {
            return Err(YardError::new(ErrorCode::NeedsConfirmation, "uninstalling Yard needs confirmation")
                .with_hint("yard uninstall --yes")
                .into());
        }
        print!("{}", describe(ctx, &plan, &facts.kind));
        print!("Type uninstall to continue: ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer).is_err() || !typed_uninstall(&answer) {
            println!("Nothing was removed.");
            return Ok(1);
        }
    }

    let hosts = tunnel_hosts(ctx);
    let mut outcomes = vec![];
    for step in &plan {
        let result = run_step(ctx, *step, &facts.kind, brew.as_ref()).await;
        let (outcome, message) = match result {
            Ok(message) => ("done", message),
            Err(e) => ("failed", e.hint.map_or(e.message.clone(), |h| format!("{} (fix: {h})", e.message))),
        };
        if !ctx.json {
            println!("{}: {message}", if outcome == "done" { "done" } else { "FAILED" });
        }
        outcomes.push(StepOutcome { step: *step, outcome, message });
    }
    let failed = outcomes.iter().any(|o| o.outcome == "failed");
    if ctx.json {
        crate::output::print_json(&serde_json::json!({ "steps": outcomes, "tunnel_hosts": hosts }));
    } else {
        println!(
            "Homebrew and what it installed for Yard (PHP, extensions, nginx, dnsmasq, fnm, the service engines) are left in place; `brew list` shows them."
        );
        if !hosts.is_empty() {
            println!(
                "Delete these DNS records at Cloudflare if you no longer need them: {}",
                hosts.join(", ")
            );
        }
        if failed {
            println!("Some steps failed; each FAILED line names the command that finishes it.");
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn describe(ctx: &Ctx, plan: &[UninstallStep], kind: &InstallKind) -> String {
    let mut out = String::from("yard uninstall will:\n");
    for step in plan {
        let line = match step {
            UninstallStep::StopDaemon => {
                "stop yardd and everything it runs, and remove its LaunchAgent".to_string()
            }
            UninstallStep::Untrust => {
                "remove Yard's CA from your login keychain (macOS asks for your password)".into()
            }
            UninstallStep::HelperUninstall => {
                "uninstall the root helper and Yard's /etc/resolver files (sudo asks for your password)"
                    .into()
            }
            UninstallStep::RemoveData => {
                let mut l =
                    format!("delete {} ({})", ctx.paths.root().display(), human(dir_size(ctx.paths.root())));
                for (name, size) in data_sizes(&ctx.paths.root().join("data")) {
                    l.push_str(&format!("\n    including data/{name} ({})", human(size)));
                }
                l
            }
            UninstallStep::RemovePrefs => "delete Yard.app's settings".into(),
            UninstallStep::RemoveBundle => format!(
                "delete {} and the yard link in ~/.local/bin",
                kind.app().map(|a| a.display().to_string()).unwrap_or_default()
            ),
            UninstallStep::RemoveBrewCask => "run brew uninstall --cask yard".into(),
        };
        out.push_str(&format!("  - {line}\n"));
    }
    out.push_str(
        "It leaves Homebrew and everything brew installed, your projects and their .yard files, Herd, DBngin, Postgres.app and emailulator.\n",
    );
    out
}

async fn run_step(
    ctx: &Ctx,
    step: UninstallStep,
    kind: &InstallKind,
    brew: Option<&Brew>,
) -> Result<String, YardError> {
    match step {
        UninstallStep::StopDaemon => {
            stop_agent(ctx).await?;
            // Unloaded is not removed: a plist left behind starts yardd again at the next login.
            let plist = agent_plist()?;
            match std::fs::remove_file(&plist) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(YardError::internal(format!("cannot delete {}: {e}", plist.display()))
                        .with_hint(format!("rm {}", plist.display())));
                }
            }
            Ok("stopped yardd and removed its LaunchAgent".into())
        }
        UninstallStep::Untrust => {
            super::trust::untrust_offline(ctx).await?;
            Ok("removed Yard's CA from the login keychain".into())
        }
        UninstallStep::HelperUninstall => {
            let exe = std::env::current_exe().map_err(|e| YardError::internal(e.to_string()))?;
            let status = Command::new("sudo")
                .arg(exe)
                .args(["helper", "uninstall"])
                .status()
                .map_err(|e| YardError::internal(format!("cannot run sudo: {e}")))?;
            if !status.success() {
                return Err(YardError::new(ErrorCode::HelperRefused, "the helper was not uninstalled")
                    .with_hint("sudo yard helper uninstall"));
            }
            Ok("uninstalled the root helper".into())
        }
        UninstallStep::RemoveData => {
            let root = ctx.paths.root();
            std::fs::remove_dir_all(root).map_err(|e| {
                YardError::internal(format!("cannot delete {}: {e}", root.display()))
                    .with_hint(format!("rm -rf {}", root.display()))
            })?;
            Ok(format!("deleted {}", root.display()))
        }
        UninstallStep::RemovePrefs => {
            let _ = Command::new("defaults").args(["delete", PREFS_DOMAIN]).output();
            if let Some(home) = std::env::var_os("HOME") {
                let _ =
                    std::fs::remove_dir_all(PathBuf::from(home).join("Library/Caches").join(PREFS_DOMAIN));
            }
            Ok("deleted Yard.app's settings".into())
        }
        UninstallStep::RemoveBundle => {
            let app = kind.app().ok_or_else(|| YardError::internal("no Yard.app to remove"))?;
            if let Some(home) = std::env::var_os("HOME") {
                let link = PathBuf::from(home).join(".local/bin/yard");
                if std::fs::read_link(&link).is_ok_and(|t| t.starts_with(app)) {
                    let _ = std::fs::remove_file(&link);
                }
            }
            // This yard runs from inside the app; deleting a running binary is safe on macOS.
            std::fs::remove_dir_all(app).map_err(|e| {
                YardError::internal(format!("cannot delete {}: {e}", app.display()))
                    .with_hint(format!("rm -rf {}", app.display()))
            })?;
            Ok(format!("deleted {}", app.display()))
        }
        UninstallStep::RemoveBrewCask => {
            let brew =
                brew.ok_or_else(|| YardError::new(ErrorCode::BrewMissing, "Homebrew is not installed"))?;
            let status = Command::new(brew.brew_bin())
                .args(["uninstall", "--cask", "yard"])
                .status()
                .map_err(|e| YardError::internal(format!("cannot run brew: {e}")))?;
            if !status.success() {
                return Err(YardError::internal("brew uninstall --cask yard failed")
                    .with_hint("brew uninstall --cask yard"));
            }
            Ok("removed Yard.app with brew uninstall --cask yard".into())
        }
    }
}

/// The hostnames of named tunnels, whose DNS records stay at Cloudflare.
fn tunnel_hosts(ctx: &Ctx) -> Vec<String> {
    yard_core::config::load_sites(&ctx.paths)
        .map(|c| c.sites.values().filter_map(|s| s.tunnel.as_ref().map(|t| t.host().to_string())).collect())
        .unwrap_or_default()
}

fn typed_uninstall(answer: &str) -> bool {
    answer.trim() == "uninstall"
}

/// Each directory under `data/` with its size in bytes, by name.
fn data_sizes(data: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(data) else { return vec![] };
    let mut sizes: Vec<(String, u64)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), dir_size(&e.path())))
        .collect();
    sizes.sort();
    sizes
}

fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|es| es.filter_map(Result::ok).map(|e| dir_size(&e.path())).sum())
        .unwrap_or(0)
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / (1u64 << 10) as f64),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_typed_word_must_be_uninstall() {
        assert!(typed_uninstall("uninstall\n"));
        assert!(typed_uninstall("  uninstall  \n"));
        assert!(!typed_uninstall("y\n"));
        assert!(!typed_uninstall("UNINSTALL\n"));
        assert!(!typed_uninstall("\n"));
    }

    #[test]
    fn data_sizes_name_each_data_directory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("data/mysql-8.4/sub")).unwrap();
        std::fs::write(root.path().join("data/mysql-8.4/sub/a"), vec![0u8; 2048]).unwrap();
        std::fs::create_dir_all(root.path().join("data/redis")).unwrap();
        std::fs::write(root.path().join("data/redis/b"), "xy").unwrap();
        let sizes = data_sizes(&root.path().join("data"));
        assert_eq!(sizes, [("mysql-8.4".to_string(), 2048), ("redis".to_string(), 2)]);
    }
}
