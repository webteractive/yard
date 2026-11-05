//! The Logs screen's buffer (M6 design §8.2): the last lines from every unit, filtered by unit and
//! text, and a pause that keeps collecting without showing new lines.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub id: u64,
    pub unit: String,
    pub text: String,
}

#[derive(Debug)]
pub struct LogBuffer {
    pub capacity: usize,
    pub unit_filter: Option<String>,
    /// Shown lines contain it, ignoring case; empty shows all.
    pub text_filter: String,
    lines: VecDeque<LogLine>,
    next_id: u64,
    /// While paused, lines after this id are kept but not shown.
    paused_at: Option<u64>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        LogBuffer::new(5000)
    }
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        LogBuffer {
            capacity,
            unit_filter: None,
            text_filter: String::new(),
            lines: VecDeque::new(),
            next_id: 1,
            paused_at: None,
        }
    }

    pub fn push(&mut self, unit: &str, text: &str) {
        self.lines.push_back(LogLine { id: self.next_id, unit: unit.to_string(), text: text.to_string() });
        self.next_id += 1;
        while self.lines.len() > self.capacity {
            self.lines.pop_front();
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused_at = paused.then(|| self.next_id - 1);
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn visible(&self) -> impl Iterator<Item = &LogLine> {
        let needle = self.text_filter.to_lowercase();
        self.lines.iter().filter(move |l| {
            self.paused_at.is_none_or(|at| l.id <= at)
                && self.unit_filter.as_ref().is_none_or(|u| &l.unit == u)
                && (needle.is_empty() || l.text.to_lowercase().contains(&needle))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(b: &LogBuffer) -> Vec<String> {
        b.visible().map(|l| format!("{}:{}", l.unit, l.text)).collect()
    }

    #[test]
    fn logs_keep_the_last_lines_and_filter() {
        let mut b = LogBuffer::new(3);
        for (u, t) in [("nginx", "a"), ("redis", "b"), ("nginx", "c"), ("nginx", "GET /x")] {
            b.push(u, t);
        }
        assert_eq!(texts(&b), ["redis:b", "nginx:c", "nginx:GET /x"]);
        b.unit_filter = Some("nginx".into());
        assert_eq!(texts(&b), ["nginx:c", "nginx:GET /x"]);
        b.text_filter = "get".into();
        assert_eq!(texts(&b), ["nginx:GET /x"], "text filter ignores case");
        assert_eq!(LogBuffer::default().capacity, 5000);
    }

    #[test]
    fn paused_logs_drop_nothing_but_show_nothing_new() {
        let mut b = LogBuffer::new(100);
        b.push("nginx", "1");
        b.set_paused(true);
        b.push("nginx", "2");
        assert_eq!(texts(&b), ["nginx:1"]);
        b.set_paused(false);
        assert_eq!(texts(&b), ["nginx:1", "nginx:2"]);
    }
}
