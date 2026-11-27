//! Turns a [`FileDiff`] into display rows: paired left/right lines for side-by-side, single
//! lines for inline, head lines plus zero-height removal markers for new only, a collapsed bar per
//! gap of unchanged lines (or the lines the user revealed), and comment threads plus the
//! new-comment composer slotted in after their line. The folder summary's compact layout keeps a
//! few unchanged lines around each change and stands a thin separator for the rest.
//! Rows only hold indices and line numbers, so a 100k-line diff costs ~24 bytes per row.

use std::ops::Range;

use review_core::{
    DiffSide, FilePositions, LineRange, RangeDetail, ReviewThread, ThreadAnchor, ThreadStatus,
};
use review_git::{DiffLine, FileDiff, LineKind};

use super::compaction::{self, CompactRow};
use super::gaps::{Gap, Reveal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Collapsed unchanged lines: index into the gap list given to [`build`].
    Gap(u32),
    /// `left`/`right` index into [`FileDiff::lines`]; side-by-side rows may have one side empty.
    Line,
    /// An unchanged line revealed from a gap: `left`/`right` are its base and head line numbers.
    Expanded,
    /// New only: consecutive deleted lines that are not shown, marked between the rows around
    /// them. `left..right` is their index range in [`FileDiff::lines`]; the row has no height.
    Removal,
    /// Index into the thread list given to [`build`].
    Thread(u32),
    Composer,
    /// Compact layout: unchanged lines left out between two shown runs.
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub left: Option<u32>,
    pub right: Option<u32>,
}

impl Row {
    fn line(left: Option<usize>, right: Option<usize>) -> Self {
        Self {
            kind: RowKind::Line,
            left: left.map(|index| index as u32),
            right: right.map(|index| index as u32),
        }
    }

    fn expanded(old_line: u32, new_line: u32) -> Self {
        Self { kind: RowKind::Expanded, left: Some(old_line), right: Some(new_line) }
    }

    fn marker(kind: RowKind) -> Self {
        Self { kind, left: None, right: None }
    }

    fn removal(lines: Range<usize>) -> Self {
        Self {
            kind: RowKind::Removal,
            left: Some(lines.start as u32),
            right: Some(lines.end as u32),
        }
    }

    /// Indices into [`FileDiff::lines`] of the deleted lines a removal row stands for.
    pub fn removed_lines(&self) -> Option<Range<usize>> {
        (self.kind == RowKind::Removal).then_some(())?;
        Some(self.left? as usize..self.right? as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    SideBySide,
    Inline,
    /// The head version only: changed lines marked, deleted lines behind a marker between rows.
    NewOnly,
    /// An added (`New`) or deleted (`Old`) file: its one version, full width and unmarked,
    /// whatever layout the user picked.
    WholeFile(DiffSide),
    /// The folder summary's cards: inline, with this many unchanged lines around each change and
    /// a separator for the lines left out.
    Compact(u32),
    /// The folder summary's cards side by side: the compact layout's lines, deleted and added
    /// lines paired up as [`Layout::SideBySide`] pairs them.
    CompactSplit(u32),
}

impl Layout {
    /// One pane of code instead of base and head next to each other.
    pub fn is_single_pane(self) -> bool {
        !matches!(self, Layout::SideBySide | Layout::CompactSplit(_))
    }

    /// The gutter shows base and head line numbers (inline); otherwise one number per pane.
    pub fn has_two_numbers(self) -> bool {
        matches!(self, Layout::Inline | Layout::Compact(_))
    }
}

/// Where the user is writing a new inline comment: its (last) line, and the first one of a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerAnchor {
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub range: Option<LineRange>,
    /// The card sits right above the range's first line instead of below its last: the lines were
    /// picked dragging up, so the pointer is at the top.
    pub above: bool,
}

/// Rows `start..=end` hold the lines of a multi-line comment, on `side` (side by side) or in the
/// one pane; the thread card sits right below `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeRows {
    pub start: usize,
    pub end: usize,
    pub side: DiffSide,
    /// The new comment being written, not a posted thread.
    pub is_draft: bool,
}

impl RangeRows {
    pub fn contains(&self, row: usize) -> bool {
        (self.start..=self.end).contains(&row)
    }
}

pub struct DiffRows {
    pub rows: Vec<Row>,
    /// Threads on this file whose line is not shown (outdated, or in a collapsed gap).
    pub detached_threads: Vec<usize>,
    /// First row of every block of consecutive changed lines, for next/previous change.
    pub changes: Vec<u32>,
    /// Lines of the multi-line threads and of a multi-line draft.
    pub ranges: Vec<RangeRows>,
    /// The thread (index into the thread list given to [`build`]) whose lines each of `ranges`
    /// holds; `None` for the draft's.
    pub range_threads: Vec<Option<u32>>,
}

pub struct RowInputs<'a> {
    pub diff: &'a FileDiff,
    pub layout: Layout,
    pub gaps: &'a [Gap],
    pub reveals: &'a [Reveal],
    pub threads: &'a [&'a ReviewThread],
    pub composer: Option<&'a ComposerAnchor>,
}

pub fn build(inputs: &RowInputs<'_>) -> DiffRows {
    let diff = inputs.diff;
    let mut rows = content_rows(inputs);

    let mut placements: Vec<(usize, RowKind)> = Vec::new();
    let mut detached_threads = Vec::new();
    for (thread_index, thread) in inputs.threads.iter().enumerate() {
        let Some(anchor) = thread.anchor.as_ref() else {
            continue;
        };
        match find_row(diff, &rows, anchor.old_line, anchor.new_line) {
            Some(row_index) if !thread.is_outdated => {
                placements.push((row_index, RowKind::Thread(thread_index as u32)));
            }
            _ => detached_threads.push(thread_index),
        }
    }
    // Right above the first line of the range: before that row, after any card of the row above.
    let mut composer_before = None;
    if let Some(composer) = inputs.composer
        && let Some(row_index) = find_row(diff, &rows, composer.old_line, composer.new_line)
    {
        let start = composer.range.as_ref().filter(|_| composer.above).and_then(|range| {
            find_row(diff, &rows[..=row_index], range.start_old_line, range.start_new_line)
        });
        match start {
            Some(start) => composer_before = Some(start),
            None => placements.push((row_index, RowKind::Composer)),
        }
    }

    if !placements.is_empty() || composer_before.is_some() {
        placements.sort_by_key(|(row_index, _)| *row_index);
        let mut merged = Vec::with_capacity(rows.len() + placements.len() + 1);
        let mut next_placement = placements.into_iter().peekable();
        for (row_index, row) in rows.drain(..).enumerate() {
            if composer_before == Some(row_index) {
                merged.push(Row::marker(RowKind::Composer));
            }
            merged.push(row);
            while let Some((_, kind)) = next_placement.next_if(|(target, _)| *target == row_index) {
                merged.push(Row::marker(kind));
            }
        }
        rows = merged;
    }
    let changes = change_starts(diff, &rows);
    let (ranges, range_threads) = range_rows(diff, &rows, inputs).into_iter().unzip();
    DiffRows { rows, detached_threads, changes, ranges, range_threads }
}

/// Where the lines of each placed active thread (and of the draft) are: from the row of its first
/// line down to the code row its card follows, with the thread. Resolved threads leave their
/// lines plain.
fn range_rows(
    diff: &FileDiff,
    rows: &[Row],
    inputs: &RowInputs<'_>,
) -> Vec<(RangeRows, Option<u32>)> {
    let mut ranges = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let (range, is_new_side, thread_index) = match row.kind {
            RowKind::Thread(thread_index) => {
                let thread = inputs.threads[thread_index as usize];
                let Some(anchor) =
                    thread.anchor.as_ref().filter(|_| thread.status == ThreadStatus::Active)
                else {
                    continue;
                };
                (anchor.range.as_ref(), anchor.new_line.is_some(), Some(thread_index))
            }
            RowKind::Composer => {
                let Some(composer) = inputs.composer else {
                    continue;
                };
                // A card above the lines: they follow it, from its row down.
                let below = |old_line, new_line| {
                    find_row(diff, &rows[index..], old_line, new_line).map(|row| row + index)
                };
                if composer.above
                    && let Some(range) = composer.range.as_ref()
                    && let (Some(start), Some(end)) = (
                        below(range.start_old_line, range.start_new_line),
                        below(composer.old_line, composer.new_line),
                    )
                {
                    ranges.push((RangeRows { start, end, side: range.side, is_draft: true }, None));
                    continue;
                }
                (composer.range.as_ref(), composer.new_line.is_some(), None)
            }
            _ => continue,
        };
        let Some(end) = rows[..index]
            .iter()
            .rposition(|row| !matches!(row.kind, RowKind::Thread(_) | RowKind::Composer))
        else {
            continue;
        };
        let (start, side) = match range {
            Some(range) => {
                match find_row(diff, &rows[..=end], range.start_old_line, range.start_new_line) {
                    Some(start) => (start, range.side),
                    None => continue,
                }
            }
            None => (end, if is_new_side { DiffSide::New } else { DiffSide::Old }),
        };
        ranges
            .push((RangeRows { start, end, side, is_draft: thread_index.is_none() }, thread_index));
    }
    ranges
}

fn content_rows(inputs: &RowInputs<'_>) -> Vec<Row> {
    match inputs.layout {
        Layout::Compact(context) => return compact_rows(inputs.diff, context, false),
        Layout::CompactSplit(context) => return compact_rows(inputs.diff, context, true),
        Layout::SideBySide | Layout::Inline | Layout::NewOnly | Layout::WholeFile(_) => {}
    }
    let lines = inputs.diff.lines();
    let mut rows = Vec::with_capacity(lines.len() + inputs.gaps.len() * 2);
    let mut pairs = Pairs::default();
    // New only: the run of deleted lines that becomes one removal marker.
    let mut removed: Option<Range<usize>> = None;
    let flush = |rows: &mut Vec<Row>, pairs: &mut Pairs, removed: &mut Option<Range<usize>>| {
        pairs.flush(rows);
        if let Some(run) = removed.take() {
            rows.push(Row::removal(run));
        }
    };
    let mut next_gap = 0;
    for (index, line) in lines.iter().enumerate() {
        match (line.kind, inputs.layout) {
            (LineKind::Hunk, _) => {
                flush(&mut rows, &mut pairs, &mut removed);
                while let Some(gap) = inputs
                    .gaps
                    .get(next_gap)
                    .filter(|gap| gap.hunk.is_some_and(|hunk| hunk as usize <= index))
                {
                    push_gap(
                        &mut rows,
                        next_gap,
                        gap,
                        inputs.reveals.get(next_gap).copied().unwrap_or_default(),
                    );
                    next_gap += 1;
                }
            }
            (LineKind::Context, _) => {
                flush(&mut rows, &mut pairs, &mut removed);
                rows.push(Row::line(Some(index), Some(index)));
            }
            (LineKind::Deleted, Layout::SideBySide) => pairs.deleted(&mut rows, index),
            (LineKind::Added, Layout::SideBySide) => pairs.added(index),
            (LineKind::Deleted, _) if inputs.layout != Layout::NewOnly => {
                rows.push(Row::line(Some(index), None));
            }
            (LineKind::Added, _) if inputs.layout != Layout::NewOnly => {
                rows.push(Row::line(None, Some(index)));
            }
            (LineKind::Deleted, _) => {
                let run = removed.get_or_insert(index..index);
                run.end = index + 1;
            }
            (LineKind::Added, _) => {
                flush(&mut rows, &mut pairs, &mut removed);
                rows.push(Row::line(None, Some(index)));
            }
        }
    }
    flush(&mut rows, &mut pairs, &mut removed);
    for (gap_index, gap) in inputs.gaps.iter().enumerate().skip(next_gap) {
        push_gap(
            &mut rows,
            gap_index,
            gap,
            inputs.reveals.get(gap_index).copied().unwrap_or_default(),
        );
    }
    rows
}

/// Side by side: the deleted and added lines of the current change, paired up row by row on flush
/// (the n-th deleted line beside the n-th added one). Shared by the file view, the folder summary's
/// split cards and the thread cards' Split excerpts, so all pair lines (and so word highlights) the
/// same way.
#[derive(Default)]
pub struct Pairs {
    deleted: Vec<usize>,
    added: Vec<usize>,
}

impl Pairs {
    /// A deleted line after added ones starts a new change.
    pub fn deleted(&mut self, rows: &mut Vec<Row>, index: usize) {
        if !self.added.is_empty() {
            self.flush(rows);
        }
        self.deleted.push(index);
    }

    pub fn added(&mut self, index: usize) {
        self.added.push(index);
    }

    pub fn flush(&mut self, rows: &mut Vec<Row>) {
        let pairs = self.deleted.len().max(self.added.len());
        for pair in 0..pairs {
            rows.push(Row::line(self.deleted.get(pair).copied(), self.added.get(pair).copied()));
        }
        self.deleted.clear();
        self.added.clear();
    }
}

/// The compact layout's rows: the lines near the changes, a separator where lines are left out;
/// `split` pairs each change's deleted and added lines side by side.
fn compact_rows(diff: &FileDiff, context: u32, split: bool) -> Vec<Row> {
    let lines = diff.lines();
    let compact = compaction::compact(diff, context);
    let mut rows = Vec::with_capacity(compact.len());
    let mut pairs = Pairs::default();
    for row in compact {
        let index = match row {
            CompactRow::Separator => {
                pairs.flush(&mut rows);
                rows.push(Row::marker(RowKind::Separator));
                continue;
            }
            CompactRow::Line(index) => index as usize,
        };
        match (lines[index].kind, split) {
            (LineKind::Deleted, true) => pairs.deleted(&mut rows, index),
            (LineKind::Added, true) => pairs.added(index),
            (LineKind::Deleted, false) => rows.push(Row::line(Some(index), None)),
            (LineKind::Added, false) => rows.push(Row::line(None, Some(index))),
            (LineKind::Context | LineKind::Hunk, _) => {
                pairs.flush(&mut rows);
                rows.push(Row::line(Some(index), Some(index)));
            }
        }
    }
    pairs.flush(&mut rows);
    rows.shrink_to_fit();
    rows
}

/// Code rows and separators of a compact card, laid out inline or side by side.
pub fn compact_shape(diff: &FileDiff, context: u32, split: bool) -> (u32, u32) {
    let rows = compact_rows(diff, context, split);
    let separators = rows.iter().filter(|row| row.kind == RowKind::Separator).count() as u32;
    (rows.len() as u32 - separators, separators)
}

/// Deleted lines paired with the added line that replaced them, as side-by-side rows pair them;
/// inline rows use the same pairs for word highlights.
pub fn change_pairs(diff: &FileDiff) -> Vec<(u32, u32)> {
    let inputs = RowInputs {
        diff,
        layout: Layout::SideBySide,
        gaps: &[],
        reveals: &[],
        threads: &[],
        composer: None,
    };
    content_rows(&inputs)
        .into_iter()
        .filter_map(|row| match (row.left, row.right) {
            (Some(left), Some(right)) if left != right => Some((left, right)),
            _ => None,
        })
        .collect()
}

/// Revealed lines at the top, the collapsed bar while anything stays hidden, revealed lines at the bottom.
fn push_gap(rows: &mut Vec<Row>, gap_index: usize, gap: &Gap, reveal: Reveal) {
    let top = reveal.top.min(gap.len);
    let bottom = reveal.bottom.min(gap.len - top);
    for offset in 0..top {
        rows.push(Row::expanded(gap.old_start + offset, gap.new_start + offset));
    }
    if top + bottom < gap.len {
        rows.push(Row::marker(RowKind::Gap(gap_index as u32)));
    }
    for offset in gap.len - bottom..gap.len {
        rows.push(Row::expanded(gap.old_start + offset, gap.new_start + offset));
    }
}

fn left_line<'a>(lines: &'a [DiffLine], row: &Row) -> Option<&'a DiffLine> {
    (row.kind == RowKind::Line).then_some(())?;
    row.left.map(|index| &lines[index as usize])
}

fn right_line<'a>(lines: &'a [DiffLine], row: &Row) -> Option<&'a DiffLine> {
    (row.kind == RowKind::Line).then_some(())?;
    row.right.map(|index| &lines[index as usize])
}

/// New-side lines win: a comment on a context line is placed by its head line number.
pub fn find_row(
    diff: &FileDiff,
    rows: &[Row],
    old_line: Option<u32>,
    new_line: Option<u32>,
) -> Option<usize> {
    let lines = diff.lines();
    if let Some(wanted) = new_line {
        let found = rows.iter().position(|row| match row.kind {
            RowKind::Expanded => row.right == Some(wanted),
            _ => right_line(lines, row).is_some_and(|line| line.new_line == wanted),
        });
        if found.is_some() {
            return found;
        }
    }
    let wanted = old_line?;
    rows.iter().position(|row| match row.kind {
        RowKind::Expanded => row.left == Some(wanted),
        // New only: a comment on a deleted line sits at the marker of its removal.
        RowKind::Removal => row
            .removed_lines()
            .is_some_and(|mut removed| removed.any(|index| lines[index].old_line == wanted)),
        _ => left_line(lines, row)
            .is_some_and(|line| line.old_line == wanted && line.kind != LineKind::Added),
    })
}

fn is_change(diff: &FileDiff, row: &Row) -> bool {
    let lines = diff.lines();
    let is_deleted = left_line(lines, row).is_some_and(|line| line.kind == LineKind::Deleted);
    let is_added = right_line(lines, row).is_some_and(|line| line.kind == LineKind::Added);
    is_deleted || is_added || row.kind == RowKind::Removal
}

fn change_starts(diff: &FileDiff, rows: &[Row]) -> Vec<u32> {
    let mut starts = Vec::new();
    let mut was_change = false;
    for (index, row) in rows.iter().enumerate() {
        let is_change = is_change(diff, row);
        if is_change && !was_change {
            starts.push(index as u32);
        }
        // Threads between changed lines do not split a change.
        if !matches!(row.kind, RowKind::Thread(_) | RowKind::Composer) {
            was_change = is_change;
        }
    }
    starts
}

/// The anchor a new comment on this row's side gets.
pub fn anchor_for(
    diff: &FileDiff,
    row: &Row,
    prefer_right: bool,
    path: &str,
    old_path: Option<&str>,
) -> Option<ThreadAnchor> {
    let (old_line, new_line) = match row.kind {
        RowKind::Expanded => (row.left, row.right),
        RowKind::Line => {
            let lines = diff.lines();
            let side = if prefer_right { row.right.or(row.left) } else { row.left.or(row.right) };
            let line = &lines[side? as usize];
            match line.kind {
                LineKind::Added => (None, Some(line.new_line)),
                LineKind::Deleted => (Some(line.old_line), None),
                LineKind::Context => (Some(line.old_line), Some(line.new_line)),
                LineKind::Hunk => return None,
            }
        }
        RowKind::Removal => {
            // A comment on removed lines anchors at the last of them, right above the marker's card.
            let last = row.removed_lines()?.last()?;
            (Some(diff.lines()[last].old_line), None)
        }
        RowKind::Gap(_) | RowKind::Separator | RowKind::Thread(_) | RowKind::Composer => {
            return None;
        }
    };
    Some(ThreadAnchor {
        path: path.to_owned(),
        old_path: old_path.map(str::to_owned),
        old_line,
        new_line,
        range: None,
    })
}

/// The lines `start_row..=end_row` picked in one pane, for a multi-line comment.
pub struct RangePick {
    pub start_row: usize,
    pub end_row: usize,
    /// The head side of a row wins (right pane, single pane), as for one-line comments.
    pub prefer_right: bool,
    /// The file the lines were picked in.
    pub side: DiffSide,
    /// 1-based character offsets of the first selected character and just past the last one.
    pub offsets: (u32, u32),
}

/// The anchor of a comment on several lines: the last line as the anchor's line, the first as the
/// range start, and where both sit in either file (GitLab line codes need that). A pick of one
/// line gives a plain one-line anchor.
pub fn range_anchor(
    diff: &FileDiff,
    rows: &[Row],
    pick: &RangePick,
    path: &str,
    old_path: Option<&str>,
) -> Option<ThreadAnchor> {
    let end = anchor_for(diff, rows.get(pick.end_row)?, pick.prefer_right, path, old_path)?;
    if pick.start_row >= pick.end_row {
        return Some(end);
    }
    let start = anchor_for(diff, rows.get(pick.start_row)?, pick.prefer_right, path, old_path)?;
    let detail = RangeDetail {
        start: file_positions(diff, &rows[pick.start_row], pick.prefer_right)?,
        end: file_positions(diff, &rows[pick.end_row], pick.prefer_right)?,
        start_offset: pick.offsets.0,
        end_offset: pick.offsets.1,
    };
    Some(ThreadAnchor {
        range: Some(LineRange {
            start_old_line: start.old_line,
            start_new_line: start.new_line,
            side: pick.side,
            detail: Some(detail),
        }),
        ..end
    })
}

/// Where a code row's line sits in both files; a line missing from one file counts the line it
/// comes before there (the hunk header gives the count at a hunk's top). `prefer_right` picks the
/// line of a side-by-side row as for comments.
pub fn file_positions(diff: &FileDiff, row: &Row, prefer_right: bool) -> Option<FilePositions> {
    match row.kind {
        RowKind::Expanded => Some(FilePositions { old: row.left?, new: row.right? }),
        RowKind::Line => {
            let lines = diff.lines();
            let index = if prefer_right { row.right.or(row.left) } else { row.left.or(row.right) }?
                as usize;
            let line = &lines[index];
            let counted_before = |number: fn(&DiffLine) -> u32, exists: fn(LineKind) -> bool| {
                lines[..index].iter().rev().find_map(|earlier| match earlier.kind {
                    LineKind::Hunk => Some(number(earlier)),
                    kind if exists(kind) => Some(number(earlier) + 1),
                    _ => None,
                })
            };
            match line.kind {
                LineKind::Context => Some(FilePositions { old: line.old_line, new: line.new_line }),
                LineKind::Added => {
                    let old = counted_before(|line| line.old_line, |kind| kind != LineKind::Added)?;
                    Some(FilePositions { old, new: line.new_line })
                }
                LineKind::Deleted => {
                    let new =
                        counted_before(|line| line.new_line, |kind| kind != LineKind::Deleted)?;
                    Some(FilePositions { old: line.old_line, new })
                }
                LineKind::Hunk => None,
            }
        }
        RowKind::Gap(_)
        | RowKind::Separator
        | RowKind::Removal
        | RowKind::Thread(_)
        | RowKind::Composer => None,
    }
}

/// Rows `start..=end` run across more than one hunk: a collapsed or revealed gap lies between
/// them, or their diff lines include a hunk header. GitHub takes a multi-line comment only
/// within one hunk.
pub fn spans_hunks(diff: &FileDiff, rows: &[Row], start: usize, end: usize) -> bool {
    let mut indices = None::<(u32, u32)>;
    for row in rows.get(start..=end).unwrap_or_default() {
        match row.kind {
            RowKind::Gap(_) | RowKind::Expanded => return true,
            RowKind::Line | RowKind::Removal => {
                for index in [row.left, row.right].into_iter().flatten() {
                    indices =
                        Some(indices.map_or((index, index), |(low, high)| {
                            (low.min(index), high.max(index))
                        }));
                }
            }
            // Left-out lines of a compact diff: a hunk header among them shows in the indices.
            RowKind::Separator | RowKind::Thread(_) | RowKind::Composer => {}
        }
    }
    let Some((low, high)) = indices else {
        return false;
    };
    diff.lines()[low as usize..=high as usize].iter().any(|line| line.kind == LineKind::Hunk)
}

/// The rows of `diff` without a comment being written.
#[cfg(test)]
pub(crate) fn rows_of(
    diff: &FileDiff,
    layout: Layout,
    gaps: &[Gap],
    reveals: &[Reveal],
    threads: &[&ReviewThread],
) -> DiffRows {
    build(&RowInputs { diff, layout, gaps, reveals, threads, composer: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaps::{self, Expand};

    /// Head line number a code row shows.
    fn new_number(diff: &FileDiff, row: &Row) -> Option<u32> {
        match row.kind {
            RowKind::Expanded => row.right,
            RowKind::Line => Some(diff.lines()[row.right.or(row.left)? as usize].new_line),
            RowKind::Removal
            | RowKind::Gap(_)
            | RowKind::Separator
            | RowKind::Thread(_)
            | RowKind::Composer => None,
        }
    }

    fn sample() -> FileDiff {
        let text = concat!(
            "@@ -1,4 +1,4 @@\n",
            " keep\n",
            "-old a\n",
            "-old b\n",
            "+new a\n",
            " tail\n",
            "+added end\n",
        );
        FileDiff::parse_unified(text.as_bytes())
    }

    #[test]
    fn pairs_deletions_with_additions_side_by_side() {
        let diff = sample();
        let rows = rows_of(&diff, Layout::SideBySide, &[], &[], &[]).rows;
        let shape: Vec<(RowKind, bool, bool)> =
            rows.iter().map(|row| (row.kind, row.left.is_some(), row.right.is_some())).collect();
        assert_eq!(
            shape,
            [
                (RowKind::Line, true, true),
                (RowKind::Line, true, true),
                (RowKind::Line, true, false),
                (RowKind::Line, true, true),
                (RowKind::Line, false, true),
            ]
        );
    }

    #[test]
    fn pairs_only_replaced_lines_for_word_highlights() {
        // Line 0 is the hunk header; "-old a" (2) was replaced by "+new a" (4), "-old b" has no partner.
        assert_eq!(change_pairs(&sample()), [(2, 4)]);
        assert_eq!(change_pairs(&two_hunks()), [(2, 3), (6, 7)]);
    }

    #[test]
    fn inline_keeps_one_line_per_row() {
        let diff = sample();
        let rows = rows_of(&diff, Layout::Inline, &[], &[], &[]).rows;
        assert_eq!(rows.len(), diff.lines().len() - 1, "every line but the hunk header");
    }

    #[test]
    fn marks_the_start_of_each_change() {
        let diff = sample();
        let inline = rows_of(&diff, Layout::Inline, &[], &[], &[]);
        assert_eq!(inline.changes, [1, 5]);
        let split = rows_of(&diff, Layout::SideBySide, &[], &[], &[]);
        assert_eq!(split.changes, [1, 4]);
    }

    #[test]
    fn new_only_shows_head_lines_and_marks_each_removal() {
        let diff = sample();
        let result = rows_of(&diff, Layout::NewOnly, &[], &[], &[]);
        assert_eq!(
            result.rows,
            [
                Row::line(Some(1), Some(1)),
                Row::removal(2..4),
                Row::line(None, Some(4)),
                Row::line(Some(5), Some(5)),
                Row::line(None, Some(6)),
            ]
        );
        assert_eq!(result.rows[1].removed_lines(), Some(2..4));
        assert_eq!(result.changes, [1, 4], "a replacement is one change, starting at its removal");
    }

    #[test]
    fn new_only_places_removals_before_gaps_and_threads_at_their_marker() {
        let diff = FileDiff::parse_unified(
            concat!(
                "@@ -5,3 +5,1 @@\n",
                " a\n",
                "-b\n",
                "-c\n",
                "@@ -60,1 +58,2 @@\n",
                " d\n",
                "+e\n"
            )
            .as_bytes(),
        );
        let gaps = gaps::find(&diff, Some(59));
        let on_removed = on_old(7);
        let threads = [&on_removed];
        let result = rows_of(&diff, Layout::NewOnly, &gaps, &[], &threads);
        let kinds: Vec<RowKind> = result.rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Gap(0),
                RowKind::Line,
                RowKind::Removal,
                RowKind::Thread(0),
                RowKind::Gap(1),
                RowKind::Line,
                RowKind::Line,
            ]
        );
        assert_eq!(result.detached_threads, [0; 0]);
        let anchor = anchor_for(&diff, &result.rows[2], true, "a.rs", None).unwrap();
        assert_eq!(
            (anchor.old_line, anchor.new_line),
            (Some(7), None),
            "comments on removed lines anchor at the last one"
        );
    }

    fn two_hunks() -> FileDiff {
        let text = concat!(
            "@@ -5,2 +5,2 @@ fn first()\n",
            " a\n",
            "-b\n",
            "+B\n",
            "@@ -60,2 +60,2 @@\n",
            " c\n",
            "-d\n",
            "+D\n"
        );
        FileDiff::parse_unified(text.as_bytes())
    }

    #[test]
    fn collapses_gaps_and_reveals_them_from_either_end() {
        let diff = two_hunks();
        let gaps = gaps::find(&diff, Some(70));
        assert_eq!(gaps.len(), 3);
        let collapsed = rows_of(&diff, Layout::Inline, &gaps, &[], &[]).rows;
        let kinds: Vec<RowKind> = collapsed.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Gap(0),
                RowKind::Line,
                RowKind::Line,
                RowKind::Line,
                RowKind::Gap(1),
                RowKind::Line,
                RowKind::Line,
                RowKind::Line,
                RowKind::Gap(2),
            ]
        );

        let mut reveals = vec![Reveal::default(); gaps.len()];
        reveals[1].expand(&gaps[1], Expand::Down);
        reveals[1].expand(&gaps[1], Expand::Up);
        let partly = rows_of(&diff, Layout::Inline, &gaps, &reveals, &[]).rows;
        let middle: Vec<&Row> = partly.iter().skip(4).take(41).collect();
        assert_eq!(*middle[0], Row::expanded(7, 7), "first line below the first hunk");
        assert_eq!(middle[20].kind, RowKind::Gap(1), "{} lines stay hidden", gaps[1].len - 40);
        assert_eq!(*middle[21], Row::expanded(40, 40));
        assert_eq!(*middle[40], Row::expanded(59, 59), "last line above the second hunk");

        reveals[1].expand(&gaps[1], Expand::All);
        let open = rows_of(&diff, Layout::Inline, &gaps, &reveals, &[]).rows;
        assert!(
            !open.iter().any(|row| row.kind == RowKind::Gap(1)),
            "a fully revealed gap disappears"
        );
        let numbers: Vec<u32> = open.iter().filter_map(|row| new_number(&diff, row)).collect();
        let middle_numbers: Vec<u32> =
            numbers.iter().copied().skip_while(|line| *line != 6).take(55).collect();
        assert_eq!(middle_numbers, (6..61).collect::<Vec<u32>>(), "lines run on without a break");
    }

    #[test]
    fn places_threads_on_revealed_lines_and_detaches_hidden_ones() {
        let diff = two_hunks();
        let gaps = gaps::find(&diff, Some(70));
        let hidden = on_new(20);
        let threads = [&hidden];
        let collapsed = rows_of(&diff, Layout::Inline, &gaps, &[], &threads);
        assert_eq!(collapsed.detached_threads, [0]);
        let mut reveals = vec![Reveal::default(); gaps.len()];
        reveals[1].expand(&gaps[1], Expand::All);
        let revealed = rows_of(&diff, Layout::Inline, &gaps, &reveals, &threads);
        assert_eq!(revealed.detached_threads, [0; 0]);
        let position = revealed.rows.iter().position(|row| row.kind == RowKind::Thread(0)).unwrap();
        assert_eq!(revealed.rows[position - 1], Row::expanded(20, 20));
    }

    #[test]
    fn places_threads_after_their_line_and_detaches_unknown_lines() {
        let diff = sample();
        let on_new_line = on_new(2);
        let on_deleted = on_old(3);
        let outside = on_new(99);
        let threads = [&on_new_line, &on_deleted, &outside];
        let result = rows_of(&diff, Layout::Inline, &[], &[], &threads);
        let positions: Vec<usize> = result
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row.kind, RowKind::Thread(_)))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(positions.len(), 2);
        assert_eq!(result.detached_threads, [2]);
        let new_line_row = result.rows[positions[1] - 1];
        assert_eq!(diff.lines()[new_line_row.right.unwrap() as usize].new_line, 2);
    }

    #[test]
    fn builds_anchors_by_line_kind() {
        let diff = sample();
        let rows = rows_of(&diff, Layout::Inline, &[], &[], &[]).rows;
        let added = anchor_for(&diff, &rows[3], true, "a.rs", None).unwrap();
        assert_eq!((added.old_line, added.new_line), (None, Some(2)));
        let context = anchor_for(&diff, &rows[0], true, "a.rs", None).unwrap();
        assert_eq!((context.old_line, context.new_line), (Some(1), Some(1)));
        let expanded = anchor_for(&diff, &Row::expanded(7, 9), true, "a.rs", None).unwrap();
        assert_eq!((expanded.old_line, expanded.new_line), (Some(7), Some(9)));
        assert!(anchor_for(&diff, &Row::marker(RowKind::Gap(0)), true, "a.rs", None).is_none());
    }

    #[test]
    fn whole_files_show_their_one_version() {
        let added = FileDiff::parse_unified(b"@@ -0,0 +1,2 @@\n+a\n+b\n");
        let rows = rows_of(&added, Layout::WholeFile(DiffSide::New), &[], &[], &[]).rows;
        assert_eq!(rows, [Row::line(None, Some(1)), Row::line(None, Some(2))]);
        let deleted = FileDiff::parse_unified(b"@@ -1,2 +0,0 @@\n-a\n-b\n");
        let rows = rows_of(&deleted, Layout::WholeFile(DiffSide::Old), &[], &[], &[]).rows;
        assert_eq!(rows, [Row::line(Some(1), None), Row::line(Some(2), None)]);
        let anchor = anchor_for(&deleted, &rows[1], true, "a.rs", None).unwrap();
        assert_eq!(
            (anchor.old_line, anchor.new_line),
            (Some(2), None),
            "deleted files comment on the base side"
        );
    }

    #[test]
    fn counts_each_line_in_both_files() {
        // keep(1/1) -old a(2) -old b(3) +new a(/2) tail(4/3) +added end(/4)
        let diff = sample();
        let rows = rows_of(&diff, Layout::Inline, &[], &[], &[]).rows;
        let positions: Vec<(u32, u32)> = rows
            .iter()
            .map(|row| {
                file_positions(&diff, row, true)
                    .map(|positions| (positions.old, positions.new))
                    .unwrap()
            })
            .collect();
        assert_eq!(positions, [(1, 1), (2, 2), (3, 2), (4, 2), (4, 3), (5, 4)]);
        let added = FileDiff::parse_unified(b"@@ -0,0 +1,1 @@\n+a\n");
        let row = Row::line(None, Some(1));
        assert_eq!(file_positions(&added, &row, true), Some(FilePositions { old: 0, new: 1 }));
    }

    #[test]
    fn builds_range_anchors_and_marks_their_rows() {
        let diff = sample();
        let rows = rows_of(&diff, Layout::Inline, &[], &[], &[]).rows;
        let pick = RangePick {
            start_row: 0,
            end_row: 3,
            prefer_right: true,
            side: DiffSide::New,
            offsets: (1, 7),
        };
        let anchor = range_anchor(&diff, &rows, &pick, "a.rs", None).unwrap();
        assert_eq!(
            (anchor.old_line, anchor.new_line),
            (None, Some(2)),
            "the last line is the anchor's line"
        );
        let range = anchor.range.clone().unwrap();
        assert_eq!(
            (range.start_old_line, range.start_new_line, range.side),
            (Some(1), Some(1), DiffSide::New)
        );
        assert_eq!(
            range.detail,
            Some(RangeDetail {
                start: FilePositions { old: 1, new: 1 },
                end: FilePositions { old: 4, new: 2 },
                start_offset: 1,
                end_offset: 7
            })
        );
        let single = RangePick { start_row: 3, ..pick };
        assert!(
            range_anchor(&diff, &rows, &single, "a.rs", None).unwrap().range.is_none(),
            "one line is a plain anchor"
        );

        let mut on_range = on_new(2);
        on_range.anchor = Some(anchor);
        let threads = [&on_range];
        let placed = rows_of(&diff, Layout::Inline, &[], &[], &threads);
        assert_eq!(placed.rows[4].kind, RowKind::Thread(0), "the card follows the last line");
        assert_eq!(
            placed.ranges,
            [RangeRows { start: 0, end: 3, side: DiffSide::New, is_draft: false }]
        );
    }

    #[test]
    fn places_the_draft_above_its_first_line_when_picked_dragging_up() {
        let diff = sample();
        // Lines "-old a" to "+new a", with a thread on `keep` right before them.
        let range = LineRange {
            start_old_line: Some(2),
            start_new_line: None,
            side: DiffSide::New,
            detail: None,
        };
        let mut composer =
            ComposerAnchor { old_line: None, new_line: Some(2), range: Some(range), above: false };
        let on_keep = on_both(1);
        let threads = [&on_keep];
        let rows = |composer: &ComposerAnchor| {
            build(&RowInputs {
                diff: &diff,
                layout: Layout::Inline,
                gaps: &[],
                reveals: &[],
                threads: &threads,
                composer: Some(composer),
            })
        };
        let below = rows(&composer);
        assert_eq!(below.rows[5].kind, RowKind::Composer, "below the last line");
        assert_eq!(
            below.ranges.last(),
            Some(&RangeRows { start: 2, end: 4, side: DiffSide::New, is_draft: true })
        );

        composer.above = true;
        let above = rows(&composer);
        let kinds: Vec<RowKind> = above.rows.iter().take(4).map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [RowKind::Line, RowKind::Thread(0), RowKind::Composer, RowKind::Line],
            "right above the first line, after the card of the line before"
        );
        assert_eq!(
            above.ranges.last(),
            Some(&RangeRows { start: 3, end: 5, side: DiffSide::New, is_draft: true }),
            "the lines below the card stay marked"
        );
    }

    #[test]
    fn marks_the_line_of_an_active_thread_but_not_of_a_resolved_one() {
        let diff = sample();
        let on_line = on_new(2);
        let threads = [&on_line];
        let placed = rows_of(&diff, Layout::Inline, &[], &[], &threads);
        assert_eq!(
            placed.ranges,
            [RangeRows { start: 3, end: 3, side: DiffSide::New, is_draft: false }]
        );
        let mut resolved = on_new(2);
        resolved.status = review_core::ThreadStatus::Resolved;
        let threads = [&resolved];
        assert_eq!(rows_of(&diff, Layout::Inline, &[], &[], &threads).ranges, []);
    }

    #[test]
    fn tells_when_rows_cross_a_hunk() {
        let diff = two_hunks();
        let rows = rows_of(&diff, Layout::Inline, &[], &[], &[]).rows;
        assert!(!spans_hunks(&diff, &rows, 0, 2));
        assert!(spans_hunks(&diff, &rows, 2, 3), "the second hunk starts between them");
        let gaps = gaps::find(&diff, Some(70));
        let with_gaps = rows_of(&diff, Layout::Inline, &gaps, &[], &[]).rows;
        assert!(spans_hunks(&diff, &with_gaps, 1, 5), "a collapsed gap lies between");
    }

    fn on_new(line: u32) -> ReviewThread {
        anchored(None, Some(line))
    }

    fn on_old(line: u32) -> ReviewThread {
        anchored(Some(line), None)
    }

    fn on_both(line: u32) -> ReviewThread {
        anchored(Some(line), Some(line))
    }

    fn anchored(old_line: Option<u32>, new_line: Option<u32>) -> ReviewThread {
        ReviewThread {
            id: "t".into(),
            status: review_core::ThreadStatus::Active,
            anchor: Some(ThreadAnchor {
                path: "a.rs".into(),
                old_path: None,
                old_line,
                new_line,
                range: None,
            }),
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: true,
            can_resolve: true,
            comments: Vec::new(),
        }
    }
}
