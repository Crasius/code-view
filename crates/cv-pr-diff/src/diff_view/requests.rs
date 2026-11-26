//! Requests from outside the text diff, carried out once the rows hold what they need: lines the
//! rendered Markdown diff hands over, the assistant cited or the find bar goes to
//! ([`LineRequest`]) and a thread to
//! scroll to. Lines in a collapsed gap are shown first, which rebuilds the rows, and the request
//! waits for that. Cited lines and the lines of a thread scrolled to flash once on screen.

use std::ops::RangeInclusive;

use review_core::{DiffSide, ReviewThread};

use super::DiffInputs;
use super::code_rows::flash::{Flash, Target};
use super::navigation;
use super::offsets::RowOffsets;
use super::rows::DiffRows;
use crate::line_anchor::LineAnchor;
use crate::selection::Selection;
use cv_code::diff_rows::{self, RowKind};
use cv_code::gaps::Expand;
use cv_code::line_source::Pane;

/// Lines other views hand to the text diff, carried out once the rows are built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineRequest {
    /// Put this line at the top of the view: switching from the rendered view keeps the place.
    Top { side: DiffSide, line: u32 },
    /// Select these lines (1-based, end exclusive) and open a new comment on them.
    Comment { side: DiffSide, lines: std::ops::Range<u32> },
    /// Show these lines (1-based, inclusive; the assistant cited them), scroll to and flash them.
    Cite { side: DiffSide, lines: std::ops::RangeInclusive<u32> },
    /// Show this line (the find bar's current hit) and scroll it to the middle of the view.
    Find { side: DiffSide, line: u32 },
    /// Go to the first change (`Next`) or the last one (`Previous`): N or P went on from the file
    /// before.
    Change(navigation::Direction),
}

/// What other views asked of the text diff, kept until the rows hold what it needs.
#[derive(Default)]
pub(super) struct Requests {
    /// A thread to scroll to once its row exists.
    thread: Option<String>,
    /// What the rendered Markdown diff or the assistant asked for.
    lines: Option<LineRequest>,
    /// The flash of the lines of a request carried out, which the code rows paint.
    pub flash: Flash,
}

impl LineRequest {
    /// Head lines `lines` the assistant cited: shown, scrolled to and flashed.
    pub fn head(lines: RangeInclusive<u32>) -> Self {
        Self::Cite { side: DiffSide::New, lines }
    }
}

impl Requests {
    /// Scrolls to thread `thread_id` once its row exists.
    pub fn reveal_thread(&mut self, thread_id: String) {
        self.thread = Some(thread_id);
    }

    /// Carries out `request` once the rows hold its lines.
    pub fn request_lines(&mut self, request: LineRequest) {
        self.lines = Some(request);
    }

    /// A thread waits to be scrolled to.
    pub fn is_revealing(&self) -> bool {
        self.thread.is_some()
    }

    /// Carries out the line request on the rows `built`, placed at `offsets`, in a view
    /// `view_height` tall (see [`carry_out`]); `None` without one. A request on lines in a
    /// collapsed gap waits until they show.
    pub fn carry_out_lines(
        &mut self,
        built: &mut DiffRows,
        offsets: &RowOffsets,
        inputs: &DiffInputs<'_>,
        view_height: f32,
    ) -> Option<LineOutcome> {
        let request = self.lines.take()?;
        let mut outcome = carry_out(&request, built, offsets, inputs, view_height);
        if outcome.is_waiting {
            self.lines = Some(request);
        }
        if let Some(cited) = &outcome.flash {
            self.flash.cite(cited);
        }
        self.flash.start(outcome.flash.take());
        Some(outcome)
    }

    /// Where to scroll for the thread waiting since [`Self::reveal_thread`], once its card's row
    /// is in `built`; a thread on collapsed lines first reveals them, one that will not show stops
    /// waiting.
    pub fn reveal_pending(
        &mut self,
        built: &mut DiffRows,
        offsets: &RowOffsets,
        inputs: &DiffInputs<'_>,
        view_height: f32,
    ) -> Option<f32> {
        let thread_id = self.thread.as_deref()?;
        match reveal_thread(thread_id, built, offsets, inputs, view_height) {
            ThreadReveal::Scroll(offset, flash) => {
                self.thread = None;
                self.flash.start(flash);
                Some(offset)
            }
            ThreadReveal::Drop => {
                self.thread = None;
                None
            }
            ThreadReveal::Keep => None,
        }
    }
}

/// What carrying out a line request gave.
#[derive(Default)]
pub(super) struct LineOutcome {
    pub scroll_to: Option<f32>,
    /// Lines to select and open a new comment on.
    pub select: Option<Selection>,
    /// The lines are in a collapsed gap: the request waits until they show.
    pub is_waiting: bool,
    /// Lines to flash once they are on screen.
    pub flash: Option<Target>,
    /// The change now in focus.
    pub change: Option<usize>,
}

/// Carries out `request` on the rows `built`, placed at `offsets`, in a view `view_height` tall: a
/// line goes to the top of the view (a collapsed gap holding it is enough); lines to comment on
/// or cited are shown (their gaps expanded first) and scrolled to, then selected or flashed.
fn carry_out(
    request: &LineRequest,
    built: &mut DiffRows,
    offsets: &RowOffsets,
    inputs: &DiffInputs<'_>,
    view_height: f32,
) -> LineOutcome {
    let numbers = |side: DiffSide, line: u32| match side {
        DiffSide::Old => (Some(line), None),
        DiffSide::New => (None, Some(line)),
    };
    let diff = inputs.diff.as_ref();
    let (side, first, last) = match request {
        LineRequest::Top { side, line } => {
            let (old_line, new_line) = numbers(*side, *line);
            let row = built.row_of_anchor(&LineAnchor::of_numbers(old_line, new_line), diff);
            let scroll_to = row.and_then(|row| offsets.get(row));
            return LineOutcome { scroll_to, ..LineOutcome::default() };
        }
        LineRequest::Comment { side, lines } => {
            (*side, lines.start, lines.end.saturating_sub(1).max(lines.start))
        }
        LineRequest::Cite { side, lines } => (*side, *lines.start(), *lines.end()),
        LineRequest::Find { side, line } => (*side, *line, *line),
        LineRequest::Change(direction) => {
            return change_at_end(*direction, built, offsets, view_height);
        }
    };
    let find = |line: u32| {
        let (old_line, new_line) = numbers(side, line);
        diff_rows::find_row(diff, &built.rows, old_line, new_line)
    };
    let Some((first_row, last_row)) = find(first).zip(find(last)) else {
        // Lines in a collapsed gap show once the file's versions are loaded; lines after the last
        // hunk get their gap only with them.
        let hidden = [first, last].into_iter().find_map(|line| {
            let (old_line, new_line) = numbers(side, line);
            built.gap_of(old_line, new_line)
        });
        if let Some(gap) = hidden
            && inputs.can_expand()
        {
            built.expand_gap(gap, Expand::All);
        }
        let is_settled =
            inputs.contents.is_some_and(|contents| contents.error.is_some() || inputs.can_expand());
        let is_waiting = hidden.is_some() || !is_settled;
        return LineOutcome { is_waiting, ..LineOutcome::default() };
    };
    let pane = match (inputs.layout.is_single_pane(), side) {
        (true, _) => Pane::Inline,
        (false, DiffSide::Old) => Pane::Left,
        (false, DiffSide::New) => Pane::Right,
    };
    let total = offsets.height();
    let scroll_to = match request {
        LineRequest::Find { .. } => offsets
            .get(first_row)
            .zip(offsets.get(first_row + 1))
            .map(|(top, bottom)| navigation::center_offset(top..bottom, view_height, total)),
        _ => offsets.get(first_row).map(|top| navigation::focus_offset(top, view_height, total)),
    };
    let (select, flash) = match request {
        LineRequest::Cite { .. } => (None, Some(Target { side, lines: first..=last })),
        LineRequest::Find { .. } => (None, None),
        _ => (Some(Selection::lines(pane, first_row, last_row.max(first_row))), None),
    };
    LineOutcome { scroll_to, select, is_waiting: false, flash, change: None }
}

/// The first change (`Next`) or the last one (`Previous`) of the rows `built`, placed at
/// `offsets`, in focus and scrolled to as N and P do.
fn change_at_end(
    direction: navigation::Direction,
    built: &DiffRows,
    offsets: &RowOffsets,
    view_height: f32,
) -> LineOutcome {
    let index = match direction {
        navigation::Direction::Next => 0,
        navigation::Direction::Previous => built.changes.len().saturating_sub(1),
    };
    let Some(top) = built.changes.get(index).and_then(|row| offsets.get(*row as usize)) else {
        return LineOutcome::default();
    };
    let scroll_to = Some(navigation::focus_offset(top, view_height, offsets.height()));
    LineOutcome { scroll_to, change: Some(index), ..LineOutcome::default() }
}

/// Where looking for a thread to scroll to got.
enum ThreadReveal {
    /// Its card's row is there: scroll the view's top to this, and flash its lines.
    Scroll(f32, Option<Target>),
    /// It will not show in the rows (outdated, or not on any line the file has).
    Drop,
    /// Not yet: its collapsed lines are being shown, or the file's contents may still bring them.
    Keep,
}

/// Looks for the card of thread `thread_id` in the rows `built`, placed at `offsets`, in a view
/// `view_height` tall; a thread on collapsed lines first reveals them.
fn reveal_thread(
    thread_id: &str,
    built: &mut DiffRows,
    offsets: &RowOffsets,
    inputs: &DiffInputs<'_>,
    view_height: f32,
) -> ThreadReveal {
    let threads = inputs.threads;
    let row = built.rows.iter().position(
        |row| matches!(row.kind, RowKind::Thread(index) if threads[index as usize].id == thread_id),
    );
    if let Some(row) = row {
        let flash = threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .and_then(|thread| thread_lines(thread));
        return ThreadReveal::Scroll((offsets[row] - view_height / 3.0).max(0.0), flash);
    }
    let Some(thread) =
        built.detached.iter().map(|index| threads[*index]).find(|thread| thread.id == thread_id)
    else {
        return ThreadReveal::Keep;
    };
    let anchor = thread.anchor.as_ref().filter(|_| !thread.is_outdated && inputs.can_expand());
    let gap = anchor.and_then(|anchor| {
        built.gaps.iter().zip(&built.reveals).position(|(gap, reveal)| {
            reveal.hidden(gap) > 0 && gap.contains(anchor.old_line, anchor.new_line)
        })
    });
    if let Some(gap) = gap {
        built.expand_gap(gap, Expand::All);
        return ThreadReveal::Keep;
    }
    // Contents may still arrive (and with them the gap after the last hunk); outdated threads
    // stay detached.
    let is_settled =
        inputs.contents.is_some_and(|contents| contents.error.is_some() || inputs.can_expand());
    if thread.is_outdated || is_settled { ThreadReveal::Drop } else { ThreadReveal::Keep }
}

/// The lines `thread` is written on, in the side its (last) line counts in.
fn thread_lines(thread: &ReviewThread) -> Option<Target> {
    let anchor = thread.anchor.as_ref()?;
    let side = if anchor.new_line.is_some() { DiffSide::New } else { DiffSide::Old };
    let last = anchor.line_in(side)?;
    let first = anchor.range.as_ref().and_then(|range| range.start_line_in(side)).unwrap_or(last);
    Some(Target { side, lines: first.min(last)..=last })
}
