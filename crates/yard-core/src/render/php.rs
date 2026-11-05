//! php.ini and the php-fpm pool for one version (spec §4.2, §4.4).

use super::quote::ini_str;
use super::{RenderCtx, ini_header};
use crate::PhpVersion;
use crate::config::yard::{ExtEntry, ExtSource};
use std::collections::BTreeMap;

/// FPM's ini. The CLI SAPI forces `max_execution_time = 0` itself, so 300 here only
/// limits web requests.
pub fn php_ini(ctx: &RenderCtx, v: PhpVersion) -> String {
    format!(
        "{header}memory_limit = 512M
upload_max_filesize = 1G
post_max_size = 1G
max_execution_time = 300
display_errors = On
error_reporting = E_ALL
opcache.enable = 1
opcache.validate_timestamps = 1
opcache.revalidate_freq = 0
date.timezone = {tz}
",
        header = ini_header(&ctx.paths.php_overrides(v)),
        tz = ini_str(ctx.timezone),
    )
}

pub fn fpm_conf(ctx: &RenderCtx, v: PhpVersion) -> String {
    let paths = ctx.paths;
    // decorate_workers_output arrived in PHP 7.3; older pools refuse to start with it.
    let first_with_decorate = PhpVersion { major: 7, minor: 3 };
    let decorate = if v >= first_with_decorate { "decorate_workers_output = no\n" } else { "" };
    format!(
        "{header}[global]
pid = {pid}
error_log = /dev/stderr
daemonize = no

[yard]
listen = {socket}
listen.mode = 0600
pm = ondemand
pm.max_children = 10
pm.process_idle_timeout = 30s
catch_workers_output = yes
{decorate}; libpq looks for Kerberos credentials on every connect unless told not to, which can
; crash a worker inside CoreFoundation: a forked php-fpm child is not safe to use it.
env[PGGSSENCMODE] = disable
",
        header = ini_header(&paths.php_overrides(v)),
        pid = ini_str(&paths.run_dir().join(format!("php-{v}.pid")).display().to_string()),
        socket = ini_str(&paths.fpm_socket(v).display().to_string()),
    )
}

/// Only Yard's conf.d: brew's inis reach PHP as mirrored copies (`brew_inis`), so an
/// extension Yard manages or disables is never loaded behind its back.
pub fn php_fpm_env(ctx: &RenderCtx, v: PhpVersion) -> Vec<(String, String)> {
    vec![("PHP_INI_SCAN_DIR".to_string(), ctx.paths.php_conf_d(v).display().to_string())]
}

/// Brew's `conf.d` files, as `(file name, text)`, mirrored into Yard's conf.d as
/// `10-brew-<file>`. A file that loads an extension `exts` manages, or a bundled one it
/// disables, is left out: Yard's `50-<name>.ini` or nothing loads it instead.
pub fn brew_inis(
    ctx: &RenderCtx,
    files: &[(String, String)],
    exts: &BTreeMap<String, ExtEntry>,
) -> BTreeMap<String, String> {
    let header = ini_header(&ctx.paths.yard_toml());
    files
        .iter()
        .filter(|(_, text)| {
            !ini_modules(text).any(|m| {
                exts.iter().any(|(n, e)| {
                    n.eq_ignore_ascii_case(&m) && (e.source != ExtSource::Bundled || !e.enabled)
                })
            })
        })
        .map(|(name, text)| (format!("10-brew-{name}"), format!("{header}{text}")))
        .collect()
}

/// The modules an ini's `extension=` and `zend_extension=` lines load: each value's
/// basename, without `.so` and a leading `php_`.
pub fn ini_modules(text: &str) -> impl Iterator<Item = String> + '_ {
    ini_loads(text).map(|(module, _)| module)
}

/// Whether an ini loads a `.so` by absolute path that does not exist. `brew uninstall` of
/// an extension formula removes its keg but leaves its ini in brew's conf.d.
pub fn is_stale(text: &str, exists: &dyn Fn(&std::path::Path) -> bool) -> bool {
    ini_loads(text).any(|(_, value)| value.starts_with('/') && !exists(std::path::Path::new(value)))
}

/// Each `extension=`/`zend_extension=` line of an ini as (module, the value as written).
pub fn ini_loads(text: &str) -> impl Iterator<Item = (String, &str)> + '_ {
    text.lines().filter_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        if !matches!(key.trim(), "extension" | "zend_extension") {
            return None;
        }
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        let base = value.rsplit('/').next().unwrap_or(value);
        let base = base.strip_suffix(".so").unwrap_or(base);
        Some((base.strip_prefix("php_").unwrap_or(base).to_string(), value))
    })
}

/// A PEAR user config (pecl's `-c <file>`) that sets only `php_ini`. brew's `pear.conf`
/// names brew's php.ini, and pecl adds `extension=` to it after an install. `-d`/`-D`
/// cannot stop that: they store the value under pecl's channel, and the install reads
/// it for `pear.php.net`, which only a config file's top-level key sets. The user layer
/// is read before the system one, so this wins and leaves every other setting brew's.
pub fn pecl_user_config(php_ini: &std::path::Path) -> String {
    let path = php_ini.display().to_string();
    // PHP's serialize: string lengths are in bytes.
    format!("#PEAR_Config 0.9\na:1:{{s:7:\"php_ini\";s:{}:\"{path}\";}}", path.len())
}

/// `50-<name>.ini` for one extension; `None` when nothing should load it (disabled, or
/// bundled, which PHP's own ini or brew's mirrored one loads). A `brew` extension loads
/// the path its formula's ini in brew's conf.d (`brew_files`) names: shivammathur's
/// formulas keep the `.so` in their own keg, outside `extension_dir`.
pub fn ext_ini(ctx: &RenderCtx, name: &str, e: &ExtEntry, brew_files: &[(String, String)]) -> Option<String> {
    if !e.enabled || e.source == ExtSource::Bundled {
        return None;
    }
    let key = if matches!(name, "xdebug" | "opcache") { "zend_extension" } else { "extension" };
    let from_brew = || {
        brew_files
            .iter()
            .flat_map(|(_, text)| ini_loads(text).collect::<Vec<_>>())
            .find(|(module, _)| module.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.to_string())
    };
    let value = match (&e.source, &e.so) {
        (_, Some(so)) => so.display().to_string(),
        (ExtSource::Brew, None) => from_brew().unwrap_or_else(|| format!("{name}.so")),
        _ => format!("{name}.so"),
    };
    Some(format!(
        "{header}{key} = {value}\n",
        header = ini_header(&ctx.paths.yard_toml()),
        value = ini_str(&value)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pecl_user_config_sets_only_php_ini() {
        let text = pecl_user_config(std::path::Path::new("/r/Application Support/php/8.4/pecl-php.ini"));
        assert_eq!(
            text,
            "#PEAR_Config 0.9\na:1:{s:7:\"php_ini\";s:43:\"/r/Application Support/php/8.4/pecl-php.ini\";}"
        );
    }
    use crate::Paths;
    use crate::brew::Brew;
    use crate::config::yard::{ExtEntry, ExtSource};
    use insta::assert_snapshot;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn parts() -> (Paths, Brew) {
        (Paths::new("/r"), Brew::new("/opt/homebrew"))
    }

    fn ctx<'a>(paths: &'a Paths, brew: &'a Brew) -> RenderCtx<'a> {
        RenderCtx { paths, brew, tld: "yard", http_port: 8080, https_port: 8443, timezone: "UTC" }
    }

    fn v84() -> PhpVersion {
        "8.4".parse().unwrap()
    }

    fn entry(source: ExtSource, enabled: bool) -> ExtEntry {
        ExtEntry { source, enabled, so: None }
    }

    fn brew_files() -> Vec<(String, String)> {
        vec![
            ("ext-opcache.ini".into(), "[opcache]\nzend_extension=\"opcache.so\"\n".into()),
            ("20-redis.ini".into(), "[redis]\nextension=\"redis.so\"\n".into()),
        ]
    }

    #[test]
    fn brew_conf_d_inis_are_mirrored_except_managed() {
        let (p, b) = parts();
        let exts = BTreeMap::from([("redis".to_string(), entry(ExtSource::Brew, true))]);
        let out = brew_inis(&ctx(&p, &b), &brew_files(), &exts);
        assert_eq!(out.keys().collect::<Vec<_>>(), ["10-brew-ext-opcache.ini"]);
        let text = &out["10-brew-ext-opcache.ini"];
        assert!(text.starts_with("; Generated by Yard — edit /r/yard.toml instead\n"), "{text}");
        assert!(text.ends_with("[opcache]\nzend_extension=\"opcache.so\"\n"), "{text}");

        let mut exts = exts;
        exts.insert("opcache".into(), entry(ExtSource::Bundled, false));
        assert!(brew_inis(&ctx(&p, &b), &brew_files(), &exts).is_empty());
    }

    #[test]
    fn brew_inis_match_module_names_loosely() {
        let (p, b) = parts();
        let files = vec![
            ("a.ini".to_string(), "; zend_extension=xdebug.so\nextension = /x/php_Redis.so\n".to_string()),
            ("b.ini".to_string(), "extension=imagick\n".to_string()),
            ("notes.txt".to_string(), "extension=redis.so\n".to_string()),
        ];
        let exts = BTreeMap::from([
            ("redis".to_string(), entry(ExtSource::Pecl, true)),
            ("xdebug".to_string(), entry(ExtSource::Brew, true)),
        ]);
        let out = brew_inis(&ctx(&p, &b), &files, &exts);
        // A commented line loads nothing; a bare name and a non-ini file count too.
        assert_eq!(out.keys().collect::<Vec<_>>(), ["10-brew-b.ini"]);
    }

    #[test]
    fn ext_ini_snapshots() {
        let (p, b) = parts();
        let c = ctx(&p, &b);
        assert_snapshot!("brew_redis", ext_ini(&c, "redis", &entry(ExtSource::Brew, true), &[]).unwrap());
        let custom = ExtEntry {
            source: ExtSource::Path,
            enabled: true,
            so: Some(PathBuf::from("/Users/me/Library/Application Support/Yard/php/8.4/ext/custom.so")),
        };
        assert_snapshot!("path_custom", ext_ini(&c, "custom", &custom, &[]).unwrap());
        assert_snapshot!("brew_xdebug", ext_ini(&c, "xdebug", &entry(ExtSource::Brew, true), &[]).unwrap());
        assert_eq!(ext_ini(&c, "redis", &entry(ExtSource::Brew, false), &[]), None);
        assert_eq!(ext_ini(&c, "opcache", &entry(ExtSource::Bundled, true), &[]), None);
    }

    #[test]
    fn brew_ext_loads_from_the_path_brews_ini_names() {
        let (p, b) = parts();
        let c = ctx(&p, &b);
        // shivammathur's formulas keep the .so in their own keg, outside extension_dir.
        let files = vec![(
            "20-redis.ini".to_string(),
            "[redis]\nextension=\"/opt/homebrew/opt/phpredis@7.1/redis.so\"\n".to_string(),
        )];
        let ini = ext_ini(&c, "redis", &entry(ExtSource::Brew, true), &files).unwrap();
        assert!(ini.ends_with("extension = \"/opt/homebrew/opt/phpredis@7.1/redis.so\"\n"), "{ini}");
        // pecl installs into extension_dir, so its bare name stays.
        let ini = ext_ini(&c, "redis", &entry(ExtSource::Pecl, true), &files).unwrap();
        assert!(ini.ends_with("extension = \"redis.so\"\n"), "{ini}");
    }

    #[test]
    fn an_ini_whose_so_is_gone_is_stale() {
        let ini = "[redis]\nextension=\"/opt/homebrew/opt/phpredis@7.1/redis.so\"\n";
        assert!(is_stale(ini, &|_| false), "brew uninstall leaves the ini, not the .so");
        assert!(!is_stale(ini, &|_| true));
        assert!(!is_stale("zend_extension=\"opcache.so\"\n", &|_| false), "a bare name is PHP's to find");
    }

    #[test]
    fn scan_dir_is_yards_only() {
        let (p, b) = parts();
        assert_eq!(
            php_fpm_env(&ctx(&p, &b), v84()),
            vec![("PHP_INI_SCAN_DIR".to_string(), "/r/php/8.4/conf.d".to_string())]
        );
    }
}
