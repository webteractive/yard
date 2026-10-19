//! php.ini and the php-fpm pool for one version (spec §4.2, §4.4).

use super::quote::ini_str;
use super::{RenderCtx, ini_header};
use crate::PhpVersion;

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
{decorate}",
        header = ini_header(&paths.php_overrides(v)),
        pid = ini_str(&paths.run_dir().join(format!("php-{v}.pid")).display().to_string()),
        socket = ini_str(&paths.fpm_socket(v).display().to_string()),
    )
}

/// Brew's conf.d first, so extension inis load; Yard's last, so `99-overrides.ini` wins.
pub fn php_fpm_env(ctx: &RenderCtx, v: PhpVersion) -> Vec<(String, String)> {
    vec![(
        "PHP_INI_SCAN_DIR".to_string(),
        format!("{}:{}", ctx.brew.php_conf_d(v).display(), ctx.paths.php_conf_d(v).display()),
    )]
}
