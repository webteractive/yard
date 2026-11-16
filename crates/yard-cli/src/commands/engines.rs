//! `yard engines`, `yard engines check <file>` and `yard engines install <key>` (recipes spec §9).

use super::{Ctx, Outcome, run_job};
use crate::output::{for_agents, table};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use yard_core::services::engines::{
    InstanceCtx, LoadOptions, Origin, ServiceEngine, SourceSpec, parse_recipe,
};
use yard_proto::methods::{self, EngineInstallParams};
use yard_proto::{EngineOrigin, EngineSourceKind, EnginesListResult, ErrorCode, PortRef, YardError};

pub async fn list(ctx: &Ctx) -> Outcome {
    let list: EnginesListResult = ctx.client().await?.call(methods::ENGINES_LIST, Value::Null).await?;
    ctx.emit(&list, || human(&list));
    Ok(0)
}

fn human(list: &EnginesListResult) -> String {
    let rows: Vec<Vec<String>> = list
        .engines
        .iter()
        .map(|e| {
            let source = match e.source {
                EngineSourceKind::Brew => "brew",
                EngineSourceKind::Path => "path",
                EngineSourceKind::Download => "download",
                EngineSourceKind::Npm => "npm",
                EngineSourceKind::Other => "unknown",
            };
            let source = match &e.version {
                Some(v) => format!("{source} {v}"),
                None => source.to_string(),
            };
            let origin = match &e.origin {
                EngineOrigin::BuiltIn => "built-in".to_string(),
                EngineOrigin::Carried => "carried".to_string(),
                EngineOrigin::User { file } => file.clone(),
                EngineOrigin::Other => "unknown".to_string(),
            };
            let installed = if e.installed { "yes" } else { "no" };
            vec![e.key.clone(), e.name.clone(), source, origin, installed.to_string()]
        })
        .collect();
    let mut out = table(&["ENGINE", "NAME", "SOURCE", "ORIGIN", "INSTALLED"], &rows);
    let errors: Vec<&YardError> =
        list.engines.iter().filter_map(|e| e.error.as_ref()).chain(&list.errors).collect();
    if !errors.is_empty() {
        out.push('\n');
        for e in errors {
            out.push_str(&format!("error: {}\n", e.message));
        }
    }
    out.push('\n');
    out.push_str("Add one with yard service add <engine>; write your own in ~/.yard/engines/<key>.toml.\n");
    out.push_str(&for_agents("yard engines"));
    out
}

/// What `engines check` found: the instance a recipe would make, with its usual ports.
#[derive(Serialize)]
struct Checked {
    key: String,
    name: String,
    source: String,
    ports: Vec<PortRef>,
    command: Vec<String>,
    env: Vec<String>,
    console: Option<String>,
}

/// Loads a recipe file with no daemon and says what it would run, or its first error (exit 1).
pub fn check(ctx: &Ctx, file: PathBuf) -> Outcome {
    let text = std::fs::read_to_string(&file).map_err(|e| {
        YardError::new(ErrorCode::InvalidArgument, format!("cannot read {}: {e}", file.display()))
    })?;
    let recipe = parse_recipe(&text, &file, Origin::User(file.clone()), &LoadOptions::default())
        .map_err(|e| e.to_yard_error())?;
    let key = recipe.key().to_string();
    let ports: Vec<PortRef> = recipe
        .ports()
        .into_iter()
        .map(|p| PortRef {
            name: if p.suffix.is_empty() { key.clone() } else { format!("{key}-{}", p.suffix) },
            port: p.usual,
        })
        .collect();
    let (source, home) = match recipe.source() {
        SourceSpec::Brew { formula } => {
            let keg = Path::new("/opt/homebrew/opt").join(formula.rsplit('/').next().unwrap_or(&formula));
            (format!("brew {formula}"), keg)
        }
        SourceSpec::BrewLines(spec) => {
            (format!("brew {}", spec.base_formula), PathBuf::from("/opt/homebrew/opt"))
        }
        SourceSpec::Path { program } => (
            format!("path {}", program.display()),
            program.parent().map(Path::to_path_buf).unwrap_or_default(),
        ),
        SourceSpec::Download { version, url, .. } => {
            (format!("download {version} from {url}"), ctx.paths.program_root(&key, &version))
        }
        SourceSpec::Npm { package, version, node } => {
            (format!("npm {package}@{version} on Node {node}"), ctx.paths.program_root(&key, &version))
        }
    };
    let node = PathBuf::from("<node>");
    let ictx = InstanceCtx {
        instance: &key,
        home: &home,
        paths: &ctx.paths,
        ports: &ports,
        macos_user: "user",
        node: matches!(recipe.source(), SourceSpec::Npm { .. }).then_some(node.as_path()),
    };
    let cmd = recipe.command(&ictx);
    let checked = Checked {
        name: recipe.display().to_string(),
        source,
        command: std::iter::once(cmd.program.display().to_string())
            .chain(cmd.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect(),
        env: recipe.env_lines(&ictx),
        console: recipe.console().map(|_| format!("{key}.yard")),
        ports,
        key,
    };
    ctx.emit(&checked, || {
        let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut out = format!(
            "{file_name} loads: {} ({}), from {}\n\nPorts\n",
            checked.name, checked.key, checked.source
        );
        for p in &checked.ports {
            out.push_str(&format!("  {}  {}\n", p.name, p.port));
        }
        out.push_str(&format!("\nRuns\n  {}\n", checked.command.join(" ")));
        if !checked.env.is_empty() {
            out.push_str("\nyard env\n");
            for line in &checked.env {
                out.push_str(&format!("  {line}\n"));
            }
        }
        if let Some(console) = &checked.console {
            out.push_str(&format!("\nConsole\n  {console}\n"));
        }
        out.push_str(&format!(
            "\nCopy it to ~/.yard/engines/{}.toml, then yard service add {}.\n",
            checked.key, checked.key
        ));
        out
    });
    Ok(0)
}

pub async fn install(ctx: &Ctx, key: String) -> Outcome {
    let done = format!("Installed {key}.\n");
    run_job(ctx, methods::ENGINES_INSTALL, EngineInstallParams { key }, || done).await
}
