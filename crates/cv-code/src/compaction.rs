//! Hunk compaction for the folder summary: a card shows the changed lines with at most a few
//! unchanged lines around them, whatever context the diff was made with (the file view's choice,
//! so both share one cached diff). Left-out lines become one thin separator row; hunk headers are
//! not shown. Rows are indices only, so a 20,000-line card costs 8 bytes per row.

use review_git::{FileDiff, LineKind};

/// Unchanged lines a card keeps on each side of a change, as Azure DevOps shows them.
pub const SUMMARY_CONTEXT: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactRow {
    /// A diff line, by its index in [`FileDiff::lines`].
    Line(u32),
    /// Unchanged lines left out between two shown runs.
    Separator,
}

/// The rows of `diff` in inline order with `context` unchanged lines around each change.
pub fn compact(diff: &FileDiff, context: u32) -> Vec<CompactRow> {
    let lines = diff.lines();
    let is_change = |kind: LineKind| matches!(kind, LineKind::Added | LineKind::Deleted);
    // Unchanged lines since the last change of the same hunk, and until the next one.
    let mut after = vec![u32::MAX; lines.len()];
    let mut distance = u32::MAX;
    for (index, line) in lines.iter().enumerate() {
        distance = match line.kind {
            LineKind::Hunk => u32::MAX,
            kind if is_change(kind) => 0,
            _ => distance.saturating_add(1),
        };
        after[index] = distance;
    }
    let mut keep = vec![false; lines.len()];
    distance = u32::MAX;
    for (index, line) in lines.iter().enumerate().rev() {
        distance = match line.kind {
            LineKind::Hunk => u32::MAX,
            kind if is_change(kind) => 0,
            _ => distance.saturating_add(1),
        };
        keep[index] = line.kind != LineKind::Hunk && after[index].min(distance) <= context;
    }

    let mut rows = Vec::with_capacity(lines.len());
    // Something was left out since the last row: a hunk boundary or dropped unchanged lines.
    // Before the first row nothing is marked: the card starts at its first change.
    let mut skipped = false;
    for (index, is_kept) in keep.iter().enumerate() {
        if *is_kept {
            if skipped && !rows.is_empty() {
                rows.push(CompactRow::Separator);
            }
            skipped = false;
            rows.push(CompactRow::Line(index as u32));
        } else {
            skipped = true;
        }
    }
    rows.shrink_to_fit();
    rows
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use review_git::{FileDiff, LineKind};

    use super::{CompactRow, compact};
    use crate::diff_rows::compact_shape;

    /// The rows as `+`, `-`, ` ` plus the line's text, and `~` for a separator.
    fn render(diff: &FileDiff, rows: &[CompactRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                CompactRow::Separator => "~".to_owned(),
                CompactRow::Line(index) => {
                    let line = &diff.lines()[*index as usize];
                    let marker = match line.kind {
                        LineKind::Added => '+',
                        LineKind::Deleted => '-',
                        LineKind::Context | LineKind::Hunk => ' ',
                    };
                    format!("{marker}{}", diff.text(line))
                }
            })
            .collect()
    }

    /// A hunk made with 10 lines of context: ten unchanged lines, a change, ten more.
    fn wide_context() -> FileDiff {
        let mut text = String::from("@@ -1,21 +1,21 @@\n");
        for line in 1..=10 {
            writeln!(text, " a{line}").unwrap();
        }
        text.push_str("-old\n+new\n");
        for line in 1..=10 {
            writeln!(text, " b{line}").unwrap();
        }
        FileDiff::parse_unified(text.as_bytes())
    }

    #[test]
    fn keeps_three_unchanged_lines_around_a_change() {
        let diff = wide_context();
        let rows = compact(&diff, 3);
        assert_eq!(
            render(&diff, &rows),
            [" a8", " a9", " a10", "-old", "+new", " b1", " b2", " b3"]
        );
        assert_eq!(
            compact_shape(&diff, 3, false),
            (8, 0),
            "nothing before the first or after the last shown line gets a separator"
        );
    }

    #[test]
    fn splits_a_hunk_where_unchanged_lines_are_left_out() {
        let text = concat!(
            "@@ -1,12 +1,12 @@\n",
            "-x\n",
            "+X\n",
            " 1\n",
            " 2\n",
            " 3\n",
            " 4\n",
            " 5\n",
            " 6\n",
            " 7\n",
            " 8\n",
            "-y\n",
            "+Y\n",
        );
        let diff = FileDiff::parse_unified(text.as_bytes());
        let rows = compact(&diff, 3);
        assert_eq!(
            render(&diff, &rows),
            ["-x", "+X", " 1", " 2", " 3", "~", " 6", " 7", " 8", "-y", "+Y"]
        );
        assert_eq!(compact_shape(&diff, 3, false), (10, 1));
        // Side by side, -x|+X and -y|+Y each share a row.
        assert_eq!(compact_shape(&diff, 3, true), (8, 1));
        // Exactly 2 × context unchanged lines between two changes stay one run.
        let close = concat!(
            "@@ -1,8 +1,8 @@\n",
            "-x\n",
            "+X\n",
            " 1\n",
            " 2\n",
            " 3\n",
            " 4\n",
            " 5\n",
            " 6\n",
            "-y\n",
            "+Y\n"
        );
        let diff = FileDiff::parse_unified(close.as_bytes());
        assert!(!compact(&diff, 3).contains(&CompactRow::Separator));
    }

    #[test]
    fn separates_hunks_but_not_the_top_of_the_file() {
        let text = concat!(
            "@@ -5,3 +5,3 @@ fn first()\n",
            " a\n",
            "-b\n",
            "+B\n",
            "@@ -60,2 +60,2 @@\n",
            " c\n",
            "-d\n",
            "+D\n"
        );
        let diff = FileDiff::parse_unified(text.as_bytes());
        let rows = compact(&diff, 3);
        assert_eq!(render(&diff, &rows), [" a", "-b", "+B", "~", " c", "-d", "+D"]);
    }

    #[test]
    fn shows_added_and_deleted_files_whole() {
        let added = FileDiff::parse_unified(b"@@ -0,0 +1,3 @@\n+a\n+b\n+c\n");
        assert_eq!(render(&added, &compact(&added, 3)), ["+a", "+b", "+c"]);
        let deleted = FileDiff::parse_unified(b"@@ -1,2 +0,0 @@\n-a\n-b\n");
        assert_eq!(render(&deleted, &compact(&deleted, 3)), ["-a", "-b"]);
    }

    #[test]
    fn honours_a_smaller_context_and_none_at_all() {
        let diff = wide_context();
        assert_eq!(render(&diff, &compact(&diff, 1)), [" a10", "-old", "+new", " b1"]);
        assert_eq!(render(&diff, &compact(&diff, 0)), ["-old", "+new"]);
        assert_eq!(compact(&FileDiff::default(), 3), [] as [CompactRow; 0]);
    }
}
