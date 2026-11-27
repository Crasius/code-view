//! The file a thread's code excerpt reads from: every line of the head version, with the removed
//! lines of the diff between them where they were, numbered in both versions. Built once in the
//! background from the head blob and a zero-context diff, so every mode and expansion is a
//! slice of the same rows (Split's paired up once, as the Files tab pairs them). Syntax colors
//! arrive later from the coloring thread; until they do the rows paint plain in the same font, so
//! nothing moves.

use std::sync::{Arc, OnceLock};

use cv_code::diff_rows::{Pairs, Row, RowKind};
use cv_syntax::blob_text::BlobText;
use cv_syntax::highlight::{self, LineSpans, Span};
use cv_syntax::{color_worker, grammar};
use review_git::{FileDiff, LineKind};

/// Files above this many bytes stay plain in excerpts (our own budget): coloring one is a whole-file
/// parse on the coloring thread the Files tab shares.
const MAX_COLOR_BYTES: usize = 2 * 1024 * 1024;

/// One line of the unified file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeRow {
    /// `Context` for an unchanged line, `Added` or `Deleted` for a changed one.
    pub kind: LineKind,
    /// Line numbers in the base and the head version, 0 where the line does not exist.
    pub old_line: u32,
    pub new_line: u32,
    /// A removed line's index among the removed lines.
    removed: u32,
}

/// One side-by-side line: indices into [`CodeFile::rows`] of its base (`old`) and head (`new`)
/// line, the same row on both sides for an unchanged line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRow {
    pub old: Option<u32>,
    pub new: Option<u32>,
}

/// The syntax spans of the head text and of the removed lines, line by line.
struct Colors {
    head: LineSpans,
    removed: LineSpans,
}

pub struct CodeFile {
    head: BlobText,
    /// The removed lines, one per line, in file order.
    removed: BlobText,
    rows: Vec<CodeRow>,
    /// Indices into `rows` of the lines New only shows (all but the removed ones).
    head_rows: Vec<u32>,
    /// The rows side by side, each change's removed and added lines paired up.
    split_rows: Vec<SplitRow>,
    max_line: u32,
    colors: OnceLock<Colors>,
}

impl CodeFile {
    /// The rows of `head` (the head version's bytes) with the removed lines of `diff` between them.
    pub fn build(diff: &FileDiff, head: Vec<u8>) -> Self {
        let head = BlobText::from_bytes(head);
        let (rows, removed) = unify(diff, head.line_count());
        let head_rows = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.kind != LineKind::Deleted)
            .map(|(index, _)| index as u32)
            .collect();
        let split_rows = pair(&rows);
        let max_line = rows.iter().map(|row| row.old_line.max(row.new_line)).max().unwrap_or(0);
        Self {
            head,
            removed: BlobText::from_bytes(removed.into_bytes()),
            rows,
            head_rows,
            split_rows,
            max_line,
            colors: OnceLock::new(),
        }
    }

    pub fn rows(&self) -> &[CodeRow] {
        &self.rows
    }

    /// Indices into [`Self::rows`] of the lines the head version has.
    pub fn head_rows(&self) -> &[u32] {
        &self.head_rows
    }

    /// The rows side by side.
    pub fn split_rows(&self) -> &[SplitRow] {
        &self.split_rows
    }

    /// The largest line number in either version, for the gutter's width.
    pub fn max_line(&self) -> u32 {
        self.max_line
    }

    pub fn text(&self, row: &CodeRow) -> &str {
        let line = match row.kind {
            LineKind::Deleted => self.removed.line(row.removed as usize),
            _ => row.new_line.checked_sub(1).and_then(|index| self.head.line(index as usize)),
        };
        line.unwrap_or_default()
    }

    /// The row's syntax spans once the file is colored; empty before and for plain files.
    pub fn spans(&self, row: &CodeRow) -> Option<&[Span]> {
        let colors = self.colors.get()?;
        match row.kind {
            LineKind::Deleted => colors.removed.line(row.removed as usize),
            _ => colors.head.line(row.new_line.checked_sub(1)? as usize),
        }
    }

    /// Queues the file's coloring by the grammar of `path`; `ctx` repaints when it is done.
    pub fn color(self: &Arc<Self>, path: &str, ctx: &egui::Context) {
        if self.head.len().max(self.removed.len()) > MAX_COLOR_BYTES {
            return;
        }
        let (file, path, ctx) = (self.clone(), path.to_owned(), ctx.clone());
        color_worker::submit(move || {
            let first_line = file.head.line(0).or_else(|| file.removed.line(0));
            let Some(grammar) = grammar::for_path(&path, first_line.unwrap_or_default()) else {
                return;
            };
            let working = grammar::compiled(grammar);
            let spans = |text: &BlobText| {
                let mut spans = LineSpans::default();
                highlight::highlight(text, grammar.syntax, &working, |chunk| {
                    spans.append(chunk);
                    true
                });
                spans.shrink_to_fit();
                spans
            };
            let colors = Colors { head: spans(&file.head), removed: spans(&file.removed) };
            // Set only here, once: a second coloring of the same file is never queued.
            let _first = file.colors.set(colors);
            ctx.request_repaint();
        });
    }
}

/// Walks `diff` (any context) along the `head_lines` lines of the head version: unchanged lines
/// between its lines advance both numbers, removed lines come in at their place. Returns the rows
/// and the removed lines' text, one per line.
fn unify(diff: &FileDiff, head_lines: usize) -> (Vec<CodeRow>, String) {
    let mut rows = Vec::with_capacity(head_lines + diff.lines().len());
    let mut removed = String::new();
    let mut removed_count = 0u32;
    let (mut old, mut new) = (1u32, 1u32);
    let unchanged = |rows: &mut Vec<CodeRow>, old: &mut u32, new: &mut u32| {
        rows.push(CodeRow { kind: LineKind::Context, old_line: *old, new_line: *new, removed: 0 });
        *old += 1;
        *new += 1;
    };
    for line in diff.lines() {
        match line.kind {
            LineKind::Hunk => {}
            LineKind::Context | LineKind::Added => {
                while new < line.new_line {
                    unchanged(&mut rows, &mut old, &mut new);
                }
                let old_line = if line.kind == LineKind::Context { line.old_line } else { 0 };
                rows.push(CodeRow {
                    kind: line.kind,
                    old_line,
                    new_line: line.new_line,
                    removed: 0,
                });
                new = line.new_line + 1;
                if old_line > 0 {
                    old = old_line + 1;
                }
            }
            LineKind::Deleted => {
                while old < line.old_line {
                    unchanged(&mut rows, &mut old, &mut new);
                }
                rows.push(CodeRow {
                    kind: LineKind::Deleted,
                    old_line: line.old_line,
                    new_line: 0,
                    removed: removed_count,
                });
                removed.push_str(diff.text(line));
                removed.push('\n');
                removed_count += 1;
                old = line.old_line + 1;
            }
        }
    }
    while (new as usize) <= head_lines {
        unchanged(&mut rows, &mut old, &mut new);
    }
    (rows, removed)
}

/// `rows` side by side, paired by the Files tab's own [`Pairs`]: the n-th removed line of a change
/// beside its n-th added one, an unchanged line on both sides.
fn pair(rows: &[CodeRow]) -> Vec<SplitRow> {
    let mut paired = Vec::with_capacity(rows.len());
    let mut pairs = Pairs::default();
    for (index, row) in rows.iter().enumerate() {
        match row.kind {
            LineKind::Deleted => pairs.deleted(&mut paired, index),
            LineKind::Added => pairs.added(index),
            LineKind::Context | LineKind::Hunk => {
                pairs.flush(&mut paired);
                let both = Some(index as u32);
                paired.push(Row { kind: RowKind::Line, left: both, right: both });
            }
        }
    }
    pairs.flush(&mut paired);
    paired.into_iter().map(|row| SplitRow { old: row.left, new: row.right }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base `a b c d e f`, head `a B c d e2 x f`: `b` replaced, `e` replaced and `x` added.
    fn sample() -> CodeFile {
        let diff = FileDiff::parse_unified(b"@@ -2 +2 @@\n-b\n+B\n@@ -5 +5,2 @@\n-e\n+e2\n+x\n");
        CodeFile::build(&diff, b"a\nB\nc\nd\ne2\nx\nf\n".to_vec())
    }

    #[test]
    fn unifies_the_head_with_the_removed_lines_in_place() {
        use LineKind::{Added, Context, Deleted};

        let file = sample();
        let shown: Vec<(LineKind, u32, u32, &str)> = file
            .rows()
            .iter()
            .map(|row| (row.kind, row.old_line, row.new_line, file.text(row)))
            .collect();
        assert_eq!(
            shown,
            [
                (Context, 1, 1, "a"),
                (Deleted, 2, 0, "b"),
                (Added, 0, 2, "B"),
                (Context, 3, 3, "c"),
                (Context, 4, 4, "d"),
                (Deleted, 5, 0, "e"),
                (Added, 0, 5, "e2"),
                (Added, 0, 6, "x"),
                (Context, 6, 7, "f"),
            ]
        );
        assert_eq!(file.head_rows().len(), 7, "New only leaves the two removed lines out");
        assert_eq!(file.max_line(), 7);
    }

    #[test]
    fn a_file_without_changes_is_all_context_and_a_deleted_file_all_removed() {
        let unchanged = CodeFile::build(&FileDiff::default(), b"x\ny\n".to_vec());
        assert!(unchanged.rows().iter().all(|row| row.kind == LineKind::Context));
        assert_eq!(unchanged.rows().len(), 2);
        let deleted =
            CodeFile::build(&FileDiff::parse_unified(b"@@ -1,2 +0,0 @@\n-x\n-y\n"), Vec::new());
        let texts: Vec<&str> = deleted.rows().iter().map(|row| deleted.text(row)).collect();
        assert_eq!(texts, ["x", "y"]);
        assert_eq!(deleted.head_rows(), [0u32; 0]);
    }
}
