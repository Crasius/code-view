//! The rows of a layout built from the lined-up pairs: shown and folded runs, a card row under
//! the block of each thread, the space above each row, the changes to step through and the
//! overview strip's marks. Every frame starts with the rows for its model and mode
//! ([`begin_frame`]).

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use review_core::{DiffSide, ReviewThread};
use review_git::FULL_FILE_CONTEXT;

use super::align::{Change, Pair};
use super::model::{Cell, Look, MarkdownModel, Mode, Row, RowsKey, slot};
use super::threads::{self, Placement, ThreadRow};
use super::{MarkdownDiff, ROW_PAD, RenderedInputs};
use cv_base::theme::style;
use cv_markdown::Units;
use cv_pr_diff::diff_overview::{ChangeBlock, Overview};

/// Between a fold bar and the blocks around it (at zoom 1).
const FOLD_SPACE: f32 = 8.0;
/// Runs of fewer unchanged blocks than this are shown rather than folded.
const MIN_FOLDED: usize = 2;

/// The rows of `mode` from the pairs, with what goes with them.
pub(super) struct Built {
    pub rows: Vec<Row>,
    pub spaces: Vec<f32>,
    pub hunks: Vec<Range<usize>>,
    pub overview: Overview,
    pub old_rows: Vec<Option<usize>>,
    pub new_rows: Vec<Option<usize>>,
    /// Threads in folded blocks: (the fold's first pair, the thread's index).
    pub folded_threads: Vec<(usize, usize)>,
}

pub(super) enum Segment {
    Shown(usize),
    Folded(Range<usize>),
}

/// The start of every frame of `diff`, in the file view and embedded: the loaded model, its mode,
/// the rows for it, and whether the banner over a diff without visible changes shows.
pub(super) fn begin_frame(
    diff: &mut MarkdownDiff,
    ctx: &egui::Context,
    inputs: &RenderedInputs<'_>,
) -> (Arc<MarkdownModel>, Mode, bool) {
    let model = Arc::clone(inputs.model);
    diff.ensure_model(&model);
    let mode = Mode::of(model.has_old, model.has_new, inputs.layout);
    diff.follow_thread_visibility(inputs.show_threads);
    let key = RowsKey {
        mode,
        context: inputs.context,
        expanded: diff.expanded_version,
        threads: threads::threads_key(inputs.threads),
    };
    diff.ensure_rows(&model, key, inputs.threads);
    diff.cards.begin(ctx);
    let has_banner = model.has_old && model.has_new && diff.hunks.is_empty();
    (model, mode, has_banner)
}

pub(super) fn build(
    model: &MarkdownModel,
    mode: Mode,
    context: u32,
    expanded: &HashSet<usize>,
    threads: &[&ReviewThread],
) -> Built {
    let folds = !matches!(mode, Mode::Whole(_));
    let mut rows = Vec::with_capacity(model.pairs.len() + threads.len());
    for segment in segments(&model.pairs, context, folds, expanded) {
        match segment {
            Segment::Shown(index) => push_pair(&mut rows, index, &model.pairs[index], mode),
            Segment::Folded(pairs) => rows.push(Row::Folded { pairs }),
        }
    }
    let folded_threads = place_threads(&mut rows, model, threads);
    let spaces = spaces(&rows, model, mode == Mode::Columns);
    let hunks = match mode {
        Mode::Whole(_) if !rows.is_empty() => std::iter::once(0..rows.len()).collect(),
        _ => hunks(&rows),
    };
    let overview = overview(&rows, &hunks, mode, &folded_threads);
    let (old_rows, new_rows) = unit_rows(&rows, model);
    Built { rows, spaces, hunks, overview, old_rows, new_rows, folded_threads }
}

/// The row showing each unit of the old and of the new text.
pub(super) fn unit_rows(
    rows: &[Row],
    model: &MarkdownModel,
) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let (mut old_rows, mut new_rows) = (vec![None; model.old.len()], vec![None; model.new.len()]);
    for (index, row) in rows.iter().enumerate() {
        for cell in row.cells().into_iter().flatten() {
            match cell.side {
                DiffSide::Old => old_rows[cell.unit] = Some(index),
                DiffSide::New => new_rows[cell.unit] = Some(index),
            }
        }
    }
    (old_rows, new_rows)
}

/// Puts a card row for each thread under the block row it belongs to (old lane first, then by
/// line) and returns the threads in folds: (the fold's first pair, the thread's index).
pub(super) fn place_threads(
    rows: &mut Vec<Row>,
    model: &MarkdownModel,
    threads: &[&ReviewThread],
) -> Vec<(usize, usize)> {
    if threads.is_empty() {
        return Vec::new();
    }
    let (old_rows, new_rows) = unit_rows(rows, model);
    let mut pair_rows = vec![usize::MAX; model.pairs.len()];
    for (index, row) in rows.iter().enumerate().rev() {
        for pair in row.pairs() {
            pair_rows[pair] = index;
        }
    }
    let mut under: Vec<(usize, DiffSide, u32, ThreadRow)> = Vec::with_capacity(threads.len());
    let mut folded = Vec::new();
    for (index, thread) in threads.iter().enumerate() {
        match threads::place(
            model,
            rows,
            [old_rows.as_slice(), new_rows.as_slice()],
            &pair_rows,
            index,
            thread,
        ) {
            Some(Placement::Under { row, thread, line }) => {
                under.push((row, thread.lane, line, thread));
            }
            Some(Placement::Folded { row }) => {
                if let Row::Folded { pairs } = &rows[row] {
                    folded.push((pairs.start, index));
                }
            }
            None => {}
        }
    }
    if under.is_empty() {
        return folded;
    }
    under.sort_by_key(|(row, lane, line, thread)| {
        (*row, *lane == DiffSide::New, *line, thread.thread)
    });
    let mut merged = Vec::with_capacity(rows.len() + under.len());
    let mut next = under.into_iter().peekable();
    for (index, row) in rows.drain(..).enumerate() {
        merged.push(row);
        while let Some((_, _, _, thread)) = next.next_if(|(target, ..)| *target == index) {
            merged.push(Row::Thread(thread));
        }
    }
    *rows = merged;
    folded
}

/// Which pairs show and which fold: runs of unchanged pairs keep `context` pairs next to a change
/// and fold the rest, unless it is fewer than [`MIN_FOLDED`] or the user opened that fold.
pub(super) fn segments(
    pairs: &[Pair],
    context: u32,
    folds: bool,
    expanded: &HashSet<usize>,
) -> Vec<Segment> {
    let has_changes = pairs.iter().any(|pair| pair.change.is_change());
    if !folds || context >= FULL_FILE_CONTEXT || !has_changes {
        return (0..pairs.len()).map(Segment::Shown).collect();
    }
    let context = context as usize;
    let mut segments = Vec::with_capacity(pairs.len());
    let mut index = 0;
    while index < pairs.len() {
        if pairs[index].change.is_change() {
            segments.push(Segment::Shown(index));
            index += 1;
            continue;
        }
        let start = index;
        while index < pairs.len() && !pairs[index].change.is_change() {
            index += 1;
        }
        let end = index;
        let hidden_start = if start == 0 { start } else { start + context };
        let hidden_end = if end == pairs.len() { end } else { end.saturating_sub(context) };
        if hidden_end >= hidden_start + MIN_FOLDED && !expanded.contains(&hidden_start) {
            segments.extend((start..hidden_start).map(Segment::Shown));
            segments.push(Segment::Folded(hidden_start..hidden_end));
            segments.extend((hidden_end..end).map(Segment::Shown));
        } else {
            segments.extend((start..end).map(Segment::Shown));
        }
    }
    segments
}

pub(super) fn push_pair(rows: &mut Vec<Row>, index: usize, pair: &Pair, mode: Mode) {
    let cell = |side: DiffSide, unit: usize, look: Look| Cell { side, unit, pair: index, look };
    let one = |cell: Cell| Row::Blocks { left: Some(cell), right: None };
    let is_equal = pair.change == Change::Equal;
    match mode {
        Mode::Columns => {
            let (old_look, new_look) =
                if is_equal { (Look::Plain, Look::Plain) } else { (Look::Removed, Look::Added) };
            rows.push(Row::Blocks {
                left: pair.old.map(|unit| cell(DiffSide::Old, unit, old_look)),
                right: pair.new.map(|unit| cell(DiffSide::New, unit, new_look)),
            });
        }
        Mode::Unified => match (is_equal, pair.old, pair.new) {
            (true, _, Some(new)) => rows.push(one(cell(DiffSide::New, new, Look::Plain))),
            (true, Some(old), None) => rows.push(one(cell(DiffSide::Old, old, Look::Plain))),
            (true, None, None) => {}
            (false, old, new) => {
                rows.extend(old.map(|unit| one(cell(DiffSide::Old, unit, Look::Removed))));
                rows.extend(new.map(|unit| one(cell(DiffSide::New, unit, Look::Added))));
            }
        },
        Mode::NewOnly => match pair.new {
            Some(new) => rows.push(one(cell(
                DiffSide::New,
                new,
                if is_equal { Look::Plain } else { Look::Focus },
            ))),
            None => match rows.last_mut() {
                Some(Row::Removal { pairs }) => pairs.end = index + 1,
                _ => rows.push(Row::Removal { pairs: index..index + 1 }),
            },
        },
        Mode::Whole(side) => {
            let unit = match side {
                DiffSide::Old => pair.old,
                DiffSide::New => pair.new,
            };
            rows.extend(unit.map(|unit| one(cell(side, unit, Look::Plain))));
        }
    }
}

/// The space above each row at zoom 1: the document's own spacing between two blocks (measured
/// between the blocks of the same side, side by side), less the room inside the tints.
pub(super) fn spaces(rows: &[Row], model: &MarkdownModel, columns: bool) -> Vec<f32> {
    let mut spaces = Vec::with_capacity(rows.len());
    // The last block of each side, and the last block of any side (one column).
    let mut last: [Option<usize>; 2] = [None, None];
    let mut previous: Option<Cell> = None;
    let mut after_fold = false;
    for (index, row) in rows.iter().enumerate() {
        let space = match row {
            // A card's band keeps its own room; the block after it is spaced from the block above.
            Row::Removal { .. } | Row::Thread(_) => 0.0,
            _ if index == 0 => 0.0,
            Row::Folded { .. } => FOLD_SPACE,
            Row::Blocks { .. } if after_fold => FOLD_SPACE,
            Row::Blocks { .. } => {
                let gap = row
                    .cells()
                    .into_iter()
                    .flatten()
                    .filter_map(|cell| {
                        let (side, unit) = if columns {
                            (cell.side, last[slot(cell.side)]?)
                        } else {
                            previous.map(|previous| (previous.side, previous.unit))?
                        };
                        Some(Units::gap(
                            (model.units(side), unit),
                            (model.units(cell.side), cell.unit),
                        ))
                    })
                    .fold(None, |widest: Option<f32>, gap| {
                        Some(widest.map_or(gap, |widest| widest.max(gap)))
                    });
                (gap.unwrap_or(style::SPACE_M) - 2.0 * ROW_PAD).max(0.0)
            }
        };
        spaces.push(space);
        match row {
            Row::Folded { .. } => {
                last = [None, None];
                previous = None;
                after_fold = true;
            }
            Row::Blocks { .. } => {
                for cell in row.cells().into_iter().flatten() {
                    last[slot(cell.side)] = Some(cell.unit);
                    previous = Some(cell);
                }
                after_fold = false;
            }
            Row::Removal { .. } | Row::Thread(_) => {}
        }
    }
    spaces
}

/// Consecutive changed rows, each one change to step to; cards between them do not split a change.
pub(super) fn hunks(rows: &[Row]) -> Vec<Range<usize>> {
    let mut hunks = Vec::new();
    let mut start = None;
    for (index, row) in rows.iter().enumerate() {
        if matches!(row, Row::Thread(_)) {
            continue;
        }
        if row.is_change() {
            start.get_or_insert(index);
        } else if let Some(start) = start.take() {
            hunks.push(start..index);
        }
    }
    hunks.extend(start.map(|start| start..rows.len()));
    hunks
}

pub(super) fn overview(
    rows: &[Row],
    hunks: &[Range<usize>],
    mode: Mode,
    folded_threads: &[(usize, usize)],
) -> Overview {
    let mut overview = Overview { is_focus: mode == Mode::NewOnly, ..Overview::default() };
    // The comments, on their cards and on the folds hiding them.
    overview.threads = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| match row {
            Row::Thread(_) => true,
            Row::Folded { pairs } => folded_threads.iter().any(|(start, _)| *start == pairs.start),
            Row::Blocks { .. } | Row::Removal { .. } => false,
        })
        .map(|(index, _)| index as u32)
        .collect();
    // A whole added or deleted file is one change; the strip then only shows the comments.
    if matches!(mode, Mode::Whole(_)) {
        return overview;
    }
    for hunk in hunks {
        let looks = || {
            rows[hunk.clone()]
                .iter()
                .flat_map(|row| row.cells().into_iter().flatten())
                .map(|cell| cell.look)
        };
        let has_added = looks().any(|look| matches!(look, Look::Added | Look::Focus));
        let has_deleted = looks().any(|look| look == Look::Removed);
        if has_added || has_deleted {
            overview.blocks.push(ChangeBlock {
                start: hunk.start as u32,
                end: hunk.end as u32,
                has_added,
                has_deleted,
            });
        }
    }
    overview.removals = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, Row::Removal { .. }))
        .map(|(index, _)| index as u32)
        .collect();
    overview
}
