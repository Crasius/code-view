//! Tells whether every change of a diff is only whitespace, and which kind: line endings, the
//! newline at the end of the file, the byte order mark, trailing whitespace, indentation or the
//! spacing inside lines. Worked out once while the diff is parsed (off the UI thread), from the
//! changed lines only.
//!
//! Git lists a change as its deleted lines followed by the added lines that replace them. A
//! whitespace-only change replaces every line with one that reads the same once whitespace is
//! ignored, so the deleted and added lines pair up in order; a block where they do not (a line
//! more or less, other text) is a real change and the diff gets no summary.

use crate::file_diff::{DiffLine, FileDiff, LineKind};

/// Something at one end of the file that came or went: the final newline or the byte order mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeChange {
    Added,
    Removed,
}

/// What a whitespace-only diff changes, in changed lines per kind. A line can count for more
/// than one kind (a new line ending and trailing spaces, say).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WhitespaceSummary {
    /// Changed lines: deleted and added line pairs.
    pub lines: u32,
    /// Lines whose ending went from LF to CRLF.
    pub to_crlf: u32,
    /// Lines whose ending went from CRLF to LF.
    pub to_lf: u32,
    pub final_newline: Option<EdgeChange>,
    pub bom: Option<EdgeChange>,
    /// Lines that gained, lost or changed whitespace at their end.
    pub trailing: u32,
    /// Lines whose indentation went from tabs to spaces.
    pub tabs_to_spaces: u32,
    /// Lines whose indentation went from spaces to tabs.
    pub spaces_to_tabs: u32,
    /// Lines indented differently with the same kind of whitespace (2 → 4 spaces, say).
    pub reindented: u32,
    /// Lines whose spacing between words changed.
    pub inner: u32,
    /// Every line of the file changed: one change from its first line to its last, no unchanged line.
    pub whole_file: bool,
}

pub(crate) fn summarize(diff: &FileDiff) -> Option<WhitespaceSummary> {
    let lines = diff.lines();
    let mut summary = WhitespaceSummary::default();
    let mut hunks = 0;
    let mut starts_at_top = false;
    let mut has_context = false;
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        match line.kind {
            LineKind::Hunk => {
                hunks += 1;
                starts_at_top = line.old_line <= 1;
                index += 1;
            }
            LineKind::Context => {
                has_context = true;
                index += 1;
            }
            LineKind::Deleted | LineKind::Added => {
                let deleted = run_of(lines, index, LineKind::Deleted);
                let added = run_of(lines, index + deleted, LineKind::Added);
                if deleted != added {
                    return None;
                }
                for offset in 0..deleted {
                    let old = &lines[index + offset];
                    let new = &lines[index + deleted + offset];
                    if !count_pair(&mut summary, (old, diff.text(old)), (new, diff.text(new))) {
                        return None;
                    }
                }
                index += deleted + added;
            }
        }
    }
    if summary.lines == 0 {
        return None;
    }
    summary.whole_file = hunks == 1 && starts_at_top && !has_context;
    Some(summary)
}

/// How many lines of `kind` follow from `start` on.
fn run_of(lines: &[DiffLine], start: usize, kind: LineKind) -> usize {
    lines[start.min(lines.len())..].iter().take_while(|line| line.kind == kind).count()
}

/// Counts what changed between a deleted line and the added line that replaced it; `false` when
/// they differ in more than whitespace.
fn count_pair(
    summary: &mut WhitespaceSummary,
    (old, old_text): (&DiffLine, &str),
    (new, new_text): (&DiffLine, &str),
) -> bool {
    if old_text != new_text {
        let (old_lead, old_core, old_trail) = split(old_text);
        let (new_lead, new_core, new_trail) = split(new_text);
        if old_core == new_core {
            if old_lead != new_lead {
                match (old_lead.contains('\t'), new_lead.contains('\t')) {
                    (true, false) => summary.tabs_to_spaces += 1,
                    (false, true) => summary.spaces_to_tabs += 1,
                    _ => summary.reindented += 1,
                }
            }
            if old_trail != new_trail {
                summary.trailing += 1;
            }
        } else if without_whitespace(old_text).eq(without_whitespace(new_text)) {
            summary.inner += 1;
        } else {
            return false;
        }
    }
    if old.has_cr != new.has_cr {
        if new.has_cr {
            summary.to_crlf += 1;
        } else {
            summary.to_lf += 1;
        }
    }
    if old.missing_newline != new.missing_newline {
        summary.final_newline =
            Some(if new.missing_newline { EdgeChange::Removed } else { EdgeChange::Added });
    }
    if old.has_bom != new.has_bom {
        summary.bom = Some(if new.has_bom { EdgeChange::Added } else { EdgeChange::Removed });
    }
    summary.lines += 1;
    true
}

/// Whitespace as `git diff -w` sees it (C's `isspace` within a line).
fn is_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\u{b}' | '\u{c}' | '\r')
}

/// (leading whitespace, the rest up to the trailing whitespace, trailing whitespace).
fn split(text: &str) -> (&str, &str, &str) {
    let core_start = text.len() - text.trim_start_matches(is_whitespace).len();
    let core_end = text.trim_end_matches(is_whitespace).len().max(core_start);
    (&text[..core_start], &text[core_start..core_end], &text[core_end..])
}

fn without_whitespace(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().filter(|character| !is_whitespace(*character))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(unified: &str) -> Option<WhitespaceSummary> {
        FileDiff::parse_unified(unified.as_bytes()).whitespace().copied()
    }

    #[test]
    fn whole_file_lf_to_crlf() {
        let found = summary("@@ -1,3 +1,3 @@\n-one\n-two\n-three\n+one\r\n+two\r\n+three\r\n")
            .expect("whitespace only");
        assert_eq!((found.lines, found.to_crlf, found.to_lf, found.whole_file), (3, 3, 0, true));
        assert_eq!(
            found.trailing + found.inner + found.reindented,
            0,
            "the CR is a line ending, not trailing whitespace"
        );
    }

    #[test]
    fn mixed_line_endings_in_part_of_the_file() {
        let found =
            summary("@@ -1,4 +1,4 @@\n keep\n-a\r\n-b\n+a\n+b\r\n c\n").expect("whitespace only");
        assert_eq!((found.to_crlf, found.to_lf, found.whole_file), (1, 1, false));
    }

    #[test]
    fn final_newline_added_and_removed() {
        let added = summary("@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+two\n")
            .expect("whitespace only");
        assert_eq!(added.final_newline, Some(EdgeChange::Added));
        let removed = summary("@@ -1,2 +1,2 @@\n one\n-two\n+two\n\\ No newline at end of file\n")
            .expect("whitespace only");
        assert_eq!((removed.final_newline, removed.lines), (Some(EdgeChange::Removed), 1));
    }

    #[test]
    fn trailing_whitespace() {
        let found =
            summary("@@ -1,2 +1,2 @@\n-one  \n-two\n+one\n+two\t\n").expect("whitespace only");
        assert_eq!((found.trailing, found.lines), (2, 2));
    }

    #[test]
    fn indentation_kinds() {
        let tabs = summary("@@ -1,2 +1,2 @@\n-\tone\n-\t\ttwo\n+    one\n+        two\n")
            .expect("whitespace only");
        assert_eq!(tabs.tabs_to_spaces, 2);
        let spaces = summary("@@ -1 +1 @@\n-    one\n+\tone\n").expect("whitespace only");
        assert_eq!(spaces.spaces_to_tabs, 1);
        let deeper = summary("@@ -1 +1 @@\n-  one\n+    one\n").expect("whitespace only");
        assert_eq!(deeper.reindented, 1);
    }

    #[test]
    fn spacing_inside_lines() {
        let found = summary("@@ -1 +1 @@\n-let a = 1;\n+let  a=1;\n").expect("whitespace only");
        assert_eq!(found.inner, 1);
    }

    #[test]
    fn byte_order_mark_added_and_removed() {
        let removed =
            summary("@@ -1,2 +1,2 @@\n-\u{feff}one\n+one\n two\n").expect("whitespace only");
        assert_eq!(removed.bom, Some(EdgeChange::Removed));
        let added = summary("@@ -1 +1 @@\n-one\n+\u{feff}one\n").expect("whitespace only");
        assert_eq!(added.bom, Some(EdgeChange::Added));
        let diff = FileDiff::parse_unified("@@ -1 +1 @@\n-one\n+\u{feff}one\n".as_bytes());
        assert_eq!(diff.text(&diff.lines()[2]), "one", "the BOM is a flag, not text");
        assert!(diff.lines()[2].has_bom && !diff.lines()[1].has_bom);
    }

    #[test]
    fn a_bom_like_character_past_the_first_line_is_text() {
        let diff = FileDiff::parse_unified("@@ -5 +5 @@\n-one\n+\u{feff}one\n".as_bytes());
        assert!(diff.whitespace().is_none());
        assert_eq!(diff.text(&diff.lines()[2]), "\u{feff}one");
    }

    #[test]
    fn combined_kinds_count_separately() {
        let found = summary("@@ -1,2 +1,2 @@\n-\tone\n-two\n+    one  \r\n+two\r\n")
            .expect("whitespace only");
        assert_eq!(
            (found.tabs_to_spaces, found.trailing, found.to_crlf, found.lines),
            (1, 1, 2, 2)
        );
    }

    #[test]
    fn real_changes_get_no_summary() {
        assert!(summary("@@ -1 +1 @@\n-one\n+two\n").is_none(), "other text");
        assert!(summary("@@ -1,2 +1,3 @@\n one\n+\n two\n").is_none(), "an added blank line");
        assert!(summary("@@ -1,2 +1,1 @@\n-one\n-two\n+one\n").is_none(), "a line less");
        assert!(
            summary("@@ -1,2 +1,2 @@\n-one\n+one\r\n-two\n+three\n").is_none(),
            "one real change among whitespace ones"
        );
        assert!(summary("").is_none(), "no changes at all");
    }
}
