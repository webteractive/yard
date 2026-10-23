//! `sources.*`: the brew taps Yard installs from, and trusting or untrusting what they
//! carry. Trust is brew's: these run `brew trust` / `brew untrust` and read `brew trust
//! --json=v1` (`crate::sources`).

use crate::daemon::Context;
use crate::jobs::JobCtx;
use crate::sources;
use yard_core::config::{load_services, load_yard};
use yard_proto::methods::SourceParams;
use yard_proto::{JobStarted, SourceInfo, YardError};

pub(super) async fn list(ctx: &Context) -> Result<Vec<SourceInfo>, YardError> {
    let (yard, _) = load_yard(&ctx.paths)?;
    // A services.toml that does not parse is the config check's to say: MinIO's keg is then
    // told apart by asking brew.
    let services = load_services(&ctx.paths).ok();
    sources::list(&ctx.brew, &yard, services.as_ref()).await
}

/// `sources.trust` (`grant`) and `sources.untrust`, as jobs. The target is checked
/// first: brew would read `--tap` as a flag.
pub(super) fn change(ctx: &Context, p: SourceParams, grant: bool) -> Result<JobStarted, YardError> {
    sources::check_target(&p.target)?;
    let brew = ctx.brew.clone();
    let kind = if grant { "source-trust" } else { "source-untrust" };
    Ok(ctx.jobs.start(kind, move |job: JobCtx| async move {
        if grant {
            sources::trust(&job, &brew, &p.target, p.tap).await
        } else {
            sources::untrust(&job, &brew, &p.target, p.tap).await
        }
    }))
}
