//! Shell-word splitting for hints (`yard node install 'lts/*'`): whitespace, single and double
//! quotes, backslash escapes. Nothing is expanded; this only reads what a hint says.

/// The words of `text`, or `None` when a quote is not closed.
pub fn split_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => word.push(chars.next()?),
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.push(chars.next()?);
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_quotes_and_escapes() {
        let w = |s: &str| split_words(s).map(|v| v.join("|"));
        assert_eq!(w("yard  php install 8.4"), Some("yard|php|install|8.4".into()));
        assert_eq!(w("yard node install 'lts/*'"), Some("yard|node|install|lts/*".into()));
        assert_eq!(w(r#"yard service start "a b""#), Some("yard|service|start|a b".into()));
        assert_eq!(w(r"a\ b 'it'\''s'"), Some("a b|it's".into()));
        assert_eq!(w("yard php install 'unclosed"), None);
        assert_eq!(w(""), Some(String::new()));
    }
}
