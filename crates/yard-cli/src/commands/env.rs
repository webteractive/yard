//! `yard env`: the `.env` lines for a project's services. Prints; never writes a file.

use super::{Ctx, Outcome, cwd_absolute};
use yard_core::present::{env_text, split_env_warnings};
use yard_proto::methods::{self, EnvParams, SiteDetailParams};
use yard_proto::{EnvBlock, ErrorCode, SiteDetail};

pub async fn env(ctx: &Ctx, site: Option<String>, service: Option<String>) -> Outcome {
    // With no flag the daemon needs the directory, and it resolves a relative path against
    // its own working directory, so it gets the real absolute one.
    let path = if site.is_none() && service.is_none() { Some(cwd_absolute()?) } else { None };
    let mut client = ctx.client().await?;
    let params = EnvParams { path: path.clone(), site: site.clone(), service: service.clone() };
    let blocks: Vec<EnvBlock> = client.call(methods::ENV_PRINT, params).await?;
    if ctx.json {
        ctx.emit(&blocks, String::new);
        return Ok(0);
    }
    if blocks.is_empty() && service.is_none() {
        let params = SiteDetailParams { path, site };
        let detail: Result<SiteDetail, _> = client.call(methods::SITES_INFO, params).await;
        match detail {
            Ok(detail) => eprintln!("{}", nothing_for(&detail)),
            Err(e) if e.code == ErrorCode::SiteNotFound => {
                eprintln!(
                    "No service instances yet. Add one with yard service add <engine>: mysql, postgres, redis, minio or emailulator."
                );
            }
            Err(e) => eprintln!(
                "There are no lines to print, and yardd could not say why: {}. yard info shows the site.",
                e.message
            ),
        }
        return Ok(0);
    }
    print!("{}", env_text(&warn_notes(blocks)));
    Ok(0)
}

/// Prints emailulator's note as a `warning:` on stderr when the block still has lines (it is
/// caught but not answering), and drops it from what is pasted.
pub fn warn_notes(blocks: Vec<EnvBlock>) -> Vec<EnvBlock> {
    let (blocks, warnings) = split_env_warnings(blocks);
    for note in warnings {
        eprintln!("warning: {note}");
    }
    blocks
}

/// Why a project prints nothing, and how to get lines anyway.
fn nothing_for(detail: &SiteDetail) -> String {
    let name = &detail.site.name;
    let why = match detail.site.project_file {
        Some(_) => format!("{name}'s .yard lists no services, so there is nothing to print."),
        None => format!("{name} has no .yard, so it asks for no services and there is nothing to print."),
    };
    format!(
        "{why} yard env --service <instance> prints one instance (yard services lists them); outside a project, yard env prints every instance."
    )
}
