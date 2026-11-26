//! Where the text diff's view is on its rows and where it goes next: the part in view, the scroll
//! it asked for, a row to put back once the offsets are known, and the change the user stepped
//! to. New heights, a rebuild of the rows and comments switched on or off keep the line the user
//! reads where it is on screen. Rows shown inside another view's scrolling (a card of the folder
//! summary) leave clamping a scroll target to that view.

use review_core::DiffSide;

use super::frame::{Sight, Viewport};
use super::navigation::{self, Direction};
use super::offsets::{HeightKey, RowOffsets};
use super::rows::DiffRows;
use crate::line_anchor::LineAnchor;
use cv_code::diff_rows::{Row, RowKind};

/// A row kept in place across new heights: the row and how far into it, and how far below the
/// top of the view it goes.
pub(super) type KeptRow = ((usize, f32), f32);

/// The (base, head) line numbers of a row.
type Lines = (Option<u32>, Option<u32>);

/// Where the view is on its rows and where it goes next.
#[derive(Default)]
pub(super) struct Place {
    /// Viewport of the last frame.
    pub viewport: Viewport,
    /// Where the view scrolls next: the y (from the top of the first row) its top goes to.
    pub scroll_to: Option<f32>,
    /// Shown inside another view's scrolling: scroll targets are that view's to clamp.
    pub is_embedded: bool,
    /// The change the user last jumped to, while it is on screen.
    pub current_change: Option<usize>,
    /// A step found no change further that way (at the last or the first one): until taken.
    pub ran_out: Option<Direction>,
    /// Row to put back at this distance below the top of the view once offsets are known.
    restore_row: Option<(usize, f32)>,
}

impl Place {
    /// The place of a file just opened: the top of its rows, until a jump (a thread, a find hit,
    /// a line of the rendered diff) goes elsewhere. The scroll area keeps its offset in egui's
    /// memory under one id for every file, so without asking for the top a file opened where the
    /// one before was scrolled to.
    pub fn opening() -> Self {
        Self { scroll_to: Some(0.0), ..Self::default() }
    }

    /// The top of the view, from the top of the first row.
    pub fn top(&self) -> f32 {
        self.viewport.0.top()
    }

    /// How far below the top of the view row `row` (placed at `offsets`) starts, within the view.
    pub fn below_top(&self, offsets: &RowOffsets, row: usize) -> f32 {
        let view = self.viewport.0;
        offsets.get(row).map_or(0.0, |top| (top - view.top()).clamp(0.0, view.height().max(0.0)))
    }

    /// Steps from the change in focus to the next or previous one (`changes` are their rows) and
    /// scrolls to it; with none further that way, notes that it ran out.
    pub fn go_to_change(&mut self, direction: Direction, changes: &[u32], offsets: &RowOffsets) {
        let top_of = |index: usize| offsets.get(changes[index] as usize).unwrap_or(0.0);
        let view = self.viewport.0;
        let Some(index) =
            navigation::step(direction, self.current_change, changes.len(), view.top(), top_of)
        else {
            self.ran_out = Some(direction);
            return;
        };
        self.current_change = Some(index);
        let total = offsets.height();
        self.scroll_to = Some(navigation::focus_offset(top_of(index), view.height(), total));
    }

    /// The change in focus left the view: no change is in focus any more.
    pub fn forget_change_out_of_view(&mut self, changes: &[u32], offsets: &RowOffsets) {
        let Some(current) = self.current_change else {
            return;
        };
        let view = self.viewport.0;
        let offset = changes.get(current).and_then(|row| offsets.get(*row as usize));
        let is_visible =
            offset.is_some_and(|offset| offset >= view.top() - 1.0 && offset <= view.bottom());
        if !is_visible && self.scroll_to.is_none() {
            self.current_change = None;
        }
    }

    /// The lines of the change in focus in `built`, for it to stay in focus when the file's
    /// contents, a thread or revealed lines rebuild the rows; a new diff starts over.
    pub fn focused_lines(&self, built: &DiffRows, is_new_diff: bool) -> Option<Lines> {
        self.current_change
            .filter(|_| !is_new_diff)
            .and_then(|index| built.changes.get(index))
            .and_then(|row| built.rows.get(*row as usize))
            .map(|row| (row.left, row.right))
    }

    /// After a rebuild of `built`: the change showing `lines` (see [`Self::focused_lines`]) is the
    /// one in focus.
    pub fn refocus(&mut self, built: &DiffRows, lines: Option<Lines>) {
        self.current_change = lines.and_then(|lines| {
            built.changes.iter().position(|row| {
                built.rows.get(*row as usize).is_some_and(|row| (row.left, row.right) == lines)
            })
        });
    }

    /// The first code line of `built` at the top of the view (or the nearest above) and how far
    /// below the view's top edge it starts; `None` at the very top, where staying at the top is
    /// right.
    pub fn at_top(&self, built: &DiffRows, offsets: &RowOffsets) -> Option<(LineAnchor, f32)> {
        let diff = built.shown_diff.as_ref()?;
        let rows = &built.rows;
        let top = self.top();
        // Rows inside another view's scrolling that end above its view hold no line at its top.
        if top <= 0.0 || top >= offsets.height() || !offsets.is_placed(rows.len()) {
            return None;
        }
        let first = offsets.row_at(top);
        let is_code = |row: &Row| matches!(row.kind, RowKind::Line | RowKind::Expanded);
        let row_index =
            (first..rows.len()).chain((0..first).rev()).find(|index| is_code(&rows[*index]))?;
        let row = &rows[row_index];
        let anchor = match row.kind {
            RowKind::Line => LineAnchor::of_line(diff, row.right.or(row.left)? as usize)?,
            _ => LineAnchor::of_numbers(row.left, row.right),
        };
        Some((anchor, offsets[row_index] - top))
    }

    /// The first code line of `built` at the top of the view and its side; `None` at the very top.
    pub fn top_line(&self, built: &DiffRows, offsets: &RowOffsets) -> Option<(DiffSide, u32)> {
        let (anchor, _) = self.at_top(built, offsets)?;
        match (anchor.new_line, anchor.old_line) {
            (Some(line), _) => Some((DiffSide::New, line)),
            (None, Some(line)) => Some((DiffSide::Old, line)),
            (None, None) => None,
        }
    }

    /// Comments were switched on or off: the first row at the top of the view that is no comment
    /// card stays where it is on screen while the cards above it appear or go.
    pub fn keep_first_line(&mut self, rows: &[Row], offsets: &RowOffsets) {
        if self.is_embedded || self.scroll_to.is_some() || !offsets.is_placed(rows.len()) {
            return;
        }
        let top = self.top();
        let first = offsets.row_at(top);
        let is_card = |row: &Row| matches!(row.kind, RowKind::Thread(_) | RowKind::Composer);
        if let Some(row) = (first..rows.len()).find(|index| !is_card(&rows[*index])) {
            self.restore_row = Some((row, offsets[row] - top));
        }
    }

    /// Row `row` goes back to `below_top` below the top of the view once the offsets are known.
    pub fn keep_row(&mut self, row: usize, below_top: f32) {
        self.restore_row = Some((row, below_top));
    }

    /// New heights (a pane resized, focus mode folding the file tree away) keep the line at the
    /// top of the view in place, a new code size (`key` after `previous`) the one under the
    /// pointer (see [`super::keep_below`]). Rows inside another view's scrolling keep it only when
    /// it is theirs. `rows` rows are placed at `offsets`.
    pub fn for_new_heights(
        &self,
        previous: HeightKey,
        key: HeightKey,
        sight: Sight,
        offsets: &RowOffsets,
        rows: usize,
    ) -> Option<KeptRow> {
        #[expect(
            clippy::float_cmp,
            reason = "change detection: both heights come from the same rounding of the code size"
        )]
        let is_zoom = previous.line_height != key.line_height;
        let keep_below = if is_zoom { sight.keep_below } else { 0.0 };
        let keep = self.top() + keep_below;
        let holds_keep = !self.is_embedded || (0.0..offsets.height()).contains(&keep);
        if self.scroll_to.is_some() || !holds_keep {
            return None;
        }
        offsets.anchor_at(keep, rows).map(|place| (place, keep_below))
    }

    /// Once the rows are placed at `offsets`: the comment card moved under its lines (`card`: how
    /// far below the top of the view it was, and its row) stays where it was on screen, the row
    /// kept for new heights (`anchor`) goes back to its place, and so does the row kept across a
    /// rebuild.
    pub fn keep_places(
        &mut self,
        card: Option<(f32, usize)>,
        anchor: Option<KeptRow>,
        offsets: &RowOffsets,
    ) {
        if let Some((below_top, card)) = card
            && self.scroll_to.is_none()
        {
            self.scroll_to = Some((offsets[card] - below_top).max(0.0));
        }
        if let Some(((row, fraction), below_top)) = anchor
            && let (Some(top), Some(bottom)) = (offsets.get(row), offsets.get(row + 1))
        {
            self.scroll_within(top + (bottom - top) * fraction - below_top);
        }
        if let Some((row, below_top)) = self.restore_row.take()
            && let Some(top) = offsets.get(row)
            && self.scroll_to.is_none()
        {
            self.scroll_within(top - below_top);
        }
    }

    /// Scrolls the view's top to `y`, kept from going above the first row; inside another view's
    /// scrolling, the request is that view's to clamp.
    fn scroll_within(&mut self, y: f32) {
        self.scroll_to = Some(if self.is_embedded { y } else { y.max(0.0) });
    }

    /// Scrolls the view's top to `y` for a jump (a thread): no further than the end of the rows
    /// placed at `offsets` in a view `view_height` tall, as without the room past it (see
    /// [`past_end`]); inside another view's scrolling, the request is that view's to clamp.
    ///
    /// [`past_end`]: crate::diff_overview::past_end
    pub fn jump(&mut self, y: f32, offsets: &RowOffsets, view_height: f32) {
        let end = (offsets.height() - view_height).max(0.0);
        self.scroll_to = Some(if self.is_embedded { y } else { y.min(end) });
    }
}
