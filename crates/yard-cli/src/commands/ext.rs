//! `yard ext`: PHP extensions.

use super::{Ctx, Outcome, cwd, run_job};
use std::path::Path;
use yard_proto::methods::{self, ExtAddParams};

pub async fn add(ctx: &Ctx, name: String, php: Option<String>) -> Outcome {
    // yardd runs elsewhere, so a .so path is sent absolute.
    let name = if name.ends_with(".so") && !Path::new(&name).is_absolute() {
        cwd()?.join(&name).display().to_string()
    } else {
        name
    };
    let stem = Path::new(&name).file_stem().map_or(name.clone(), |s| s.to_string_lossy().into_owned());
    let module = stem.strip_prefix("php_").unwrap_or(&stem);
    let shown = php.as_ref().map_or_else(|| "the default PHP".to_string(), |v| format!("PHP {v}"));
    let done = format!("Added {module} to {shown}.\n");
    run_job(ctx, methods::EXT_ADD, ExtAddParams { name, php }, || done).await
}
