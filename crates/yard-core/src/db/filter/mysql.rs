//! MySQL's statement filter: the quoting, comments and `DELIMITER` lines the `mysql` client reads.

use super::{Switch, database_statement};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    /// Inside `'…'`, `"…"` or `` `…` ``.
    Quote(u8),
    /// After `--`, waiting to see whether whitespace makes it a comment.
    Dash,
    Line,
    Block,
    /// A `DELIMITER x` line, read to its end.
    DelimiterLine,
}

/// Holds the statement in progress and emits each one whole and unchanged.
pub struct MysqlFilter {
    buf: Vec<u8>,
    delimiter: Vec<u8>,
    state: State,
    prev: u8,
    escaped: bool,
    /// Bytes of the statement so far that are not whitespace or comment.
    content: usize,
    /// `content` at the start of the current line.
    content_at_line: usize,
    line_start: usize,
    /// This byte opened a block comment.
    opened: bool,
    switched: Option<Switch>,
}

impl Default for MysqlFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl MysqlFilter {
    pub fn new() -> MysqlFilter {
        MysqlFilter {
            buf: Vec::new(),
            delimiter: b";".to_vec(),
            state: State::Normal,
            prev: 0,
            escaped: false,
            content: 0,
            content_at_line: 0,
            line_start: 0,
            opened: false,
            switched: None,
        }
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
        if self.state == State::DelimiterLine {
            self.end_delimiter_line(&mut out);
        } else {
            self.complete(&mut out);
        }
        self.switched.clone().map_or(Ok(out), Err)
    }

    fn step(&mut self, b: u8, out: &mut Vec<u8>) {
        self.buf.push(b);
        match self.state {
            State::Quote(q) => {
                if self.escaped {
                    self.escaped = false;
                } else if b == b'\\' && q != b'`' {
                    self.escaped = true;
                } else if b == q {
                    self.state = State::Normal;
                }
            }
            State::Line => {
                if b == b'\n' {
                    self.state = State::Normal;
                    self.new_line();
                }
            }
            State::Block => {
                if self.prev == b'*' && b == b'/' {
                    self.state = State::Normal;
                    self.prev = 0;
                    return;
                }
            }
            State::DelimiterLine => {
                if b == b'\n' {
                    self.end_delimiter_line(out);
                }
            }
            State::Dash => {
                if b.is_ascii_whitespace() || b.is_ascii_control() {
                    self.state = State::Line;
                    if b == b'\n' {
                        self.state = State::Normal;
                        self.new_line();
                    }
                } else {
                    // Not a comment after all: both dashes were statement text.
                    self.state = State::Normal;
                    self.content += 2;
                    self.normal(b, out);
                }
            }
            State::Normal => self.normal(b, out),
        }
        // The `*` that opened a comment cannot also close it (`/*/`).
        self.prev = if std::mem::take(&mut self.opened) { 0 } else { b };
    }

    fn normal(&mut self, b: u8, out: &mut Vec<u8>) {
        match b {
            b'*' if self.prev == b'/' => {
                self.state = State::Block;
                self.content -= 1;
                self.opened = true;
                return;
            }
            b'-' if self.prev == b'-' => {
                self.state = State::Dash;
                self.content -= 1;
                return;
            }
            b'#' => {
                self.state = State::Line;
                return;
            }
            b'\'' | b'"' | b'`' => {
                self.state = State::Quote(b);
                self.content += 1;
            }
            b'\n' => self.new_line(),
            b if b.is_ascii_whitespace() => {
                if self.content == self.content_at_line + 9 && self.line_is(b"delimiter") {
                    self.state = State::DelimiterLine;
                    return;
                }
            }
            _ => self.content += 1,
        }
        if self.state == State::Normal && !self.delimiter.is_empty() && self.buf.ends_with(&self.delimiter) {
            self.complete(out);
        }
    }

    fn new_line(&mut self) {
        self.content_at_line = self.content;
        self.line_start = self.buf.len();
    }

    /// The current line, trimmed, is `word` (any case), and nothing came before it in the statement.
    fn line_is(&self, word: &[u8]) -> bool {
        self.content_at_line == 0
            && self.buf[self.line_start..self.buf.len() - 1].trim_ascii().eq_ignore_ascii_case(word)
    }

    fn end_delimiter_line(&mut self, out: &mut Vec<u8>) {
        let line = self.buf[self.line_start..].trim_ascii();
        let rest = line.get(b"delimiter".len()..).unwrap_or_default().trim_ascii();
        let delimiter: Vec<u8> = rest.iter().take_while(|c| !c.is_ascii_whitespace()).copied().collect();
        if !delimiter.is_empty() {
            self.delimiter = delimiter;
        }
        out.append(&mut self.buf);
        self.reset();
    }

    fn complete(&mut self, out: &mut Vec<u8>) {
        if self.content > 0
            && let Some(s) = database_statement(&self.buf, true)
        {
            self.switched = Some(s);
            self.buf.clear();
            return;
        }
        out.append(&mut self.buf);
        self.reset();
    }

    fn reset(&mut self) {
        self.state = State::Normal;
        self.content = 0;
        self.content_at_line = 0;
        self.line_start = 0;
        self.escaped = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &[u8], chunk: usize) -> Result<Vec<u8>, (Vec<u8>, Switch)> {
        let mut f = MysqlFilter::new();
        let mut out = Vec::new();
        for c in input.chunks(chunk.max(1)) {
            match f.push(c) {
                Ok(b) => out.extend(b),
                Err(s) => return Err((out, s)),
            }
        }
        match f.finish() {
            Ok(b) => out.extend(b),
            Err(s) => return Err((out, s)),
        }
        Ok(out)
    }

    const DUMP: &str = "-- MySQL dump 10.13  Distrib 8.4.13
--
-- Host: localhost    Database: shop
/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!50503 SET NAMES utf8mb4 */;
# a hash comment with USE x; in it
DROP TABLE IF EXISTS `orders`;
CREATE TABLE `orders` (
  `id` int NOT NULL,
  `note` varchar(255) DEFAULT 'it''s; USE x'
) ENGINE=InnoDB;
INSERT INTO `orders` VALUES (1,'it\\'s; USE x'),(2,\"a\\\"b; CREATE DATABASE y\");
/* USE x; */
DELIMITER ;;
/*!50003 CREATE*/ /*!50003 TRIGGER `t` BEFORE INSERT ON `orders` FOR EACH ROW BEGIN
  SET @a = 1; USE_IT: LOOP LEAVE USE_IT; END LOOP;
END */;;
DELIMITER ;
CREATE TABLE `use` (`USE` int);
SELECT 'done';
";

    #[test]
    fn plain_dump_passes_byte_identical() {
        for chunk in [1, 7, 64, 1 << 16] {
            assert_eq!(run(DUMP.as_bytes(), chunk).unwrap(), DUMP.as_bytes(), "chunk {chunk}");
        }
    }

    #[test]
    fn use_is_a_switch() {
        let input = b"CREATE TABLE a (x int);\nUSE `shop`;\nINSERT INTO a VALUES (1);\n";
        let (out, s) = run(input, 5).unwrap_err();
        assert_eq!(out, b"CREATE TABLE a (x int);");
        assert_eq!(s.database.as_deref(), Some("shop"));
        assert!(s.statement.starts_with("USE `shop`"), "{}", s.statement);
    }

    #[test]
    fn executable_comment_use_is_a_switch() {
        let (out, s) = run(b"/*!32312 USE `shop`*/;\n", 3).unwrap_err();
        assert_eq!(out, b"");
        assert_eq!(s.database.as_deref(), Some("shop"));
    }

    #[test]
    fn drop_database_is_a_switch() {
        let (out, s) = run(b"/*!40000 DROP DATABASE IF EXISTS `x`*/;\n", 64).unwrap_err();
        assert_eq!(out, b"");
        assert_eq!(s.database.as_deref(), Some("x"));
    }

    #[test]
    fn create_database_and_schema_are_switches() {
        for (sql, db) in [
            ("create database shop;", "shop"),
            (
                "CREATE DATABASE /*!32312 IF NOT EXISTS*/ `shop` /*!40100 DEFAULT CHARACTER SET utf8mb4 */;",
                "shop",
            ),
            ("Create Schema If Not Exists other;", "other"),
            ("drop schema `old`;", "old"),
            ("  -- lead\n  use other ;", "other"),
        ] {
            let (_, s) = run(sql.as_bytes(), 4).unwrap_err();
            assert_eq!(s.database.as_deref(), Some(db), "{sql}");
        }
    }

    #[test]
    fn words_in_strings_and_comments_are_not_switches() {
        let input = "SELECT 'USE x';\n-- USE x;\n# USE x;\n/* USE x; */\nCREATE TABLE `use` (`USE` int);\nSELECT \"CREATE DATABASE y\";\nUSER_DEFINED;\n";
        assert_eq!(run(input.as_bytes(), 3).unwrap(), input.as_bytes());
    }

    #[test]
    fn empty_and_starred_comments_close() {
        let input = b"/**/ SELECT 1; /***/ /* ** */ SELECT 2;\n";
        assert_eq!(run(input, 1).unwrap(), input);
        let (_, s) = run(b"/**/ USE x;", 2).unwrap_err();
        assert_eq!(s.database.as_deref(), Some("x"));
    }

    #[test]
    fn a_comment_opening_with_a_slash_is_still_open() {
        // `/*/` opens a comment; it does not close one.
        let input = b"/*/ ; USE x; */ SELECT 1;\n";
        assert_eq!(run(input, 1).unwrap(), input);
    }

    #[test]
    fn a_statement_split_across_chunks_passes_intact() {
        let mut input = b"INSERT INTO t VALUES ".to_vec();
        let mut i = 0u64;
        while input.len() < 20 << 20 {
            input.extend(format!("({i},'a\\'b;c''d','USE x; -- y'),").as_bytes());
            i += 1;
        }
        input.extend(b"(0,'',''); USE_ME;\nSELECT 1;\n");
        assert_eq!(run(&input, 1 << 16).unwrap(), input);
    }
}
