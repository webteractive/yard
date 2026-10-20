//! Editing `99-overrides.ini`, the user's php.ini overrides (spec §4.4), line by line so
//! comments and everything else in it stay as the user wrote them.

/// `(key, value)` of a `key = value` line; `None` for comments, sections and the rest.
fn entry(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with(';') || line.starts_with('#') || line.starts_with('[') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn is_key(line: &str, key: &str) -> bool {
    entry(line).is_some_and(|(k, _)| k == key)
}

/// The value of the last uncommented `key = …`, without surrounding quotes.
pub fn get(text: &str, key: &str) -> Option<String> {
    let (_, value) = text.lines().rev().filter_map(entry).find(|(k, _)| *k == key)?;
    let unquoted = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
    Some(unquoted.to_string())
}

/// Sets `key` where its last line is, dropping earlier ones, or appends it.
pub fn set(text: &str, key: &str, value: &str) -> String {
    let needs_quotes = value.contains(|c: char| c.is_whitespace() || c == ';' || c == '#' || c == '=');
    let line = if needs_quotes { format!("{key} = \"{value}\"") } else { format!("{key} = {value}") };
    let lines: Vec<&str> = text.lines().collect();
    let last = lines.iter().rposition(|l| is_key(l, key));
    let mut out = String::with_capacity(text.len() + line.len() + 1);
    for (i, l) in lines.iter().enumerate() {
        if Some(i) == last {
            out.push_str(&line);
            out.push('\n');
        } else if !is_key(l, key) {
            out.push_str(l);
            out.push('\n');
        }
    }
    if last.is_none() {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Removes every `key = …` line; the flag says whether there was one.
pub fn unset(text: &str, key: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut removed = false;
    for l in text.lines() {
        if is_key(l, key) {
            removed = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    (out, removed)
}

/// php.ini directive names: letters, digits, `_` and `.`.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// One line, and no `"`: php.ini has no escape for it inside a quoted value.
pub fn valid_value(value: &str) -> bool {
    !value.contains(['\n', '\r', '\0', '"'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_in_place_and_keeps_comments() {
        let text = "; mine\nmemory_limit = 1G\nx = 1\n";
        assert_eq!(set(text, "memory_limit", "2G"), "; mine\nmemory_limit = 2G\nx = 1\n");
    }

    #[test]
    fn set_drops_earlier_duplicates() {
        let text = "a = 1\nb = 2\na=3\n";
        assert_eq!(set(text, "a", "4"), "b = 2\na = 4\n");
    }

    #[test]
    fn set_appends_when_absent() {
        assert_eq!(set("", "memory_limit", "1G"), "memory_limit = 1G\n");
        assert_eq!(set("; USER-OWNED", "x", "1"), "; USER-OWNED\nx = 1\n");
        assert_eq!(
            set("; c\n;memory_limit = 1G\n", "memory_limit", "2G"),
            "; c\n;memory_limit = 1G\nmemory_limit = 2G\n"
        );
    }

    #[test]
    fn unset_removes_every_occurrence() {
        assert_eq!(unset("a = 1\n; a = 2\nb = 2\na = 3\n", "a"), ("; a = 2\nb = 2\n".to_string(), true));
        assert_eq!(unset("b = 2\n", "a"), ("b = 2\n".to_string(), false));
    }

    #[test]
    fn get_ignores_commented_lines_and_strips_quotes() {
        let text = "; memory_limit = 9G\n#memory_limit = 8G\nmemory_limit = 1G\nmemory_limit = \"2G\"\n";
        assert_eq!(get(text, "memory_limit").as_deref(), Some("2G"));
        assert_eq!(get(text, "memory"), None);
        assert_eq!(get("[PHP]\nx=1\n", "x").as_deref(), Some("1"));
    }

    #[test]
    fn values_with_spaces_or_semicolons_are_quoted() {
        assert_eq!(set("", "error_log", "/tmp/a b.log"), "error_log = \"/tmp/a b.log\"\n");
        assert_eq!(set("", "x", "a;b"), "x = \"a;b\"\n");
        assert_eq!(get(&set("", "error_log", "/tmp/a b.log"), "error_log").as_deref(), Some("/tmp/a b.log"));
    }

    #[test]
    fn invalid_keys_and_values_are_rejected() {
        for ok in ["memory_limit", "xdebug.mode", "opcache.jit_buffer_size"] {
            assert!(valid_key(ok), "{ok}");
        }
        for bad in ["", "a b", "a=b", "a\nb", "[x]", ";x"] {
            assert!(!valid_key(bad), "{bad:?}");
        }
        assert!(valid_value("512M") && valid_value("") && valid_value("/a b"));
        for bad in ["a\nb", "a\0b", "a\"b", "a\rb"] {
            assert!(!valid_value(bad), "{bad:?}");
        }
    }
}
