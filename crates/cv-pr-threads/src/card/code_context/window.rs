//! Which lines of a [`CodeFile`] an excerpt shows: the commented lines with [`CONTEXT`] lines on
//! each side, more where the user asked for them. Near a file's start or end the context the one
//! side cannot have goes to the other, so the default excerpt has the same height wherever the
//! comment sits (and its skeleton the height the code will have).

use std::ops::Range;

use review_core::{DiffSide, ThreadAnchor};
use review_git::LineKind;

use super::code_file::{CodeFile, CodeRow, SplitRow};

/// Lines shown above and below the commented ones by default.
pub const CONTEXT: usize = 3;
/// Lines one "Show more" adds.
pub const STEP: usize = 10;
/// How far Diff reaches beyond the commented lines into the changes around them, per side.
const HUNK_REACH: usize = 12;

/// How the excerpt shows the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The head version alone, its changed lines marked, as the diff's New only layout.
    NewOnly,
    /// Removed and added lines, unified.
    Diff,
    /// The base version on the left and the head version on the right, changes paired up.
    Split,
}

/// The lines a thread is on, in the version they are counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    side: DiffSide,
    first: u32,
    last: u32,
}

impl Focus {
    /// The commented lines of `anchor`; `None` when it names no line.
    pub fn of(anchor: &ThreadAnchor) -> Option<Self> {
        let side = match &anchor.range {
            Some(range) => range.side,
            None if anchor.new_line.is_some() => DiffSide::New,
            None => DiffSide::Old,
        };
        let last = anchor.line_in(side)?;
        let first = anchor.range.as_ref().and_then(|range| range.start_line_in(side));
        Some(Self { side, first: first.unwrap_or(last).min(last), last })
    }

    /// The number of lines, for a skeleton of the excerpt's height.
    pub fn line_count(&self) -> usize {
        (self.last - self.first) as usize + 1
    }

    /// The first commented line, for the excerpt's accessible name.
    pub fn first(&self) -> u32 {
        self.first
    }

    /// The comment is on the base version's lines: Split marks them on the left.
    pub fn is_old_side(&self) -> bool {
        self.side == DiffSide::Old
    }

    pub fn covers(&self, row: &CodeRow) -> bool {
        let line = match self.side {
            DiffSide::New if row.kind != LineKind::Deleted => row.new_line,
            DiffSide::Old if row.kind != LineKind::Added => row.old_line,
            DiffSide::New | DiffSide::Old => return false,
        };
        (self.first..=self.last).contains(&line)
    }
}

/// Lines the user added to the excerpt above and below the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Extra {
    pub above: usize,
    pub below: usize,
}

/// The lines one mode shows, as indices into [`CodeFile::rows`].
#[derive(Clone, Copy)]
pub struct Shown<'a> {
    file: &'a CodeFile,
    mode: Mode,
}

impl<'a> Shown<'a> {
    pub fn new(file: &'a CodeFile, mode: Mode) -> Self {
        Self { file, mode }
    }

    /// The row at `position` of the shown lines; Split's head row, or its base row where the
    /// head side is empty.
    pub fn row(&self, position: usize) -> &'a CodeRow {
        let index = match self.mode {
            Mode::NewOnly => self.file.head_rows()[position],
            Mode::Diff => position as u32,
            Mode::Split => {
                let split = self.file.split_rows()[position];
                split.new.or(split.old).unwrap_or_default()
            }
        };
        &self.file.rows()[index as usize]
    }

    /// The base and the head row at `position` of Split's lines; `None` for an empty side.
    pub fn sides(&self, position: usize) -> [Option<&'a CodeRow>; 2] {
        let SplitRow { old, new } = self.file.split_rows()[position];
        [old, new].map(|index| index.map(|index| &self.file.rows()[index as usize]))
    }

    /// The rows at `position`: one, or Split's two sides (one for an unchanged line).
    pub fn rows(&self, position: usize) -> impl Iterator<Item = &'a CodeRow> {
        let [old, new] = match self.mode {
            Mode::Split => self.sides(position),
            Mode::NewOnly | Mode::Diff => [None, Some(self.row(position))],
        };
        old.filter(|old| new != Some(*old)).into_iter().chain(new)
    }
}

/// What an excerpt shows: positions in its mode's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    pub lines: Range<usize>,
    /// Lines above and below the shown ones, for "Show more".
    pub hidden_above: usize,
    pub hidden_below: usize,
}

/// Whether a commented line is a removed one, which only Diff can show.
pub fn is_on_removed_lines(file: &CodeFile, focus: &Focus) -> bool {
    file.rows().iter().any(|row| row.kind == LineKind::Deleted && focus.covers(row))
}

/// The excerpt of `file` around `focus` in `mode`; `None` when the file has none of its lines.
pub fn excerpt(file: &CodeFile, mode: Mode, focus: &Focus, extra: Extra) -> Option<Excerpt> {
    let rows = file.rows();
    let first = rows.iter().position(|row| focus.covers(row))?;
    let last = rows.iter().rposition(|row| focus.covers(row))?;
    let (len, around) = match mode {
        Mode::Diff => (rows.len(), widen_to_changes(rows, first..last + 1)),
        Mode::Split => {
            // Diff's lines, on the paired rows that hold any of them.
            let changes = widen_to_changes(rows, first..last + 1);
            let split = file.split_rows();
            let touches = |row: &SplitRow| {
                [row.old, row.new]
                    .into_iter()
                    .flatten()
                    .any(|index| changes.contains(&(index as usize)))
            };
            let start = split.iter().position(touches)?;
            let end = split.iter().rposition(touches)? + 1;
            (split.len(), start..end)
        }
        Mode::NewOnly => {
            let head = file.head_rows();
            let commented: Vec<usize> = (0..head.len())
                .filter(|position| focus.covers(&rows[head[*position] as usize]))
                .collect();
            let around = if let (Some(start), Some(end)) = (commented.first(), commented.last()) {
                *start..*end + 1
            } else {
                // Only removed lines: where they were, between the head lines around them.
                let at = head.partition_point(|index| (*index as usize) < first);
                at..at
            };
            (head.len(), around)
        }
    };
    let lines = window(len, around, extra);
    Some(Excerpt { hidden_above: lines.start, hidden_below: len - lines.end, lines })
}

/// `focus` with [`CONTEXT`] lines on each side (moved inwards at the ends of `len` lines), grown
/// by `extra` up to the ends.
fn window(len: usize, focus: Range<usize>, extra: Extra) -> Range<usize> {
    let size = (focus.len() + 2 * CONTEXT).min(len);
    let start = focus.start.saturating_sub(CONTEXT).min(len - size);
    let end = start + size;
    start.saturating_sub(extra.above)..(end + extra.below).min(len)
}

/// `focus` grown into the changed lines right next to it, at most [`HUNK_REACH`] per side: the
/// change the comment is about, removed lines included.
fn widen_to_changes(rows: &[CodeRow], focus: Range<usize>) -> Range<usize> {
    let is_changed = |index: usize| rows[index].kind != LineKind::Context;
    let mut start = focus.start;
    while start > 0 && focus.start - start < HUNK_REACH && is_changed(start - 1) {
        start -= 1;
    }
    let mut end = focus.end;
    while end < rows.len() && end - focus.end < HUNK_REACH && is_changed(end) {
        end += 1;
    }
    start..end
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use review_core::LineRange;
    use review_git::FileDiff;

    use super::*;

    fn anchor(side: DiffSide, first: u32, last: u32) -> ThreadAnchor {
        let (old_line, new_line) = match side {
            DiffSide::New => (None, Some(last)),
            DiffSide::Old => (Some(last), None),
        };
        let range = (first != last).then(|| LineRange {
            start_old_line: (side == DiffSide::Old).then_some(first),
            start_new_line: (side == DiffSide::New).then_some(first),
            side,
            detail: None,
        });
        ThreadAnchor { path: "a.rs".into(), old_path: None, old_line, new_line, range }
    }

    fn focus(side: DiffSide, first: u32, last: u32) -> Focus {
        Focus::of(&anchor(side, first, last)).expect("a line")
    }

    /// 40 unchanged lines, then line 20 replaced by two.
    fn file() -> CodeFile {
        let head: String = (1..=41)
            .map(|line| match line {
                20 => "twenty a\n".to_owned(),
                21 => "twenty b\n".to_owned(),
                _ => format!("line {line}\n"),
            })
            .collect();
        let diff = FileDiff::parse_unified(b"@@ -20 +20,2 @@\n-line 20\n+twenty a\n+twenty b\n");
        CodeFile::build(&diff, head.into_bytes())
    }

    fn numbers(file: &CodeFile, mode: Mode, excerpt: &Excerpt) -> Vec<(u32, u32)> {
        let shown = Shown::new(file, mode);
        excerpt.lines.clone().map(|at| (shown.row(at).old_line, shown.row(at).new_line)).collect()
    }

    #[test]
    fn shows_three_lines_around_the_comment() {
        let file = file();
        let excerpt =
            excerpt(&file, Mode::NewOnly, &focus(DiffSide::New, 10, 10), Extra::default()).unwrap();
        let shown = numbers(&file, Mode::NewOnly, &excerpt);
        assert_eq!(
            shown.iter().map(|(_, new)| *new).collect::<Vec<_>>(),
            (7..=13).collect::<Vec<_>>()
        );
        assert_eq!((excerpt.hidden_above, excerpt.hidden_below), (6, 41 - 13));
    }

    #[test]
    fn keeps_its_height_at_the_start_and_end_of_the_file() {
        let file = file();
        let first =
            excerpt(&file, Mode::NewOnly, &focus(DiffSide::New, 1, 1), Extra::default()).unwrap();
        assert_eq!(first.lines, 0..7, "the context the top cannot have goes below");
        assert_eq!(first.hidden_above, 0);
        let last =
            excerpt(&file, Mode::NewOnly, &focus(DiffSide::New, 41, 41), Extra::default()).unwrap();
        assert_eq!(last.lines, 34..41);
        assert_eq!(last.hidden_below, 0);
        let tiny = CodeFile::build(&FileDiff::default(), b"one\ntwo\n".to_vec());
        let whole =
            excerpt(&tiny, Mode::NewOnly, &focus(DiffSide::New, 2, 2), Extra::default()).unwrap();
        assert_eq!(whole.lines, 0..2, "a short file shows whole");
    }

    #[test]
    fn a_range_thread_shows_all_its_lines_with_context() {
        let file = file();
        let excerpt =
            excerpt(&file, Mode::NewOnly, &focus(DiffSide::New, 10, 14), Extra::default()).unwrap();
        assert_eq!(excerpt.lines.len(), 5 + 2 * CONTEXT);
        let first_new = numbers(&file, Mode::NewOnly, &excerpt)[0].1;
        assert_eq!(first_new, 7);
    }

    #[test]
    fn expansion_grows_each_side_up_to_the_file_ends() {
        let file = file();
        let focus = focus(DiffSide::New, 10, 10);
        let grown =
            excerpt(&file, Mode::NewOnly, &focus, Extra { above: STEP, below: STEP }).unwrap();
        assert_eq!(grown.lines, 0..23, "6 lines above were left, so the top stops at line 1");
        let all = excerpt(&file, Mode::NewOnly, &focus, Extra { above: 100, below: 100 }).unwrap();
        assert_eq!(all.lines, 0..41);
        assert_eq!((all.hidden_above, all.hidden_below), (0, 0));
    }

    #[test]
    fn diff_shows_the_removed_lines_of_the_change_and_new_only_does_not() {
        let file = file();
        let on_added = focus(DiffSide::New, 20, 20);
        assert!(!is_on_removed_lines(&file, &on_added));
        let diff = excerpt(&file, Mode::Diff, &on_added, Extra::default()).unwrap();
        let shown = numbers(&file, Mode::Diff, &diff);
        assert!(shown.contains(&(20, 0)), "the removed line 20: {shown:?}");
        assert!(shown.contains(&(0, 21)), "the whole change: {shown:?}");
        assert_eq!(shown.first(), Some(&(17, 17)), "three lines before the change");
        let new_only = excerpt(&file, Mode::NewOnly, &on_added, Extra::default()).unwrap();
        assert!(numbers(&file, Mode::NewOnly, &new_only).iter().all(|(_, new)| *new > 0));
    }

    #[test]
    fn a_thread_on_a_removed_line_needs_diff() {
        let file = file();
        let removed = focus(DiffSide::Old, 20, 20);
        assert!(is_on_removed_lines(&file, &removed));
        let diff = excerpt(&file, Mode::Diff, &removed, Extra::default()).unwrap();
        let shown = Shown::new(&file, Mode::Diff);
        assert!(diff.lines.into_iter().any(|at| removed.covers(shown.row(at))));
        let new_only = excerpt(&file, Mode::NewOnly, &removed, Extra::default()).unwrap();
        assert_eq!(new_only.lines.len(), 2 * CONTEXT, "New only shows where the line was");
        let gone = Focus::of(&anchor(DiffSide::New, 90, 90)).unwrap();
        assert_eq!(excerpt(&file, Mode::NewOnly, &gone, Extra::default()), None);
    }

    /// `count` head lines `line N`, changed by `diff`.
    fn changed(count: u32, diff: &str) -> CodeFile {
        let head = (1..=count).fold(String::new(), |mut head, line| {
            writeln!(head, "line {line}").expect("writing to a String");
            head
        });
        CodeFile::build(&FileDiff::parse_unified(diff.as_bytes()), head.into_bytes())
    }

    /// Split's lines of `excerpt` as (base line, head line), 0 for an empty side.
    fn split(file: &CodeFile, excerpt: &Excerpt) -> Vec<(u32, u32)> {
        let shown = Shown::new(file, Mode::Split);
        excerpt
            .lines
            .clone()
            .map(|at| {
                let [old, new] = shown.sides(at);
                (old.map_or(0, |row| row.old_line), new.map_or(0, |row| row.new_line))
            })
            .collect()
    }

    #[test]
    fn split_shows_removed_lines_left_beside_nothing() {
        let file = changed(10, "@@ -5,2 +4,0 @@\n-gone a\n-gone b\n");
        let removed = focus(DiffSide::Old, 5, 5);
        let excerpt = excerpt(&file, Mode::Split, &removed, Extra::default()).unwrap();
        assert_eq!(
            split(&file, &excerpt),
            [(2, 2), (3, 3), (4, 4), (5, 0), (6, 0), (7, 5), (8, 6), (9, 7)]
        );
    }

    #[test]
    fn split_shows_added_lines_right_beside_nothing() {
        let file = changed(12, "@@ -4,0 +5,2 @@\n+line 5\n+line 6\n");
        let added = focus(DiffSide::New, 5, 5);
        let excerpt = excerpt(&file, Mode::Split, &added, Extra::default()).unwrap();
        assert_eq!(
            split(&file, &excerpt),
            [(2, 2), (3, 3), (4, 4), (0, 5), (0, 6), (5, 7), (6, 8), (7, 9)]
        );
    }

    #[test]
    fn split_pairs_a_replaced_line_with_the_first_of_its_two() {
        let file = file();
        let excerpt =
            excerpt(&file, Mode::Split, &focus(DiffSide::New, 20, 20), Extra::default()).unwrap();
        assert_eq!(
            split(&file, &excerpt),
            [(17, 17), (18, 18), (19, 19), (20, 20), (0, 21), (21, 22), (22, 23), (23, 24)]
        );
        let shown = Shown::new(&file, Mode::Split);
        let [old, new] = shown.sides(excerpt.lines.start + 3);
        assert_eq!(
            (old.map(|row| row.kind), new.map(|row| row.kind)),
            (Some(LineKind::Deleted), Some(LineKind::Added)),
            "the removed line beside the line that replaced it"
        );
        assert_eq!(shown.rows(excerpt.lines.start).count(), 1, "an unchanged line is one row");
        assert_eq!(shown.rows(excerpt.lines.start + 3).count(), 2);
    }

    #[test]
    fn split_grows_both_sides_together() {
        let file = file();
        let focus = focus(DiffSide::New, 20, 20);
        let grown =
            excerpt(&file, Mode::Split, &focus, Extra { above: STEP, below: STEP }).unwrap();
        assert_eq!(grown.lines, 6..34);
        let lines = split(&file, &grown);
        assert_eq!(lines.first(), Some(&(7, 7)));
        assert_eq!(lines.last(), Some(&(33, 34)));
        assert_eq!((grown.hidden_above, grown.hidden_below), (6, 41 - 34));
    }
}
