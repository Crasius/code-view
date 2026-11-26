//! A pipeline log on disk with a line-offset index, so a viewer can show any window of a
//! multi-gigabyte log while holding only the offsets (8 bytes per line) and the visible lines.

// RS-DONT-04: the log comes from a pipeline host; reading it never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const READ_CHUNK: usize = 256 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;

pub struct LogFile {
    path: PathBuf,
    line_starts: Vec<u64>,
    length: u64,
    /// The file ends with a newline, whose empty line after it is not counted.
    ends_with_newline: bool,
}

impl LogFile {
    /// Indexes line starts in one streaming pass.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut line_starts = vec![0u64];
        let length = index_lines(&mut file, 0, &mut line_starts)?;
        Ok(Self::indexed(path, line_starts, length))
    }

    /// The same file after more was appended to it (a running job's log fetched again): only the
    /// new bytes are indexed. A file that got shorter is indexed anew.
    pub fn extended(&self) -> io::Result<Self> {
        let mut file = File::open(&self.path)?;
        if file.metadata()?.len() < self.length {
            return Self::open(&self.path);
        }
        let mut line_starts = self.line_starts.clone();
        if self.ends_with_newline {
            line_starts.push(self.length);
        }
        file.seek(SeekFrom::Start(self.length))?;
        let length = index_lines(&mut file, self.length, &mut line_starts)?;
        Ok(Self::indexed(&self.path, line_starts, length))
    }

    fn indexed(path: &Path, mut line_starts: Vec<u64>, length: u64) -> Self {
        let ends_with_newline =
            line_starts.len() > 1 && line_starts.last().is_some_and(|start| *start == length);
        if ends_with_newline {
            line_starts.pop();
        }
        Self { path: path.to_owned(), line_starts, length, ends_with_newline }
    }

    pub fn line_count(&self) -> usize {
        if self.length == 0 {
            return 0;
        }
        self.line_starts.len()
    }

    /// Bytes indexed.
    pub fn byte_len(&self) -> u64 {
        self.length
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lines `range` (clamped), with ANSI escape sequences and timestamps left untouched,
    /// carriage returns removed and very long lines truncated.
    pub fn read_lines(&self, range: std::ops::Range<usize>) -> io::Result<Vec<String>> {
        let end = range.end.min(self.line_count());
        if range.start >= end {
            return Ok(Vec::new());
        }
        let Some(&start) = self.line_starts.get(range.start) else {
            return Ok(Vec::new());
        };
        let mut reader = BufReader::new(File::open(&self.path)?);
        reader.seek(SeekFrom::Start(start))?;
        let mut lines = Vec::with_capacity(end - range.start);
        let mut buffer = Vec::new();
        for _ in range.start..end {
            buffer.clear();
            reader.read_until(b'\n', &mut buffer)?;
            buffer.truncate(MAX_LINE_BYTES);
            let text = String::from_utf8_lossy(&buffer);
            lines.push(text.trim_end_matches(['\n', '\r']).to_owned());
        }
        Ok(lines)
    }

    /// Line numbers (0-based) containing `needle`, case-insensitive, scanning the file once.
    pub fn find(&self, needle: &str) -> io::Result<Vec<usize>> {
        let needle = needle.to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let reader = BufReader::new(File::open(&self.path)?);
        let mut matches = Vec::new();
        for (index, line) in reader.split(b'\n').enumerate() {
            let bytes = line?;
            let text = String::from_utf8_lossy(&bytes).to_lowercase();
            if text.contains(&needle) {
                matches.push(index);
            }
        }
        Ok(matches)
    }

    /// First line (0-based) containing any of `needles`, case-insensitive. Stops at the first hit.
    pub fn first_match(&self, needles: &[&str]) -> io::Result<Option<usize>> {
        self.first_match_from(needles, 0)
    }

    /// [`Self::first_match`] from line `from` on, so a grown log is searched in its new lines only.
    pub fn first_match_from(&self, needles: &[&str], from: usize) -> io::Result<Option<usize>> {
        if from >= self.line_count() {
            return Ok(None);
        }
        let Some(&start) = self.line_starts.get(from) else {
            return Ok(None);
        };
        let needles: Vec<String> = needles.iter().map(|needle| needle.to_lowercase()).collect();
        let mut reader = BufReader::new(File::open(&self.path)?);
        reader.seek(SeekFrom::Start(start))?;
        for (index, line) in reader.split(b'\n').enumerate() {
            let bytes = line?;
            let text = String::from_utf8_lossy(&bytes).to_lowercase();
            if needles.iter().any(|needle| text.contains(needle.as_str())) {
                return Ok(Some(from + index));
            }
        }
        Ok(None)
    }
}

/// Adds the start of every line after a newline in `file` from its position (`offset` in the
/// file) to its end; returns the file's length.
fn index_lines(file: &mut File, mut offset: u64, line_starts: &mut Vec<u64>) -> io::Result<u64> {
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
            return Ok(offset);
        };
        for (index, byte) in chunk.iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push(offset + index as u64 + 1);
            }
        }
        offset += read as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::LogFile;
    use crate::test_support::TestDir;

    #[test]
    fn indexes_and_reads_windows() {
        let dir = TestDir::new("log");
        let path = dir.path().join("job.log");
        std::fs::write(&path, "first\r\nsecond\nERROR third\nfourth\n").unwrap();
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.line_count(), 4);
        assert_eq!(log.read_lines(1..3).unwrap(), ["second", "ERROR third"]);
        assert_eq!(log.read_lines(3..10).unwrap(), ["fourth"]);
        assert_eq!(log.find("error").unwrap(), [2]);
        assert_eq!(log.first_match(&["##[error]", "error "]).unwrap(), Some(2));
        assert_eq!(log.first_match(&["missing"]).unwrap(), None);
    }

    /// A running job's log grows: an unfinished last line goes on, new lines follow; the index of
    /// the grown file equals that of the file indexed anew.
    #[test]
    fn extends_the_index_by_what_was_appended() {
        use std::io::Write;
        let dir = TestDir::new("log-grow");
        let path = dir.path().join("job.log");
        std::fs::write(&path, "").unwrap();
        let empty = LogFile::open(&path).unwrap();
        let mut steps = Vec::new();
        for part in ["$ pytest", " -q\n", "collected 3 items\n", "", "passed\nERROR teardown"] {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(part.as_bytes())
                .unwrap();
            let grown = steps.last().unwrap_or(&empty).extended().unwrap();
            let fresh = LogFile::open(&path).unwrap();
            // Line by line, so every indexed line start is read from.
            let every_line = |log: &LogFile| -> Vec<Vec<String>> {
                (0..log.line_count()).map(|line| log.read_lines(line..line + 1).unwrap()).collect()
            };
            assert_eq!(
                (grown.line_count(), grown.byte_len(), every_line(&grown)),
                (fresh.line_count(), fresh.byte_len(), every_line(&fresh)),
                "after {part:?}"
            );
            steps.push(grown);
        }
        let last = steps.last().unwrap();
        assert_eq!(
            last.read_lines(0..10).unwrap(),
            ["$ pytest -q", "collected 3 items", "passed", "ERROR teardown"]
        );
        assert_eq!(last.first_match_from(&["error"], 2).unwrap(), Some(3));
        assert_eq!(
            last.first_match_from(&["pytest"], 1).unwrap(),
            None,
            "only from the line given"
        );
        assert_eq!(last.first_match_from(&["error"], 9).unwrap(), None);
        std::fs::write(&path, "rewritten\n").unwrap();
        assert_eq!(
            last.extended().unwrap().read_lines(0..5).unwrap(),
            ["rewritten"],
            "a shorter file is indexed anew"
        );
    }

    #[test]
    fn handles_empty_and_unterminated_logs() {
        let dir = TestDir::new("log-empty");
        let path = dir.path().join("job.log");
        std::fs::write(&path, "").unwrap();
        assert_eq!(LogFile::open(&path).unwrap().line_count(), 0);
        std::fs::write(&path, "only").unwrap();
        let log = LogFile::open(&path).unwrap();
        assert_eq!(log.line_count(), 1);
        assert_eq!(log.read_lines(0..1).unwrap(), ["only"]);
        assert_eq!(log.read_lines(1..5).unwrap(), Vec::<String>::new(), "past the end");
        assert_eq!(log.first_match_from(&["only"], 1).unwrap(), None);
    }
}
