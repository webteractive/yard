//! Quoting for config values. The data root contains a space, so every path is quoted.

/// A double-quoted nginx string, escaping `"` and `\`.
pub fn nginx_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A double-quoted INI value (php.ini, php-fpm). INI strings cannot escape `"`.
pub fn ini_str(s: &str) -> String {
    format!("\"{s}\"")
}

pub fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
