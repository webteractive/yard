//! Each linked project's `.yard`, read for reconcile and doctor, and what it needs that
//! this machine lacks (project-file spec §5).

use crate::emailulator::Probe;
use crate::modules;
use crate::reconcile::brew_conf_d;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use yard_core::brew::Brew;
use yard_core::config::{ServicesConfig, SitesConfig, YardConfig};
use yard_core::project::{self, IgnoredFiles, Loaded, Projects};
use yard_core::render::php::ini_loads;
use yard_core::{Paths, PhpVersion};
use yard_proto::{ErrorCode, SiteInfo, SiteKind, YardError};

/// The largest `.yard` Yard reads.
pub const PROJECT_FILE_MAX: u64 = 64 * 1024;

/// `file` if it is a regular file of at most `max` bytes. A FIFO or device in a project
/// would otherwise block the reader, and yardd's runtime with it.
pub fn read_small(file: &Path, max: u64) -> Option<String> {
    if !std::fs::metadata(file).ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(file).ok()?.take(max + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= max).then_some(text)
}

/// `None` when there is no `.yard`; an error when there is one Yard will not read,
/// including a symlink to nothing.
pub fn read_project_file(file: &Path) -> Option<Result<String, YardError>> {
    let invalid = |what: &str| {
        YardError::new(ErrorCode::ProjectFileInvalid, format!("{} {what}", file.display()))
            .with_hint(format!("replace {} with a text file", file.display()))
    };
    let meta = match std::fs::metadata(file) {
        // A symlink to nothing is still a file there: it wins over a managed `.yard`, and
        // is the site's error until it is fixed (managed-file spec §4).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && file.symlink_metadata().is_ok() => {
            return Some(Err(invalid("is a symlink to nothing")));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(Err(invalid(&format!("cannot be read: {e}")))),
        Ok(meta) => meta,
    };
    if !meta.is_file() {
        return Some(Err(invalid("is not a regular file")));
    }
    Some(read_small(file, PROJECT_FILE_MAX).ok_or_else(|| invalid("is larger than 64 KiB or not UTF-8")))
}

/// The `.yard` every linked site in `sites` is served from, and the managed files not used.
pub fn read_projects(sites: &SitesConfig, paths: &Paths) -> (Projects, IgnoredFiles) {
    project::load(sites, paths, &read_project_file)
}

/// Sets `needs` on each site served from a `.yard`. An extension is present when
/// `yard.toml` manages it for the version (on or off), brew's `conf.d` loads it, or the
/// version's PHP has it compiled in. A `[services]` entry is met by an instance in
/// `services.toml`, running or not: a stopped or failed instance is not a need. `services`
/// is `None` when `services.toml` does not parse, and then no service is a need, since an
/// instance may well exist in the file nobody could read.
pub fn fill_needs(
    sites: &mut [SiteInfo],
    projects: &Projects,
    yard: &YardConfig,
    services: Option<&ServicesConfig>,
    emailulator: Option<&Probe>,
    brew: &Brew,
    paths: &Paths,
) {
    // `None` from the probe: no binary where Yard looks. One Yard could not read is not missing:
    // it may well be catching mail, and doctor says it could not be asked.
    let mail_reachable = crate::emailulator::serving_agent(emailulator).is_some()
        || emailulator.is_some_and(|p| p.status_error.is_some());
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let installed = brew.installed_php(&exists, &read_link);
    let mut present: BTreeMap<PhpVersion, BTreeSet<String>> = BTreeMap::new();
    for site in sites {
        let Some(file) = projects.get(&site.name).and_then(Loaded::ok) else { continue };
        let SiteKind::Php { php, .. } = &site.kind else { continue };
        let Ok(v) = php.parse::<PhpVersion>() else { continue };
        let loaded = present.entry(v).or_insert_with(|| match brew.php_opt(v, &exists, &read_link) {
            Ok(opt) => {
                let mut modules = modules::compiled(paths, &opt);
                for (_, text) in brew_conf_d(&brew.php_conf_d(v)) {
                    modules.extend(ini_loads(&text).map(|(m, _)| m.to_lowercase()));
                }
                modules
            }
            Err(_) => BTreeSet::new(),
        });
        let managed = yard.extensions.get(&v);
        let has = |m: &str| {
            managed.is_some_and(|exts| exts.keys().any(|k| k.eq_ignore_ascii_case(m)))
                || loaded.contains(&m.to_lowercase())
        };
        site.needs = project::needs(file, v, &installed, &has);
        site.needs.extend(services.into_iter().flat_map(|s| project::service_needs(file, s)));
        site.needs.extend(project::emailulator_need(file, mail_reachable, emailulator.is_some()));
    }
}
