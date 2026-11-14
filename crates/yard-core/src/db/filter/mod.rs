//! The statement filters an import's plain SQL runs through on its way to the tool (db spec
//! §6.6): each statement is held until it is whole, then passed on unchanged, dropped, or,
//! when it would switch, create or drop a database, refused before it is sent.

pub mod mysql;
pub mod postgres;

use yard_proto::StreamFilter;

/// A statement that would switch, create or drop a database (db spec §6.6): never sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    /// The database it names, when it can be read.
    pub database: Option<String>,
    /// Its first 80 characters, from its first keyword.
    pub statement: String,
}

impl Switch {
    pub(crate) fn new(stmt: &[u8], from: usize, database: Option<String>) -> Switch {
        let text = String::from_utf8_lossy(&stmt[from.min(stmt.len())..]);
        Switch { database, statement: text.trim().chars().take(80).collect() }
    }
}

/// The filter an import runs through, as `db.command` named it.
pub enum Filter {
    Pass,
    Mysql(mysql::MysqlFilter),
    Postgres(postgres::PostgresFilter),
}

impl Filter {
    /// `None` for a filter this build does not know (a newer yardd's): the import must not run
    /// unfiltered.
    pub fn new(f: &StreamFilter) -> Option<Filter> {
        match f {
            StreamFilter::None => Some(Filter::Pass),
            StreamFilter::Mysql => Some(Filter::Mysql(mysql::MysqlFilter::new())),
            StreamFilter::Postgres { keep_owners } => {
                Some(Filter::Postgres(postgres::PostgresFilter::new(*keep_owners)))
            }
            _ => None,
        }
    }

    /// The statements `chunk` completed, in order and unchanged. After a switch, the
    /// statements completed before it are returned, and the next call is the `Err`.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<u8>, Switch> {
        match self {
            Filter::Pass => Ok(chunk.to_vec()),
            Filter::Mysql(f) => f.push(chunk),
            Filter::Postgres(f) => f.push(chunk),
        }
    }

    /// What is left at the end of the input.
    pub fn finish(&mut self) -> Result<Vec<u8>, Switch> {
        match self {
            Filter::Pass => Ok(vec![]),
            Filter::Mysql(f) => f.finish(),
            Filter::Postgres(f) => f.finish(),
        }
    }

    /// The statements dropped on purpose (PostgreSQL's ownership and grants).
    pub fn skipped(&self) -> usize {
        match self {
            Filter::Postgres(f) => f.skipped(),
            _ => 0,
        }
    }
}

/// The words of a statement as a client reads them: whitespace, comments and the markers of an
/// executable comment (`/*!NNNNN`, `*/`) skipped, a quoted name unquoted. Each word with the
/// offset it starts at.
pub(crate) fn words(stmt: &[u8], hash_comments: bool, max: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize, s: &[u8]| stmt.get(i..i + s.len()) == Some(s);
    while i < stmt.len() && out.len() < max {
        let b = stmt[i];
        if b.is_ascii_whitespace() || b == b';' || b == b'(' || b == b')' || b == b',' {
            i += 1;
        } else if at(i, b"/*!") {
            i += 3;
            while i < stmt.len() && stmt[i].is_ascii_digit() {
                i += 1;
            }
        } else if at(i, b"/*") {
            i = find(stmt, i + 2, b"*/").map_or(stmt.len(), |e| e + 2);
        } else if at(i, b"*/") {
            i += 2;
        } else if (at(i, b"--") && stmt.get(i + 2).is_none_or(|c| c.is_ascii_whitespace()))
            || (hash_comments && b == b'#')
        {
            i = find(stmt, i, b"\n").map_or(stmt.len(), |e| e + 1);
        } else if matches!(b, b'`' | b'"' | b'\'') {
            let end = stmt[i + 1..].iter().position(|&c| c == b).map_or(stmt.len(), |p| i + 1 + p);
            out.push((i, String::from_utf8_lossy(&stmt[i + 1..end]).into_owned()));
            i = end + 1;
        } else {
            let start = i;
            while i < stmt.len()
                && (stmt[i].is_ascii_alphanumeric() || stmt[i] == b'_' || stmt[i] == b'$' || stmt[i] >= 0x80)
            {
                i += 1;
            }
            if i == start {
                i += 1;
                continue;
            }
            out.push((start, String::from_utf8_lossy(&stmt[start..i]).into_owned()));
        }
    }
    out
}

/// `needle` (lowercase) appears in `hay`, in any case.
pub(crate) fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| from + p)
}

/// MySQL's `USE x` and `CREATE|DROP DATABASE|SCHEMA [IF [NOT] EXISTS] x`, or PostgreSQL's
/// `CREATE|DROP DATABASE x` (a PostgreSQL schema is a namespace inside the database, which
/// every `pg_dump` creates): the switch, else `None`.
pub(crate) fn database_statement(stmt: &[u8], mysql: bool) -> Option<Switch> {
    let w = words(stmt, mysql, 8);
    let up = |i: usize| w.get(i).map(|(_, s)| s.to_ascii_uppercase());
    let first = up(0)?;
    let from = w[0].0;
    // The mysql client's own `connect`, `source` and `system` (off under --binary-mode, and
    // refused here as well).
    if mysql && (first == "USE" || first == "CONNECT") {
        return Some(Switch::new(stmt, from, w.get(1).map(|(_, s)| s.clone())));
    }
    if mysql && (first == "SOURCE" || first == "SYSTEM") {
        return Some(Switch::new(stmt, from, None));
    }
    let database = match up(1).as_deref() {
        Some("DATABASE") => true,
        Some("SCHEMA") => mysql,
        _ => false,
    };
    if (first == "CREATE" || first == "DROP") && database {
        let mut n = 2;
        while matches!(up(n).as_deref(), Some("IF" | "NOT" | "EXISTS")) {
            n += 1;
        }
        return Some(Switch::new(stmt, from, w.get(n).map(|(_, s)| s.clone())));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::StreamFilter;

    #[test]
    fn new_picks_the_filter() {
        assert!(matches!(Filter::new(&StreamFilter::None), Some(Filter::Pass)));
        assert!(matches!(Filter::new(&StreamFilter::Mysql), Some(Filter::Mysql(_))));
        assert!(Filter::new(&StreamFilter::Other).is_none());
        assert!(matches!(
            Filter::new(&StreamFilter::Postgres { keep_owners: false }),
            Some(Filter::Postgres(_))
        ));
        let mut pass = Filter::Pass;
        assert_eq!(pass.push(b"USE x;").unwrap(), b"USE x;");
        assert_eq!(pass.finish().unwrap(), b"");
    }
}
