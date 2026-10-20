//! `ext.*` and `xdebug.set`: listing and toggling extensions in `yard.toml`, and checking
//! an add or remove before it runs as a job (`crate::ext`).

use super::handlers::reconcile;
use super::php::config_version;
use crate::daemon::Context;
use crate::ext::{Add, Remove};
use crate::reconcile::brew_conf_d;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use toml_edit::{InlineTable, value};
use yard_core::PhpVersion;
use yard_core::config::yard::{ExtEntry, ExtSource, valid_ext_name};
use yard_core::config::{TomlFile, load_yard};
use yard_core::render::php::ini_modules;
use yard_proto::methods::{ExtAddParams, ExtListParams, ExtParams, XdebugParams};
use yard_proto::{ErrorCode, ExtInfo, JobStarted, YardError};

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

fn not_found(name: &str, v: PhpVersion) -> YardError {
    YardError::new(ErrorCode::ExtNotFound, format!("{name} is not added for PHP {v}"))
        .with_hint(format!("yard ext add {name} --php {v}"))
}

/// The extensions `yard.toml` manages for `v`.
fn managed(ctx: &Context, v: PhpVersion) -> Result<BTreeMap<String, ExtEntry>, YardError> {
    Ok(load_yard(&ctx.paths)?.0.extensions.remove(&v).unwrap_or_default())
}

/// Modules brew's `conf.d` loads for `v` that `exts` does not manage.
fn bundled(ctx: &Context, v: PhpVersion, exts: &BTreeMap<String, ExtEntry>) -> BTreeSet<String> {
    brew_conf_d(&ctx.brew.php_conf_d(v))
        .iter()
        .flat_map(|(_, text)| ini_modules(text).collect::<Vec<_>>())
        .filter(|m| !exts.keys().any(|n| n.eq_ignore_ascii_case(m)))
        .collect()
}

pub(super) fn list(ctx: &Context, p: ExtListParams) -> Result<Vec<ExtInfo>, YardError> {
    let v = config_version(ctx, p.php.as_deref())?;
    let exts = managed(ctx, v)?;
    let mut rows: Vec<ExtInfo> = exts
        .iter()
        .map(|(name, e)| ExtInfo { name: name.clone(), source: e.source.name().into(), enabled: e.enabled })
        .collect();
    rows.extend(bundled(ctx, v, &exts).into_iter().map(|name| ExtInfo {
        name,
        source: ExtSource::Bundled.name().into(),
        enabled: true,
    }));
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// `ext.enable` and `ext.disable`. A bundled module is disabled by an entry that says so,
/// and enabled again by dropping it.
pub(super) async fn toggle(ctx: &Context, p: ExtParams, on: bool) -> Result<(), YardError> {
    let v = config_version(ctx, p.php.as_deref())?;
    let exts = managed(ctx, v)?;
    let version = v.to_string();
    let keys = ["php", version.as_str(), "extensions", p.name.as_str()];
    let mut file = TomlFile::load(&ctx.paths.yard_toml())?;
    match exts.get(&p.name) {
        Some(e) if e.source == ExtSource::Bundled && on => {
            file.remove(&keys);
        }
        Some(_) => {
            let item = &file.doc()["php"][version.as_str()]["extensions"][p.name.as_str()];
            match item.as_inline_table().cloned() {
                Some(mut table) => {
                    table.insert("enabled", on.into());
                    table.fmt();
                    file.set(&keys, value(table));
                }
                None => file.set(&[keys.as_slice(), &["enabled"]].concat(), value(on)),
            }
        }
        None if valid_ext_name(&p.name) && bundled(ctx, v, &exts).contains(&p.name) => {
            if on {
                return Ok(());
            }
            let mut table = InlineTable::new();
            table.insert("source", "bundled".into());
            table.insert("enabled", false.into());
            file.set(&keys, value(table));
        }
        None => return Err(not_found(&p.name, v)),
    }
    file.save()?;
    // Reconcile rewrites conf.d and restarts the version's pool if it runs.
    reconcile(ctx).await
}

pub(super) async fn xdebug(ctx: &Context, p: XdebugParams) -> Result<(), YardError> {
    toggle(ctx, ExtParams { name: "xdebug".into(), php: p.php }, p.on).await
}

pub(super) fn remove(ctx: &Context, p: ExtParams) -> Result<JobStarted, YardError> {
    let v = config_version(ctx, p.php.as_deref())?;
    let Some(entry) = managed(ctx, v)?.remove(&p.name) else {
        return Err(not_found(&p.name, v));
    };
    if entry.source == ExtSource::Bundled {
        return Err(bad(format!("{} comes with PHP, so it cannot be removed", p.name))
            .with_hint(format!("yard ext disable {} --php {v}", p.name)));
    }
    let exists = |p: &Path| p.exists();
    let read_link = |p: &Path| std::fs::read_link(p).ok();
    let opt = ctx.brew.php_opt(v, &exists, &read_link)?;
    let remove = Remove {
        paths: ctx.paths.clone(),
        brew: ctx.brew.clone(),
        reconciler: ctx.reconciler.clone(),
        v,
        opt,
        module: p.name,
        entry,
    };
    Ok(ctx.jobs.start("ext-remove", move |job| remove.run(job)))
}
