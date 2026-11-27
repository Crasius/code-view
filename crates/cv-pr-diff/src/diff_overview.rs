//! The strip right of the diff: where changes, comments, the find bar's hits and the selection's
//! other occurrences are, and which part is on screen.
//! Clicking or dragging it scrolls there; its context menu switches "Scroll past the end"
//! ([`past_end`]), whose room the strip leaves out, and hands the switch to the caller to save.

pub mod past_end;
#[cfg(test)]
mod past_end_check;

use cv_base::theme::{self, Palette};
use cv_code::diff_rows::{Layout, Row, RowKind};
use cv_code::find_paint::{self, HitSide};
use egui::{CursorIcon, Id, Painter, PointerButton, Rangef, Rect, Sense, Stroke, Ui, pos2, vec2};
use review_git::{FileDiff, LineKind};

use self::past_end::Ends;

/// The strip's menu item that switches the room past the end.
pub const PAST_END_LABEL: &str = "Scroll past the end";

/// Rows `start..end` hold one block of consecutive changed lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeBlock {
    pub start: u32,
    pub end: u32,
    pub has_added: bool,
    pub has_deleted: bool,
}

#[derive(Debug, Default)]
pub struct Overview {
    pub blocks: Vec<ChangeBlock>,
    /// Rows of inline comment threads.
    pub threads: Vec<u32>,
    /// New only: rows of removal markers, drawn as red ticks.
    pub removals: Vec<u32>,
    /// New only: changed lines in the accent color, as the rows mark them.
    pub is_focus: bool,
    /// Rows of the find bar's hits and the side each was found on, and the current hit's row.
    pub finds: Vec<(u32, HitSide)>,
    pub current_find: Option<u32>,
    /// Rows of the other occurrences of the selected text.
    pub occurrences: Vec<u32>,
}

pub(crate) fn build(diff: &FileDiff, rows: &[Row], layout: Layout) -> Overview {
    let lines = diff.lines();
    let mut overview = Overview { is_focus: layout == Layout::NewOnly, ..Overview::default() };
    // A whole added or deleted file is one change; the strip then only shows the comments.
    let marks_changes = !matches!(layout, Layout::WholeFile(_));
    let mut current: Option<ChangeBlock> = None;
    for (index, row) in rows.iter().enumerate() {
        let index = index as u32;
        match row.kind {
            RowKind::Thread(_) => {
                overview.threads.push(index);
                continue;
            }
            _ if !marks_changes => continue,
            // A removal has no height, so it neither starts nor ends a block of changed lines.
            RowKind::Removal => {
                overview.removals.push(index);
                continue;
            }
            RowKind::Gap(_)
            | RowKind::Line
            | RowKind::Expanded
            | RowKind::Composer
            | RowKind::Separator => {}
        }
        let kind_of = |side: Option<u32>| {
            side.filter(|_| row.kind == RowKind::Line).map(|line| lines[line as usize].kind)
        };
        let has_added = kind_of(row.right) == Some(LineKind::Added);
        let has_deleted = kind_of(row.left) == Some(LineKind::Deleted);
        if !has_added && !has_deleted {
            overview.blocks.extend(current.take());
            continue;
        }
        let block = current.get_or_insert(ChangeBlock {
            start: index,
            end: index,
            has_added: false,
            has_deleted: false,
        });
        block.end = index + 1;
        block.has_added |= has_added;
        block.has_deleted |= has_deleted;
    }
    overview.blocks.extend(current);
    overview
}

/// What the user did on the strip this frame.
#[derive(Debug, Clone, Copy)]
pub struct StripPick {
    /// The scroll offset picked: clicked or dragged to (within the rows), or wheeled to with the
    /// pointer over the strip (into the room past the end too).
    pub scroll_to: Option<f32>,
    /// "Scroll past the end" was switched in the strip's menu, to this; the caller saves it.
    pub past_end: Option<bool>,
}

/// Paints the strip for rows with the room past the end when `past_end` is on, and returns what
/// the user picked on it. Its context menu switches that room.
pub fn show(
    ui: &Ui,
    strip: Rect,
    id: Id,
    overview: &Overview,
    offsets: &[f32],
    viewport: Rect,
    past_end: bool,
) -> StripPick {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter().with_clip_rect(strip);
    painter.rect_filled(strip, 0, palette.surface_subtle);
    painter.vline(strip.left(), strip.y_range(), Stroke::new(1.0, palette.border));
    let response =
        ui.interact(strip, id, Sense::click_and_drag()).on_hover_cursor(CursorIcon::PointingHand);
    let switch = past_end_menu(&response, past_end);
    let total = offsets.last().copied().unwrap_or(0.0);
    if total <= 0.0 {
        return StripPick { scroll_to: None, past_end: switch };
    }
    let scale = strip.height() / total.max(viewport.height());
    let y_of = |row: u32| strip.top() + offsets[row as usize] * scale;
    let inner = strip.shrink2(vec2(2.0, 0.0));
    paint_blocks(&painter, inner, overview, &y_of, &palette);
    paint_marks(&painter, strip, inner, overview, &y_of, &palette);
    let is_placed = |row: &u32| (*row as usize) < offsets.len();
    let occurrences =
        overview.occurrences.iter().filter(|row| is_placed(row)).map(|row| y_of(*row));
    find_paint::paint_occurrence_ticks(&painter, inner, occurrences, &palette);
    let finds = overview.finds.iter().filter(|(row, _)| is_placed(row));
    let finds = finds.map(|(row, side)| (y_of(*row), *side));
    let current = overview.current_find.filter(is_placed).map(y_of);
    find_paint::paint_ticks(&painter, inner, finds, current, &palette, overview.is_focus);

    let ends = Ends::of(offsets, viewport.height(), past_end);
    let is_scrollable = ends.end > 0.0;
    if is_scrollable {
        let view =
            Rect::from_x_y_ranges(strip.x_range(), view_box(strip.y_range(), total, viewport));
        #[cfg(any(test, feature = "test-support"))]
        ui.ctx().data_mut(|data| data.insert_temp(Id::unique(VIEW_BOX), (strip, view)));
        painter.rect_filled(view, 0, palette.text.gamma_multiply(0.07));
        painter.rect_stroke(
            view.shrink(0.5),
            0,
            Stroke::new(1.0, palette.text.gamma_multiply(0.18)),
            egui::StrokeKind::Inside,
        );
    }

    let wheel = ui.input(|input| input.smooth_scroll_delta.y);
    let scroll_to = if ui.rect_contains_pointer(strip) && wheel != 0.0 {
        Some((viewport.top() - wheel).clamp(0.0, ends.max))
    } else {
        // The secondary button opens the menu and leaves the view where it is.
        let is_primary = ui.input(|input| input.pointer.button_down(PointerButton::Primary));
        let pointer = response.interact_pointer_pos().filter(|_| {
            is_scrollable
                && ((response.is_pointer_button_down_on() && is_primary) || response.clicked())
        });
        pointer.map(|pointer| {
            let target = (pointer.y - strip.top()) / scale - viewport.height() / 2.0;
            target.clamp(0.0, ends.end)
        })
    };
    StripPick { scroll_to, past_end: switch }
}

/// The view box's span on a strip spanning `track`, for the part `view` (row coordinates) of rows
/// `total` tall: the rows fill the strip, so the box is the view's share of it. In the room past
/// the end the box rests at the bottom of the strip.
fn view_box(track: Rangef, total: f32, view: Rect) -> Rangef {
    let scale = track.span() / total.max(view.height());
    let top = view.top().min((total - view.height()).max(0.0));
    Rangef::new(track.min + top * scale, track.min + (top + view.height()) * scale)
}

/// The strip's context menu: "Scroll past the end", now `is_on` or off. Returns the setting the
/// user switched it to.
fn past_end_menu(response: &egui::Response, mut is_on: bool) -> Option<bool> {
    let mut switched = None;
    response.context_menu(|ui| {
        let item = ui.checkbox(&mut is_on, PAST_END_LABEL);
        if item.on_hover_text("Scroll on until the last line is at the top of the view").clicked() {
            switched = Some(is_on);
            ui.close();
            ui.ctx().request_repaint();
        }
    });
    switched
}

/// Where tests find the view box the strip painted last.
#[cfg(any(test, feature = "test-support"))]
const VIEW_BOX: &str = "overview_view_box";

/// The strip that painted a view box last, and the box, on screen.
#[cfg(any(test, feature = "test-support"))]
pub fn last_view_box(ctx: &egui::Context) -> Option<(Rect, Rect)> {
    ctx.data(|data| data.get_temp(Id::unique(VIEW_BOX)))
}

/// Forgets the view box painted last, so [`last_view_box`] waits for the next strip's.
#[cfg(any(test, feature = "test-support"))]
pub fn forget_view_box(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<(Rect, Rect)>(Id::unique(VIEW_BOX)));
}

/// The change blocks, left half red and right half green where a block has both. Blocks closer
/// than a pixel merge, so a 50k-line diff paints a few hundred rectangles at most.
fn paint_blocks(
    painter: &Painter,
    inner: Rect,
    overview: &Overview,
    y_of: &impl Fn(u32) -> f32,
    palette: &Palette,
) {
    let half = inner.width() / 2.0;
    let added = if overview.is_focus { palette.accent } else { palette.success };
    let paint = |(top, bottom, has_added, has_deleted): (f32, f32, bool, bool)| {
        let rows = Rangef::new(top, bottom.max(top + 2.0));
        let (left, right) = match (has_added, has_deleted) {
            (true, true) => (
                Some(Rangef::new(inner.left(), inner.left() + half)),
                Some(Rangef::new(inner.left() + half, inner.right())),
            ),
            (false, true) => (Some(inner.x_range()), None),
            _ => (None, Some(inner.x_range())),
        };
        if let Some(range) = left {
            painter.rect_filled(Rect::from_x_y_ranges(range, rows), 0, palette.error);
        }
        if let Some(range) = right {
            painter.rect_filled(Rect::from_x_y_ranges(range, rows), 0, added);
        }
    };
    let mut pending: Option<(f32, f32, bool, bool)> = None;
    for block in &overview.blocks {
        let (top, bottom) = (y_of(block.start), y_of(block.end));
        pending = match pending {
            Some((pending_top, pending_bottom, added, deleted)) if top - pending_bottom < 1.0 => {
                Some((pending_top, bottom, added || block.has_added, deleted || block.has_deleted))
            }
            Some(previous) => {
                paint(previous);
                Some((top, bottom, block.has_added, block.has_deleted))
            }
            None => Some((top, bottom, block.has_added, block.has_deleted)),
        };
    }
    if let Some(last) = pending {
        paint(last);
    }
}

/// Removals as short red ticks on the boundary between rows (merged when closer than a pixel),
/// and comments as accent dots at the strip's right edge.
fn paint_marks(
    painter: &Painter,
    strip: Rect,
    inner: Rect,
    overview: &Overview,
    y_of: &impl Fn(u32) -> f32,
    palette: &Palette,
) {
    let mut last_tick = f32::NEG_INFINITY;
    for row in &overview.removals {
        let y = y_of(*row);
        if y - last_tick >= 1.0 {
            painter.rect_filled(
                Rect::from_x_y_ranges(inner.x_range(), Rangef::new(y - 1.0, y + 2.0)),
                0,
                palette.error,
            );
            last_tick = y;
        }
    }
    for row in &overview.threads {
        let y = y_of(*row);
        painter.rect_filled(
            Rect::from_min_size(pos2(strip.right() - 4.0, y), vec2(4.0, 4.0)),
            1,
            palette.accent,
        );
    }
}

#[cfg(test)]
mod tests {
    use review_git::FileDiff;

    use super::*;
    use cv_code::diff_rows::{self, RowInputs};

    #[test]
    fn new_only_marks_added_blocks_and_ticks_removals() {
        let diff = FileDiff::parse_unified(b"@@ -1,5 +1,3 @@\n a\n-b\n-c\n+C\n d\n-e\n");
        let rows = diff_rows::build(&RowInputs {
            diff: &diff,
            layout: Layout::NewOnly,
            gaps: &[],
            reveals: &[],
            threads: &[],
            composer: None,
        })
        .rows;
        let overview = build(&diff, &rows, Layout::NewOnly);
        assert!(overview.is_focus);
        assert_eq!(overview.removals, [1, 4], "a | removal | C | d | removal");
        assert_eq!(
            overview.blocks,
            [ChangeBlock { start: 2, end: 3, has_added: true, has_deleted: false }]
        );
    }

    #[test]
    fn the_view_box_is_the_views_share_and_rests_at_the_bottom_past_the_end() {
        let track = Rangef::new(10.0, 410.0);
        let at =
            |top: f32| view_box(track, 2000.0, Rect::from_x_y_ranges(0.0..=1.0, top..=top + 500.0));
        assert_eq!(
            at(0.0),
            Rangef::new(10.0, 110.0),
            "a quarter of the rows: a quarter of the strip"
        );
        assert_eq!(at(1500.0), Rangef::new(310.0, 410.0), "at the end the box meets the bottom");
        assert_eq!(at(1950.0), at(1500.0), "in the room past the end it rests there");
        assert_eq!(at(750.0).span(), at(0.0).span(), "the box keeps its size on the way");
        let short = view_box(track, 300.0, Rect::from_x_y_ranges(0.0..=1.0, 0.0..=500.0));
        assert_eq!(short, Rangef::new(10.0, 410.0), "rows shorter than the view fill no more");
    }

    #[test]
    fn whole_files_mark_no_change_blocks() {
        let diff = FileDiff::parse_unified(b"@@ -0,0 +1,2 @@\n+a\n+b\n");
        let layout = Layout::WholeFile(review_core::DiffSide::New);
        let rows = diff_rows::build(&RowInputs {
            diff: &diff,
            layout,
            gaps: &[],
            reveals: &[],
            threads: &[],
            composer: None,
        })
        .rows;
        assert_eq!(build(&diff, &rows, layout).blocks, []);
    }
}
