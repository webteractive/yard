//! The script a Terminal hand-off runs (M6 design §10.3): every argument single-quoted, never the
//! hint's own text.

/// `#!/bin/zsh`, the command with every argument single-quoted (`'` as `'\''`), then a wait for a
/// key so the window stays until the user has read it.
pub fn terminal_script(args: &[String]) -> String {
    let quoted: Vec<String> = args.iter().map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect();
    format!("#!/bin/zsh\n{}\nread -k1 '?Press a key to close.'\n", quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_script_quotes_every_argument() {
        let args: Vec<String> =
            ["sudo", "/Users/a b/yard's/yard", "helper", "install"].iter().map(|s| s.to_string()).collect();
        let script = terminal_script(&args);
        assert!(script.starts_with("#!/bin/zsh\n"), "{script}");
        assert!(script.contains("'sudo' '/Users/a b/yard'\\''s/yard' 'helper' 'install'"), "{script}");
        assert!(script.ends_with("read -k1 '?Press a key to close.'\n"), "{script}");
    }
}
