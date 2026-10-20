//! Composer's `require.php` constraints, judged at PHP's major.minor (spec §4.5): Yard
//! picks versions, not patch releases, so `>=7.1.3` is met by 7.1.

use crate::PhpVersion;

/// Whether `v` meets `constraint`: `*`, `7.1`/`7.1.*`, `^`, `~`, `>=`, `>`, `<=`, `<`,
/// `!=`, terms joined by spaces or commas (and) and `||`/`|` (or). `None` for anything
/// else, such as `dev-main`, so the caller can stay quiet about what it cannot read.
pub fn satisfies(constraint: &str, v: PhpVersion) -> Option<bool> {
    let mut any = false;
    for alternative in constraint.split('|').filter(|a| !a.trim().is_empty()) {
        let mut all = true;
        let mut terms = 0;
        for term in alternative.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()) {
            all &= term_holds(term, v)?;
            terms += 1;
        }
        if terms == 0 {
            return None;
        }
        any |= all;
    }
    constraint.contains(|c: char| !c.is_whitespace() && c != '|').then_some(any)
}

/// A version in a constraint: major, minor if given, and whether a patch was given.
struct Spec {
    major: u8,
    minor: Option<u8>,
    patch: bool,
}

fn spec(text: &str) -> Option<Spec> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let major = parts.next()?.parse().ok()?;
    let wild = |p: &str| matches!(p, "*" | "x" | "X");
    let minor = match parts.next() {
        None => None,
        Some(p) if wild(p) => None,
        Some(p) => Some(p.parse().ok()?),
    };
    let patch = match parts.next() {
        None => false,
        Some(p) if wild(p) => false,
        Some(p) => {
            p.parse::<u16>().ok()?;
            true
        }
    };
    parts.next().is_none().then_some(Spec { major, minor, patch })
}

fn term_holds(term: &str, v: PhpVersion) -> Option<bool> {
    if term == "*" {
        return Some(true);
    }
    let ops = [">=", "<=", "!=", "==", ">", "<", "=", "^", "~"];
    let (op, rest) = ops.iter().find_map(|op| term.strip_prefix(op).map(|r| (*op, r))).unwrap_or(("", term));
    let s = spec(rest)?;
    let at = (v.major, v.minor);
    let low = (s.major, s.minor.unwrap_or(0));
    let same = v.major == s.major && s.minor.is_none_or(|m| m == v.minor);
    Some(match op {
        "" | "=" | "==" => same,
        "!=" => !same || s.patch,
        "^" => at >= low && v.major == s.major,
        "~" => match (s.minor, s.patch) {
            (Some(m), true) => at == (s.major, m),
            _ => at >= low && v.major == s.major,
        },
        ">=" => at >= low,
        // `>7.1.3` is met by a later 7.1 patch; `>7.1` only counts later minors here.
        ">" => {
            if s.patch {
                at >= low
            } else if s.minor.is_none() {
                v.major > s.major
            } else {
                at > low
            }
        }
        "<=" => {
            if s.minor.is_none() {
                v.major <= s.major
            } else {
                at <= low
            }
        }
        "<" => {
            if s.patch {
                at <= low
            } else {
                at < low
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraint_table() {
        let v = |s: &str| s.parse::<PhpVersion>().unwrap();
        for (constraint, version, expected) in [
            ("^7.1", "7.4", Some(true)),
            ("^7.1", "8.0", Some(false)),
            (">=7.2 <8.0", "7.1", Some(false)),
            (">=7.2 <8.0", "7.4", Some(true)),
            (">=7.2,<8.0", "8.0", Some(false)),
            ("^7.4 || ^8.0", "8.4", Some(true)),
            ("^7.4|^8.0", "7.3", Some(false)),
            ("~7.1.3", "7.2", Some(false)),
            ("~7.1.3", "7.1", Some(true)),
            ("~7.1", "7.4", Some(true)),
            ("7.1.*", "7.1", Some(true)),
            ("7.1", "7.2", Some(false)),
            (">=7.1.3", "7.1", Some(true)),
            ("<7.1.3", "7.1", Some(true)),
            ("<7.1", "7.1", Some(false)),
            (">7.4", "8.0", Some(true)),
            ("!=7.2", "7.2", Some(false)),
            ("*", "8.5", Some(true)),
            ("dev-main", "8.4", None),
            ("^7.1 || banana", "8.4", None),
            ("", "8.4", None),
        ] {
            assert_eq!(satisfies(constraint, v(version)), expected, "{constraint} on {version}");
        }
    }
}
