//! PostgreSQL's statement filter: the quoting, comments, psql meta-commands and `COPY` data
//! that `psql` reads from a plain `pg_dump`.

use super::{Switch, contains_ci, database_statement, words};

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Normal,
    /// Inside `'…'`; `E'…'` also takes backslash escapes.
    Single {
        escapes: bool,
    },
    Double,
    /// After a `$` that may open a dollar quote, with the tag so far.
    DollarTag(Vec<u8>),
    /// Inside a dollar quote, waiting for this closing `$tag$`.
    Dollar(Vec<u8>),
    Line,
    /// Inside `/* */`, which nests in PostgreSQL.
    Block(u32),
    /// A psql meta-command line (`\connect x`, `\restrict k`).
    Meta,
    /// `COPY … FROM stdin` data, up to its `\.` line, passed on as it comes.
    Copy,
}

/// Holds the statement in progress and emits each one whole, dropping ownership and grant
/// statements unless `keep_owners` (db spec §6.6).
pub struct PostgresFilter {
    buf: Vec<u8>,
    state: State,
    prev: u8,
    /// The byte before `prev`, to tell `E'…'` from an identifier ending in `E`.
    prev2: u8,
    escaped: bool,
    opened: bool,
    content: usize,
    keep_owners: bool,
    skipped: usize,
    copy_line: Vec<u8>,
    /// A `\` outside quotes inside a statement: a psql meta-command.
    inline_meta: bool,
    switched: Option<Switch>,
}

impl PostgresFilter {
    pub fn new(keep_owners: bool) -> PostgresFilter {
        PostgresFilter {
            buf: Vec::new(),
            state: State::Normal,
            prev: 0,
            prev2: 0,
            escaped: false,
            opened: false,
            content: 0,
            keep_owners,
            skipped: 0,
            copy_line: Vec::new(),
            inline_meta: false,
            switched: None,
        }
    }

    /// The ownership and grant statements dropped so far.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<u8>, Switch> {
        if let Some(s) = self.switched.clone() {
            return Err(s);
        }
        let mut out = Vec::new();
        for &b in chunk {
            self.step(b, &mut out);
            if self.switched.is_some() {
                break;
            }
        }
        Ok(out)
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, Switch> {
        if let Some(s) = self.switched.clone() {
            return Err(s);
        }
        let mut out = Vec::new();
        match self.state {
            State::Copy => {}
            State::Meta => self.complete_meta(&mut out),
            _ => self.complete(&mut out),
        }
        self.switched.clone().map_or(Ok(out), Err)
    }

    fn step(&mut self, b: u8, out: &mut Vec<u8>) {
        if self.state == State::Copy {
            out.push(b);
            self.copy_line.push(b);
            if b == b'\n' {
                if matches!(self.copy_line.as_slice(), b"\\.\n" | b"\\.\r\n") {
                    self.state = State::Normal;
                }
                self.copy_line.clear();
            }
            return;
        }
        self.buf.push(b);
        match self.state.clone() {
            State::Single { escapes } => {
                if self.escaped {
                    self.escaped = false;
                } else if escapes && b == b'\\' {
                    self.escaped = true;
                } else if b == b'\'' {
                    self.state = State::Normal;
                }
            }
            State::Double => {
                if b == b'"' {
                    self.state = State::Normal;
                }
            }
            State::DollarTag(mut tag) => {
                if b == b'$' {
                    let mut close = vec![b'$'];
                    close.extend(&tag);
                    close.push(b'$');
                    self.state = State::Dollar(close);
                } else if b.is_ascii_alphabetic()
                    || b == b'_'
                    || b >= 0x80
                    || (b.is_ascii_digit() && !tag.is_empty())
                {
                    tag.push(b);
                    self.state = State::DollarTag(tag);
                } else {
                    // `$1` or a lone `$`: not a quote.
                    self.state = State::Normal;
                    self.normal(b, out);
                }
            }
            State::Dollar(close) => {
                if b == b'$' && self.buf.ends_with(&close) {
                    self.state = State::Normal;
                }
            }
            State::Line => {
                if b == b'\n' {
                    self.state = State::Normal;
                }
            }
            State::Block(depth) => {
                if self.prev == b'/' && b == b'*' {
                    self.state = State::Block(depth + 1);
                    self.opened = true;
                } else if self.prev == b'*' && b == b'/' {
                    self.state = if depth == 1 { State::Normal } else { State::Block(depth - 1) };
                    self.opened = true;
                }
            }
            State::Meta => {
                if b == b'\n' {
                    self.complete_meta(out);
                }
            }
            State::Normal => self.normal(b, out),
            State::Copy => unreachable!("handled above"),
        }
        // A `/*` or `*/` just read cannot also be half of the next one.
        self.prev2 = self.prev;
        self.prev = if std::mem::take(&mut self.opened) { 0 } else { b };
    }

    fn normal(&mut self, b: u8, out: &mut Vec<u8>) {
        match b {
            b'\'' => {
                // `E'…'` only where the `E` starts a token, as psql's lexer reads it.
                let escapes = matches!(self.prev, b'E' | b'e') && !ident_byte(self.prev2);
                self.state = State::Single { escapes };
                self.content += 1;
            }
            b'"' => {
                self.state = State::Double;
                self.content += 1;
            }
            b'$' => {
                if !ident_byte(self.prev) {
                    self.state = State::DollarTag(Vec::new());
                }
                self.content += 1;
            }
            b'-' if self.prev == b'-' => {
                self.state = State::Line;
                self.content -= 1;
            }
            b'*' if self.prev == b'/' => {
                self.state = State::Block(1);
                self.content -= 1;
                self.opened = true;
            }
            b'\\' if self.content == 0 => {
                self.state = State::Meta;
                self.content += 1;
            }
            b'\\' => {
                // psql runs a backslash command found mid-statement too (`SELECT 1 \g`).
                self.inline_meta = true;
                self.content += 1;
            }
            b';' => {
                self.content += 1;
                self.complete(out);
            }
            b if b.is_ascii_whitespace() => {}
            _ => self.content += 1,
        }
    }

    fn complete(&mut self, out: &mut Vec<u8>) {
        if self.content > 0 {
            if self.inline_meta {
                let from = self.buf.len() - self.buf.trim_ascii_start().len();
                self.switched = Some(Switch::new(&self.buf, from, None));
                self.buf.clear();
                return;
            }
            if changes_string_rules(&self.buf) {
                let from = self.buf.len() - self.buf.trim_ascii_start().len();
                self.switched = Some(Switch::new(&self.buf, from, None));
                self.buf.clear();
                return;
            }
            if let Some(s) = database_statement(&self.buf, false) {
                self.switched = Some(s);
                self.buf.clear();
                return;
            }
            if !self.keep_owners && is_ownership(&self.buf) {
                self.skipped += 1;
                self.buf.clear();
                self.reset();
                return;
            }
            if is_copy_from_stdin(&self.buf) {
                out.append(&mut self.buf);
                self.reset();
                self.state = State::Copy;
                return;
            }
        }
        out.append(&mut self.buf);
        self.reset();
    }

    /// `\connect x` and `\c x` switch database; any other meta-command passes.
    fn complete_meta(&mut self, out: &mut Vec<u8>) {
        let line = self.buf.trim_ascii();
        let start = line.iter().position(|&c| c == b'\\').map_or(0, |p| p + 1);
        let mut parts = line[start..].split(|c| c.is_ascii_whitespace()).filter(|p| !p.is_empty());
        let command = parts.next();
        // pg_dump's own `\restrict`/`\unrestrict` pass; any other meta-command (`\!` runs a
        // shell, `\i` reads a file) is refused, and `\connect` names the database it switches to.
        if matches!(command, Some(b"restrict" | b"unrestrict")) {
            out.append(&mut self.buf);
            self.reset();
            return;
        }
        let db = match command {
            Some(b"connect" | b"c") => {
                parts.next().map(|d| String::from_utf8_lossy(d).trim_matches('"').to_string())
            }
            _ => None,
        };
        let from = self.buf.len() - self.buf.trim_ascii_start().len();
        self.switched = Some(Switch::new(&self.buf, from, db));
        self.buf.clear();
    }

    fn reset(&mut self) {
        self.state = State::Normal;
        self.content = 0;
        self.escaped = false;
        self.inline_meta = false;
    }
}

/// `GRANT`, `REVOKE`, `ALTER … OWNER TO`, `ALTER DEFAULT PRIVILEGES`, `SET ROLE`, `SET SESSION
/// AUTHORIZATION` and `RESET ROLE`: statements naming another server's roles.
fn is_ownership(stmt: &[u8]) -> bool {
    let w: Vec<String> = words(stmt, false, 3).into_iter().map(|(_, w)| w.to_ascii_uppercase()).collect();
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    match w.as_slice() {
        ["GRANT", ..] | ["REVOKE", ..] | ["RESET", "ROLE", ..] | ["SET", "ROLE", ..] => true,
        ["SET", "SESSION", "AUTHORIZATION", ..] | ["ALTER", "DEFAULT", "PRIVILEGES", ..] => true,
        ["ALTER", ..] => words(stmt, false, usize::MAX)
            .windows(2)
            .any(|p| p[0].1.eq_ignore_ascii_case("OWNER") && p[1].1.eq_ignore_ascii_case("TO")),
        _ => false,
    }
}

/// A byte psql's lexer reads as part of an identifier (`ident_cont`): one before `$` makes
/// it part of the name, not a dollar quote, and one before `E` makes `E'` no E-string.
fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Anything that names `standard_conforming_strings` except `SET … = on`: with it off,
/// backslashes would end strings where this filter does not (db spec §6.6; fails closed).
fn changes_string_rules(stmt: &[u8]) -> bool {
    let w: Vec<String> = words(stmt, false, 6).into_iter().map(|(_, w)| w.to_ascii_lowercase()).collect();
    if !w.iter().any(|x| x == "standard_conforming_strings")
        && !contains_ci(stmt, b"standard_conforming_strings")
    {
        return false;
    }
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    !matches!(
        w.as_slice(),
        ["set", "standard_conforming_strings", "on"] | ["set", "standard_conforming_strings", "to", "on"]
    )
}

/// `COPY <table> [(<columns>)] FROM STDIN …`, whose data lines follow up to `\.`. A query
/// (`COPY (…) TO`) never reads stdin, and `FROM`/`STDIN` count only unquoted.
fn is_copy_from_stdin(stmt: &[u8]) -> bool {
    let w = words(stmt, false, usize::MAX);
    let Some((at, first)) = w.first() else { return false };
    if !first.eq_ignore_ascii_case("COPY") {
        return false;
    }
    let after = stmt[at + 4..].iter().find(|b| !b.is_ascii_whitespace());
    if after == Some(&b'(') {
        return false;
    }
    let bare =
        |(off, word): &(usize, String), want: &str| stmt[*off] != b'"' && word.eq_ignore_ascii_case(want);
    let Some(from) = w.iter().position(|x| bare(x, "FROM")) else { return false };
    !w[1..from].iter().any(|x| bare(x, "TO")) && w.get(from + 1).is_some_and(|x| bare(x, "STDIN"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output, or what came out before the switch and the switch; then the skipped count.
    type Run = (Result<Vec<u8>, (Vec<u8>, Switch)>, usize);

    fn run(input: &[u8], chunk: usize, keep_owners: bool) -> Run {
        let mut f = PostgresFilter::new(keep_owners);
        let mut out = Vec::new();
        for c in input.chunks(chunk.max(1)) {
            match f.push(c) {
                Ok(b) => out.extend(b),
                Err(s) => return (Err((out, s)), f.skipped()),
            }
        }
        match f.finish() {
            Ok(b) => out.extend(b),
            Err(s) => return (Err((out, s)), f.skipped()),
        }
        (Ok(out), f.skipped())
    }

    const DUMP: &str = r#"--
-- PostgreSQL database dump
--
\restrict abc123

SET statement_timeout = 0;
SET client_encoding = 'UTF8';
SELECT pg_catalog.set_config('search_path', '', false);
CREATE SCHEMA app;
ALTER SCHEMA app OWNER TO postgres;
CREATE FUNCTION app.f(a integer, b text) RETURNS text
    LANGUAGE plpgsql
    AS $_$ BEGIN GRANT SELECT ON t TO x; RETURN 'it''s; \connect x'; END $_$;
ALTER FUNCTION app.f(a integer, b text) OWNER TO postgres;
CREATE TABLE app.t (a text);
ALTER TABLE app.t OWNER TO postgres;
COPY app.t (a) FROM stdin;
GRANT x	REVOKE; \connect y
it's; CREATE DATABASE z
\.

/* outer /* nested; GRANT */ still comment; */
SELECT E'a\'; GRANT b', 'c''; REVOKE d';
GRANT ALL ON SCHEMA app TO PUBLIC;
REVOKE ALL ON SCHEMA app FROM PUBLIC;
\unrestrict abc123
"#;

    #[test]
    fn plain_pg_dump_passes_with_keep_owners() {
        for chunk in [1, 5, 64, 1 << 16] {
            let (out, skipped) = run(DUMP.as_bytes(), chunk, true);
            assert_eq!(out.unwrap(), DUMP.as_bytes(), "chunk {chunk}");
            assert_eq!(skipped, 0);
        }
    }

    #[test]
    fn owners_and_grants_are_skipped() {
        let (out, skipped) = run(DUMP.as_bytes(), 7, false);
        let out = String::from_utf8(out.unwrap()).unwrap();
        for gone in [
            "ALTER SCHEMA app OWNER TO postgres;",
            "ALTER FUNCTION app.f(a integer, b text) OWNER TO postgres;",
            "ALTER TABLE app.t OWNER TO postgres;",
            "GRANT ALL ON SCHEMA app TO PUBLIC;",
            "REVOKE ALL ON SCHEMA app FROM PUBLIC;",
        ] {
            assert!(!out.contains(gone), "{gone} kept:\n{out}");
        }
        assert_eq!(skipped, 5);
        for kept in [
            "GRANT SELECT ON t TO x; RETURN",
            "GRANT x\tREVOKE; \\connect y",
            "CREATE SCHEMA app;",
            "\\restrict abc123",
            "SELECT E'a\\'; GRANT b'",
        ] {
            assert!(out.contains(kept), "{kept} lost:\n{out}");
        }
    }

    #[test]
    fn role_statements_are_skipped() {
        for sql in [
            "SET ROLE admin;",
            "SET SESSION AUTHORIZATION 'postgres';",
            "RESET ROLE;",
            "ALTER DEFAULT PRIVILEGES FOR ROLE x GRANT SELECT ON TABLES TO y;",
        ] {
            let (out, skipped) = run(sql.as_bytes(), 3, false);
            assert_eq!((out.unwrap(), skipped), (Vec::new(), 1), "{sql}");
        }
        let (out, _) = run(b"SET search_path = app;", 3, false);
        assert_eq!(out.unwrap(), b"SET search_path = app;");
    }

    #[test]
    fn connect_is_a_switch() {
        for (sql, db) in [
            ("\\connect shop\n", "shop"),
            ("\\c shop\n", "shop"),
            ("CREATE DATABASE shop WITH TEMPLATE = template0;", "shop"),
            ("DROP DATABASE IF EXISTS \"old\";", "old"),
        ] {
            let input = format!("CREATE TABLE a (x int);\n{sql}SELECT 1;\n");
            let (out, _) = run(input.as_bytes(), 4, true);
            let (out, s) = out.unwrap_err();
            assert_eq!(out, b"CREATE TABLE a (x int);", "{sql}");
            assert_eq!(s.database.as_deref(), Some(db), "{sql}");
        }
    }

    #[test]
    fn meta_commands_other_than_restrict_are_refused() {
        for sql in
            ["\\! ls\n", "\\i /tmp/x.sql\n", "\\o /tmp/out\n", "SELECT 1 \\g\n", "SELECT 1; \\! rm -rf x\n"]
        {
            let (out, _) = run(sql.as_bytes(), 2, true);
            assert!(out.is_err(), "{sql:?} passed");
        }
        let ok = b"\\restrict k1\nSELECT E'a\\\\b';\n\\unrestrict k1\n";
        assert_eq!(run(ok, 1, true).0.unwrap(), ok);
    }

    #[test]
    fn lexer_differentials_fail_closed() {
        for sql in [
            // psql reads `abcE` as an identifier, so `'x\'` ends and the meta-command runs.
            "SELECT 1 AS abcE'x\\'; \\! touch /tmp/p\n",
            // A byte >= 0x80 is an identifier character, so `é$x$` is no dollar quote.
            "SELECT é$x$ ; \\! ls\n",
            // No standard_conforming_strings but on: backslashes would change where strings end.
            "SET standard_conforming_strings = off;\n",
            "SELECT pg_catalog.set_config('standard_conforming_strings', 'off', false);\n",
            // Not a COPY that reads stdin: what follows is statements to psql.
            "COPY (SELECT 1 FROM stdin) TO STDOUT;\n\\! ls\n",
            "COPY t (\"from\", \"stdin\") TO STDOUT;\n\\! ls\n",
        ] {
            let (out, _) = run(sql.as_bytes(), 3, true);
            assert!(out.is_err(), "{sql:?} passed");
        }
        for ok in ["SET standard_conforming_strings = on;\n", "SELECT $é$ \\! ; $é$, E'a\\'b';\n"] {
            let (out, _) = run(ok.as_bytes(), 2, true);
            assert_eq!(out.unwrap(), ok.as_bytes(), "{ok:?}");
        }
    }

    #[test]
    fn create_schema_is_not_a_switch() {
        let (out, _) = run(b"CREATE SCHEMA IF NOT EXISTS app;\nDROP SCHEMA old;\n", 3, true);
        assert!(out.is_ok());
    }

    #[test]
    fn dollar_quotes_hide_keywords() {
        let input = b"DO $body$ BEGIN EXECUTE 'CREATE DATABASE x'; GRANT a; END $body$;\nSELECT $$ \\connect y; $$, $1;\n";
        let (out, skipped) = run(input, 2, false);
        assert_eq!((out.unwrap(), skipped), (input.to_vec(), 0));
    }

    #[test]
    fn a_statement_split_across_chunks_passes_intact() {
        let mut input = b"CREATE FUNCTION f() RETURNS int AS $tag$ SELECT 1; $tag$ LANGUAGE sql;\nCOPY t (a, b) FROM stdin;\n".to_vec();
        let mut i = 0u64;
        while input.len() < 20 << 20 {
            input.extend(format!("{i}\tGRANT; \\c x; $$ '\n").as_bytes());
            i += 1;
        }
        input.extend(b"\\.\nSELECT 1;\n");
        let (out, skipped) = run(&input, 1 << 16, false);
        assert_eq!(out.unwrap(), input);
        assert_eq!(skipped, 0);
    }
}
