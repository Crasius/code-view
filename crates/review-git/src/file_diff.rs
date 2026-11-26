//! Compact unified-diff representation: one text buffer plus fixed-size line records,
//! so a 100k-line diff costs one allocation for text and ~20 bytes per line.
//!
//! A line's text has neither its line break nor a leading byte order mark: whether the line ended
//! in CRLF and whether it carried the BOM are kept as flags, so a change of only those still reads
//! as a change (see [`WhitespaceSummary`]) while the text compares and highlights like any other.
//!
//! Line records address the text with `u32` offsets; a diff whose text would pass 4 GiB ends at
//! the last line that still fits, rather than wrapping an offset around.

// RS-DONT-04: the input is git's output; parsing it never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use crate::whitespace::{self, WhitespaceSummary};

/// U+FEFF at the very start of a file.
const BOM: char = '\u{FEFF}';

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// `@@ -a,b +c,d @@ section` separator; the text is the section heading git found.
    Hunk,
    Context,
    Added,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the base file, 0 when the line does not exist there.
    pub old_line: u32,
    /// Line number in the head file, 0 when the line does not exist there.
    pub new_line: u32,
    /// The line has no trailing newline in its file.
    pub missing_newline: bool,
    /// The line ended in CRLF in its file (the text never holds the CR).
    pub has_cr: bool,
    /// The line is its file's first and starts with a byte order mark (not part of the text).
    pub has_bom: bool,
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    text: String,
    lines: Vec<DiffLine>,
    pub is_binary: bool,
    /// What changed when every change is only whitespace, line endings or the BOM.
    whitespace: Option<WhitespaceSummary>,
}

impl FileDiff {
    /// Parses `git diff` unified output of a single file.
    pub fn parse_unified(output: &[u8]) -> FileDiff {
        parse(output)
    }

    pub fn lines(&self) -> &[DiffLine] {
        &self.lines
    }

    pub fn text(&self, line: &DiffLine) -> &str {
        let start = line.start as usize;
        let end = start + line.len as usize;
        self.text.get(start..end).expect("push builds every line's offsets from this text")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Bytes held by this diff, for cache accounting.
    pub fn heap_size(&self) -> usize {
        self.text.capacity() + self.lines.capacity() * std::mem::size_of::<DiffLine>()
    }

    pub fn count(&self, kind: LineKind) -> usize {
        self.lines.iter().filter(|line| line.kind == kind).count()
    }

    /// `Some` when the diff changes lines and every change is only whitespace, line endings, the
    /// newline at the end of the file or the byte order mark: what changed, in numbers.
    pub fn whitespace(&self) -> Option<&WhitespaceSummary> {
        self.whitespace.as_ref()
    }

    /// Adds one line; `false` when its text no longer fits the `u32` offsets (see the module docs).
    fn push(&mut self, kind: LineKind, old_line: u32, new_line: u32, content: &str) -> bool {
        let (content, has_cr) = match content.strip_suffix('\r') {
            Some(stripped) => (stripped, true),
            None => (content, false),
        };
        let is_first_line = match kind {
            LineKind::Deleted => old_line == 1,
            LineKind::Added => new_line == 1,
            LineKind::Context => old_line == 1 || new_line == 1,
            LineKind::Hunk => false,
        };
        let (content, has_bom) = match content.strip_prefix(BOM) {
            Some(stripped) if is_first_line => (stripped, true),
            _ => (content, false),
        };
        let (Ok(start), Ok(len)) = (u32::try_from(self.text.len()), u32::try_from(content.len()))
        else {
            return false;
        };
        self.text.push_str(content);
        self.lines.push(DiffLine {
            kind,
            old_line,
            new_line,
            missing_newline: false,
            has_cr,
            has_bom,
            start,
            len,
        });
        true
    }
}

const HUNK_PREFIX: &str = "@@ -";
const BINARY_MARKERS: [&str; 2] = ["Binary files ", "GIT binary patch"];

pub(crate) fn parse(output: &[u8]) -> FileDiff {
    let text = String::from_utf8_lossy(output);
    let mut diff = FileDiff {
        text: String::with_capacity(text.len()),
        lines: Vec::with_capacity(text.len() / 40),
        is_binary: false,
        whitespace: None,
    };
    let mut old_line = 0u32;
    let mut new_line = 0u32;
    let mut is_in_hunk = false;

    // Git ends every output line with LF; a CR before it belongs to the file's line (CRLF).
    for line in text.split('\n') {
        if line.starts_with(HUNK_PREFIX) {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let Some((old_start, new_start, section)) = parse_hunk_header(line) else {
                continue;
            };
            old_line = old_start;
            new_line = new_start;
            is_in_hunk = true;
            if !diff.push(LineKind::Hunk, old_start, new_start, section) {
                break;
            }
            continue;
        }
        if !is_in_hunk {
            let is_binary_marker = BINARY_MARKERS.iter().any(|marker| line.starts_with(marker));
            diff.is_binary |= is_binary_marker;
            continue;
        }
        let mut chars = line.chars();
        let Some(marker) = chars.next() else {
            continue;
        };
        let (kind, old, new) = match marker {
            ' ' => (LineKind::Context, old_line, new_line),
            '+' => (LineKind::Added, 0, new_line),
            '-' => (LineKind::Deleted, old_line, 0),
            '\\' => {
                if let Some(previous) = diff.lines.last_mut() {
                    previous.missing_newline = true;
                }
                continue;
            }
            // A new file header inside the output ends the hunk (should not happen for
            // single-file diffs); so does a line without a marker, such as one that starts with a
            // multi-byte character.
            _ => {
                is_in_hunk = false;
                continue;
            }
        };
        // The text starts right after the marker.
        if !diff.push(kind, old, new, chars.as_str()) {
            break;
        }
        if kind != LineKind::Added {
            old_line = old_line.saturating_add(1);
        }
        if kind != LineKind::Deleted {
            new_line = new_line.saturating_add(1);
        }
    }
    diff.text.shrink_to_fit();
    diff.lines.shrink_to_fit();
    diff.whitespace = whitespace::summarize(&diff);
    diff
}

/// `@@ -12,7 +12,9 @@ fn main()` → `(12, 12, "fn main()")`.
fn parse_hunk_header(line: &str) -> Option<(u32, u32, &str)> {
    let rest = line.strip_prefix(HUNK_PREFIX)?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old_range, new_range) = ranges.split_once(" +")?;
    let old_start = old_range.split(',').next()?.parse().ok()?;
    let new_start = new_range.split(',').next()?.parse().ok()?;
    Some((old_start, new_start, section.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        "diff --git a/src/a.rs b/src/a.rs\n",
        "index 111..222 100644\n",
        "--- a/src/a.rs\n",
        "+++ b/src/a.rs\n",
        "@@ -1,3 +1,4 @@ fn main()\n",
        " keep\n",
        "-old\n",
        "+new\n",
        "+added\n",
        " tail\n",
        "\\ No newline at end of file\n",
    );

    #[test]
    fn parses_hunks_and_line_numbers() {
        let diff = parse(SAMPLE.as_bytes());
        let kinds: Vec<LineKind> = diff.lines().iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Hunk,
                LineKind::Context,
                LineKind::Deleted,
                LineKind::Added,
                LineKind::Added,
                LineKind::Context
            ]
        );
        let lines = diff.lines();
        assert_eq!(diff.text(&lines[0]), "fn main()");
        assert_eq!((lines[2].old_line, lines[2].new_line), (2, 0));
        assert_eq!((lines[4].old_line, lines[4].new_line), (0, 3));
        assert_eq!((lines[5].old_line, lines[5].new_line), (3, 4));
        assert_eq!(diff.text(&lines[3]), "new");
        assert!(lines[5].missing_newline);
        assert!(!diff.is_binary);
    }

    #[test]
    fn detects_binary() {
        let output = "diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ\n";
        let diff = parse(output.as_bytes());
        assert!(diff.is_binary);
        assert!(diff.is_empty());
    }

    /// The parser cut the marker off as `&line[1..]`, which panics when the line starts with a
    /// multi-byte character instead of a marker.
    #[test]
    fn a_line_without_a_marker_ends_the_hunk_instead_of_panicking() {
        let diff = parse("@@ -1 +1 @@\n-a\n+b\n\u{e9}t\u{e9}\n+c\n".as_bytes());
        let kinds: Vec<LineKind> = diff.lines().iter().map(|line| line.kind).collect();
        assert_eq!(kinds, [LineKind::Hunk, LineKind::Deleted, LineKind::Added]);
    }

    #[test]
    fn keeps_carriage_returns_as_line_endings() {
        let output = "@@ -1,2 +1,2 @@ fn main()\r\n-a\n+a\r\n b\r\n";
        let diff = parse(output.as_bytes());
        let lines = diff.lines();
        assert_eq!(diff.text(&lines[0]), "fn main()");
        assert_eq!((diff.text(&lines[1]), lines[1].has_cr), ("a", false));
        assert_eq!(
            (diff.text(&lines[2]), lines[2].has_cr),
            ("a", true),
            "the CR is the line ending, not text"
        );
        assert!(lines[3].has_cr);
        assert_eq!(
            lines[2].kind,
            LineKind::Added,
            "a line that only gained a CR is still a change"
        );
    }
}
