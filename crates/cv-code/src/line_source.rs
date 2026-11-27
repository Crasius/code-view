//! Which file line a row shows in each pane, with the syntax spans highlighted for that line.
//! Diff lines map to the base blob (deleted lines, the left pane) or the head blob (added lines,
//! the right and inline panes); revealed gap lines come straight from the blobs.

use review_core::DiffSide;
use review_git::{FileDiff, LineKind};

use super::contents::FileContents;
use super::diff_rows::{Row, RowKind};
use cv_syntax::highlight::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Left,
    Right,
    Inline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeLine<'a> {
    pub text: &'a str,
    /// `Context` for revealed gap lines.
    pub kind: LineKind,
    /// Line numbers in base and head, 0 where the line does not exist.
    pub old_line: u32,
    pub new_line: u32,
    /// The number a side-by-side pane shows: base on the left and for deleted lines, head otherwise.
    pub number: u32,
    /// Empty while the line is not highlighted (yet), or for plain text.
    pub spans: &'a [Span],
    /// `spans` are the file's own, worked out from its first line; until they are, the colors
    /// by diff line stand in.
    pub has_file_spans: bool,
}

pub fn code_line<'a>(
    diff: &'a FileDiff,
    contents: Option<&'a FileContents>,
    row: &Row,
    pane: Pane,
) -> Option<CodeLine<'a>> {
    match row.kind {
        RowKind::Line => {
            let index = match pane {
                Pane::Left => row.left,
                Pane::Right => row.right,
                Pane::Inline => row.right.or(row.left),
            }?;
            Some(diff_line(diff, contents, index as usize, pane))
        }
        RowKind::Expanded => {
            let (old_line, new_line) = (row.left?, row.right?);
            let (side, number) = if pane == Pane::Left {
                (DiffSide::Old, old_line)
            } else {
                (DiffSide::New, new_line)
            };
            let side_contents = contents?.side(side)?;
            let index = number.checked_sub(1)? as usize;
            let spans = side_contents.spans.line(index);
            Some(CodeLine {
                text: side_contents.text.line(index)?,
                kind: LineKind::Context,
                old_line,
                new_line,
                number,
                spans: spans.unwrap_or_default(),
                has_file_spans: spans.is_some(),
            })
        }
        RowKind::Removal
        | RowKind::Gap(_)
        | RowKind::Separator
        | RowKind::Thread(_)
        | RowKind::Composer => None,
    }
}

/// Diff line `index` as `pane` shows it; deleted lines (also the ones behind a new-only removal
/// marker) come from the base blob.
pub fn diff_line<'a>(
    diff: &'a FileDiff,
    contents: Option<&'a FileContents>,
    index: usize,
    pane: Pane,
) -> CodeLine<'a> {
    let line = &diff.lines()[index];
    let text = diff.text(line);
    let (side, number) = match (line.kind, pane) {
        (LineKind::Deleted, _) | (LineKind::Context, Pane::Left) => (DiffSide::Old, line.old_line),
        _ => (DiffSide::New, line.new_line),
    };
    let spans = spans_for(contents, side, number, text);
    CodeLine {
        text,
        kind: line.kind,
        old_line: line.old_line,
        new_line: line.new_line,
        number,
        spans: spans.unwrap_or_default(),
        has_file_spans: spans.is_some(),
    }
}

/// The file's spans for its line `number` on `side`, once highlighted and only while it reads `text`.
fn spans_for<'a>(
    contents: Option<&'a FileContents>,
    side: DiffSide,
    number: u32,
    text: &str,
) -> Option<&'a [Span]> {
    let index = number.checked_sub(1)?;
    contents?.side(side)?.spans_for(index as usize, text)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use cv_base::theme::syntax::TokenKind;
    use cv_syntax::blob_text::BlobText;
    use cv_syntax::highlight::LineSpans;

    use crate::contents::{Coloring, SideContents};
    use crate::diff_rows::{Layout, rows_of};
    use crate::gaps::{self, Expand, Reveal};

    const OLD: &str = "one\ntwo\nthree\nfour\nfive\nsix\n";
    const NEW: &str = "one\ntwo\nthree\nfour\nFIVE\nsix\n";

    fn diff() -> FileDiff {
        FileDiff::parse_unified(b"@@ -4,3 +4,3 @@\n four\n-five\n+FIVE\n six\n")
    }

    /// Each line gets one span whose kind encodes the side and line: base lines `Comment`, head lines `String`,
    /// starting at the line number so the test can tell lines apart.
    fn side(text: &str, kind: TokenKind) -> SideContents {
        let text = BlobText::from_bytes(text.as_bytes().to_vec());
        let mut spans = LineSpans::default();
        for index in 0..text.line_count() {
            spans.push_line(&[Span::new(index + 1, kind)]);
        }
        SideContents { text: Arc::new(text), spans: Arc::new(spans) }
    }

    fn contents() -> FileContents {
        FileContents {
            old: Some(side(OLD, TokenKind::Comment)),
            new: Some(side(NEW, TokenKind::String)),
            coloring: Coloring::Done,
            error: None,
        }
    }

    fn mark(line: &CodeLine<'_>) -> Option<(usize, TokenKind)> {
        line.spans.first().map(|span| (span.start(), span.kind()))
    }

    #[test]
    fn maps_diff_lines_to_their_blob_lines() {
        let diff = diff();
        let contents = contents();
        let rows = rows_of(&diff, Layout::SideBySide, &[], &[], &[]).rows;
        let line = |row: usize, pane| code_line(&diff, Some(&contents), &rows[row], pane).unwrap();

        assert_eq!(
            mark(&line(0, Pane::Left)),
            Some((4, TokenKind::Comment)),
            "context, left: base line 4"
        );
        assert_eq!(
            mark(&line(0, Pane::Right)),
            Some((4, TokenKind::String)),
            "context, right: head line 4"
        );
        let deleted = line(1, Pane::Left);
        assert_eq!((deleted.text, mark(&deleted)), ("five", Some((5, TokenKind::Comment))));
        let added = line(1, Pane::Right);
        assert_eq!((added.text, mark(&added)), ("FIVE", Some((5, TokenKind::String))));
        assert_eq!(
            mark(&line(1, Pane::Inline)),
            Some((5, TokenKind::String)),
            "inline shows the head side"
        );
    }

    #[test]
    fn numbers_each_pane_by_its_own_side() {
        let diff = FileDiff::parse_unified(b"@@ -1,2 +1,3 @@\n a\n+b\n c\n");
        let rows = rows_of(&diff, Layout::SideBySide, &[], &[], &[]).rows;
        let number = |pane| code_line(&diff, None, &rows[2], pane).unwrap().number;
        assert_eq!((number(Pane::Left), number(Pane::Right)), (2, 3), "context after an insertion");
    }

    #[test]
    fn maps_revealed_gap_lines_to_both_blobs() {
        let diff = diff();
        let contents = contents();
        let gaps = gaps::find(&diff, Some(6));
        let mut reveals = vec![Reveal::default(); gaps.len()];
        reveals[0].expand(&gaps[0], Expand::All);
        let rows = rows_of(&diff, Layout::SideBySide, &gaps, &reveals, &[]).rows;
        assert_eq!(rows[0].kind, RowKind::Expanded);
        let left = code_line(&diff, Some(&contents), &rows[2], Pane::Left).unwrap();
        let right = code_line(&diff, Some(&contents), &rows[2], Pane::Right).unwrap();
        assert_eq!(
            (left.text, left.old_line, mark(&left)),
            ("three", 3, Some((3, TokenKind::Comment)))
        );
        assert_eq!(
            (right.text, right.new_line, mark(&right)),
            ("three", 3, Some((3, TokenKind::String)))
        );
        assert!(
            code_line(&diff, None, &rows[2], Pane::Right).is_none(),
            "no text before the blobs arrive"
        );
    }

    #[test]
    fn falls_back_to_plain_text() {
        let diff = diff();
        let rows = rows_of(&diff, Layout::Inline, &[], &[], &[]).rows;
        let unhighlighted = code_line(&diff, None, &rows[0], Pane::Inline).unwrap();
        assert_eq!(
            (unhighlighted.text, unhighlighted.spans, unhighlighted.has_file_spans),
            ("four", &[][..], false)
        );
        assert!(
            code_line(&diff, Some(&contents()), &rows[0], Pane::Inline).unwrap().has_file_spans
        );

        let mismatched = FileContents {
            new: Some(side("one\ntwo\nthree\nFOUR\n", TokenKind::String)),
            ..contents()
        };
        let line = code_line(&diff, Some(&mismatched), &rows[0], Pane::Inline).unwrap();
        assert!(line.spans.is_empty(), "spans of a different text are never applied");

        let partial = FileContents {
            new: Some(SideContents {
                spans: Arc::new(LineSpans::default()),
                ..side(NEW, TokenKind::String)
            }),
            ..contents()
        };
        let line = code_line(&diff, Some(&partial), &rows[0], Pane::Inline).unwrap();
        assert!(
            line.spans.is_empty() && !line.has_file_spans,
            "lines not highlighted yet have no file spans"
        );
    }
}
