//! `yard env`: the `.env` lines for a project's services. Prints; never writes a file.

use super::{Ctx, Outcome, cwd_absolute};
use yard_core::emailulator::NAME as EMAILULATOR;
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
    print!("{}", human(&warn_notes(blocks)));
    Ok(0)
}

/// Prints emailulator's note as a `warning:` on stderr when the block still has lines (it is
/// caught but not answering), and drops it from what is pasted.
pub fn warn_notes(blocks: Vec<EnvBlock>) -> Vec<EnvBlock> {
    blocks
        .into_iter()
        .map(|mut block| {
            if block.instance == EMAILULATOR
                && !block.lines.is_empty()
                && let Some(note) = block.note.take()
            {
                eprintln!("warning: {note}");
            }
            block
        })
        .collect()
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

/// `# <instance>`, a `# note` when there is one, then the lines; blocks are separated by a
/// blank line. A block with a note and no lines is that one comment line: the note already
/// names the instance and the command that fixes it.
pub fn human(blocks: &[EnvBlock]) -> String {
    let rendered: Vec<String> = blocks
        .iter()
        .map(|block| match (&block.note, block.lines.is_empty()) {
            (Some(note), true) => format!("# {note}\n"),
            (note, _) => {
                let mut out = format!("# {}\n", block.instance);
                if let Some(note) = note {
                    out.push_str(&format!("# {note}\n"));
                }
                for line in &block.lines {
                    out.push_str(line);
                    out.push('\n');
                }
                out
            }
        })
        .collect();
    rendered.join("\n")
}
