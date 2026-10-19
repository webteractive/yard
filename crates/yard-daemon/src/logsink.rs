//! `logs/<unit>.log`, rotated by size, with the last lines kept for `Failed` (spec §6.1).

use crate::events::EventBus;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use yard_proto::Event;

pub const TAIL_LINES: usize = 50;

pub struct LogSink {
    path: PathBuf,
    /// `None` when the file could not be opened: lines still reach the tail and the bus.
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    keep: usize,
    unit: String,
    bus: EventBus,
    tail: VecDeque<String>,
}

impl LogSink {
    pub fn open(
        path: &Path,
        max_bytes: u64,
        keep: usize,
        unit: String,
        bus: EventBus,
    ) -> io::Result<LogSink> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self { file: Some(file), size, ..Self::detached(path, max_bytes, keep, unit, bus) })
    }

    /// A sink with no file behind it, for when `open` fails.
    pub fn detached(path: &Path, max_bytes: u64, keep: usize, unit: String, bus: EventBus) -> LogSink {
        Self {
            path: path.to_path_buf(),
            file: None,
            size: 0,
            max_bytes,
            keep,
            unit,
            bus,
            tail: VecDeque::with_capacity(TAIL_LINES),
        }
    }

    pub fn write_line(&mut self, line: &str) {
        let len = line.len() as u64 + 1;
        if self.file.is_some() && self.size > 0 && self.size + len > self.max_bytes {
            self.rotate();
        }
        if let Some(file) = self.file.as_mut()
            && writeln!(file, "{line}").is_ok()
        {
            self.size += len;
        }
        if self.tail.len() == TAIL_LINES {
            self.tail.pop_front();
        }
        self.tail.push_back(line.to_string());
        self.bus.emit(Event::LogLine { unit: self.unit.clone(), line: line.to_string() });
    }

    pub fn tail(&self) -> Vec<String> {
        self.tail.iter().cloned().collect()
    }

    /// `log` → `log.1` → … → `log.<keep>`; the oldest falls off.
    fn rotate(&mut self) {
        let numbered = |n: usize| PathBuf::from(format!("{}.{n}", self.path.display()));
        let _ = std::fs::remove_file(numbered(self.keep));
        for n in (1..self.keep).rev() {
            let _ = std::fs::rename(numbered(n), numbered(n + 1));
        }
        if self.keep > 0 {
            let _ = std::fs::rename(&self.path, numbered(1));
        }
        let reopened = OpenOptions::new().create(true).write(true).truncate(true).open(&self.path);
        self.file = reopened.ok();
        self.size = 0;
    }
}
