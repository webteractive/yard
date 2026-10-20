//! `ext.*`: checks a request, then runs it as a job (`crate::ext`).

use super::php::config_version;
use crate::daemon::Context;
use crate::ext::Add;
use std::path::{Path, PathBuf};
use yard_core::config::load_yard;
use yard_core::config::yard::valid_ext_name;
use yard_proto::methods::ExtAddParams;
use yard_proto::{ErrorCode, JobStarted, YardError};

fn bad(message: String) -> YardError {
    YardError::new(ErrorCode::InvalidArgument, message)
}

pub(super) fn add(ctx: &Context, p: ExtAddParams) -> Result<JobStarted, YardError> {
    let v = config_version(ctx, p.php.as_deref())?;
    let (module, so) = match p.name.strip_suffix(".so") {
        Some(_) => {
            let path = PathBuf::from(&p.name);
            if !path.is_absolute() || !path.is_file() {
                return Err(bad(format!("{} is not a file", p.name))
                    .with_hint("pass the absolute path of a built .so"));
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            (stem.strip_prefix("php_").unwrap_or(stem).to_string(), Some(path))
        }
        None => (p.name.clone(), None),
    };
    if !valid_ext_name(&module) {
        return Err(bad(format!("{module:?} is not an extension name: use letters, digits and _"))
            .with_hint("yard ext add <name> or yard ext add /path/to/<name>.so"));
    }
    let (yard, _) = load_yard(&ctx.paths)?;
    if yard.extensions.get(&v).is_some_and(|exts| exts.contains_key(&module)) {
        return Err(bad(format!("{module} is already added for PHP {v}"))
            .with_hint(format!("yard ext enable {module} --php {v}")));
    }
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let opt = ctx.brew.php_opt(v, &exists, &read_link)?;
    let add = Add {
        paths: ctx.paths.clone(),
        brew: ctx.brew.clone(),
        reconciler: ctx.reconciler.clone(),
        v,
        opt,
        module,
        so,
    };
    Ok(ctx.jobs.start("ext-add", move |job| add.run(job)))
}
