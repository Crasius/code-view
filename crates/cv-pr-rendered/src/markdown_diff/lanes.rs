//! The geometry of a frame: where the header, the banner, the rows and the overview strip go
//! ([`Areas`]), the lanes of the old and the new side (or of the one column) with where a block's
//! text runs in each ([`Lanes`]), and the reading column's edges the user drags to resize it.

use egui::{CursorIcon, Id, Rangef, Rect, Sense, Stroke, Ui, pos2, vec2};
use review_core::DiffSide;
use review_store::RenderedWidth;

use super::model::{Mode, slot};
use super::{GUTTER, RIGHT_PADDING};
use cv_base::theme;
use cv_markdown::RenderOptions;
use cv_widgets::splitter::share_range;

const HEADER_HEIGHT: f32 = 30.0;
const BANNER_HEIGHT: f32 = 40.0;

/// A block's text is never narrower than this.
const MIN_TEXT_WIDTH: f32 = 40.0;
/// How wide the grip on each edge of the reading column is.
const EDGE_HIT: f32 = 8.0;
/// The reading column's width changes in steps of this while its edge is dragged.
const EDGE_STEP: f32 = 4.0;
/// A column dragged within this of its lane's room snaps to full width; a full lane's edge
/// dragged inward past that becomes a column again.
const SNAP_DISTANCE: f32 = 48.0;

/// Where the parts of the rendered diff go in `full`, the space it has.
#[derive(Debug, Clone, Copy)]
pub(super) struct Areas {
    pub full: Rect,
    /// Side by side: "Before" and "After" over the lanes.
    pub header: Option<Rect>,
    /// Over a diff without visible changes: why.
    pub banner: Option<Rect>,
    /// The rows' scroll area.
    pub content: Rect,
    /// The overview strip right of the rows.
    pub strip: Rect,
    /// The old lane's width side by side (the `split` share of the content), else the content's.
    pub left_width: f32,
}

impl Areas {
    /// `strip` is the overview strip's width (none where the rendered diff is embedded in a card).
    pub fn new(full: Rect, mode: Mode, has_banner: bool, split: f32, strip: f32) -> Self {
        let header = (mode == Mode::Columns)
            .then(|| Rect::from_min_size(full.min, vec2(full.width(), HEADER_HEIGHT)));
        let top = full.top()
            + header.map_or(0.0, |header| header.height())
            + if has_banner { BANNER_HEIGHT } else { 0.0 };
        let banner = has_banner.then(|| {
            Rect::from_min_max(pos2(full.left(), top - BANNER_HEIGHT), pos2(full.right(), top))
        });
        let body = Rect::from_min_max(pos2(full.left(), top), full.max);
        let content = Rect::from_min_max(body.min, pos2(body.right() - strip, body.bottom()));
        let strip = Rect::from_min_max(pos2(content.right(), body.top()), body.max);
        let left_width = if mode == Mode::Columns {
            let range = share_range(content.width());
            (content.width() * split.clamp(*range.start(), *range.end())).round()
        } else {
            content.width()
        };
        Self { full, header, banner, content, strip, left_width }
    }
}

/// Horizontal geometry of a frame, relative to the content's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Lanes {
    /// The old (or only) lane and, side by side, the new one.
    pub first: Rangef,
    pub second: Option<Rangef>,
    /// Where a block's text runs in the old and in the new lane.
    pub texts: [Rangef; 2],
    /// How wide a top-level table may grow, centered on the text, in the old and the new lane.
    pub wides: [f32; 2],
    /// The old and the new lane set a reading column narrower than the lane.
    pub reading: [bool; 2],
}

impl Lanes {
    /// With a `column` width (Reading) the text is a column of that width centered in each lane,
    /// and tables may grow past it up to the lane's room; a column as wide as the room, or none,
    /// fills the lane (tables share it out, code scrolls in its box).
    pub fn new(mode: Mode, width: f32, left_width: f32, column: Option<f32>) -> Self {
        let (first, second) = match mode {
            Mode::Columns => (Rangef::new(0.0, left_width), Some(Rangef::new(left_width, width))),
            Mode::Unified | Mode::NewOnly | Mode::Whole(_) => (Rangef::new(0.0, width), None),
        };
        let place = |lane: Rangef| match column {
            Some(column) if column < reading_room(lane) => {
                let half = column.max(MIN_TEXT_WIDTH) / 2.0;
                (Rangef::new(lane.center() - half, lane.center() + half), reading_room(lane), true)
            }
            _ => {
                let text = Rangef::new(
                    lane.min + GUTTER,
                    (lane.max - RIGHT_PADDING).max(lane.min + GUTTER + MIN_TEXT_WIDTH),
                );
                (text, text.span(), false)
            }
        };
        let (old, new) = (place(first), place(second.unwrap_or(first)));
        Self {
            first,
            second,
            texts: [old.0, new.0],
            wides: [old.1, new.1],
            reading: [old.2, new.2],
        }
    }

    pub fn lane(&self, side: DiffSide) -> Rangef {
        match (side, self.second) {
            (DiffSide::New, Some(second)) => second,
            _ => self.first,
        }
    }

    pub fn text(&self, side: DiffSide) -> Rangef {
        self.texts[slot(side)]
    }

    pub fn text_width(&self, side: DiffSide) -> f32 {
        self.text(side).span()
    }

    /// The text with its gutter and right padding: where a block's marker and its cards go.
    pub fn column(&self, side: DiffSide) -> Rangef {
        let text = self.text(side);
        Rangef::new(text.min - GUTTER, text.max + RIGHT_PADDING)
    }

    /// `options` for the blocks of `side`: how far its tables may grow.
    pub fn options(&self, options: RenderOptions, side: DiffSide) -> RenderOptions {
        RenderOptions { breakout: self.wides[slot(side)], ..options }
    }

    /// Every lane fills its width: the toggle shows "Full width".
    pub fn fills(&self) -> bool {
        !self.reading.contains(&true)
    }
}

/// What the user did to the reading column's edges this frame.
#[derive(Default)]
pub(super) struct Edge {
    pub dragged: bool,
    /// A drag ended or a double-click reset the width: save it.
    pub committed: bool,
}

/// The reading column's width at `scale` for [`Lanes::new`]: side by side each half fills its
/// width, and one column is a reading column unless `width` is Full width.
pub(super) fn reading_column(width: RenderedWidth, mode: Mode, scale: f32) -> Option<f32> {
    (!width.full && mode != Mode::Columns).then(|| (width.reading * scale).round())
}

/// The widest reading column `lane` holds: its width less a gutter on each side.
pub(super) fn reading_room(lane: Rangef) -> f32 {
    lane.span() - 2.0 * GUTTER
}

/// The reading column's edges in one column (in full width only the right one): dragging one
/// widens or narrows the column on both sides, so it stays centered, and a double-click resets
/// it. A column dragged as wide as its lane allows switches to full width, and dragging the
/// edge of a full lane inward goes back to a column.
pub(super) fn show_edges(
    ui: &Ui,
    content: Rect,
    lanes: &Lanes,
    mode: Mode,
    scale: f32,
    width: &mut RenderedWidth,
    id: Id,
) -> Edge {
    let palette = theme::palette(ui.ctx());
    let mut edge = Edge::default();
    // Side by side each half takes its whole width: there is no column to resize.
    if mode == Mode::Columns {
        return edge;
    }
    let side = DiffSide::New;
    let lane = lanes.lane(side);
    let text = lanes.text(side);
    let xs: Vec<f32> = if lanes.reading[slot(side)] {
        vec![text.min - GUTTER, text.max + GUTTER]
    } else {
        vec![lane.max - RIGHT_PADDING / 2.0 - EDGE_HIT / 2.0]
    };
    for (index, x) in xs.into_iter().enumerate() {
        let x = content.left() + x;
        let hit = Rect::from_x_y_ranges(x - EDGE_HIT / 2.0..=x + EDGE_HIT / 2.0, content.y_range());
        let response = ui
            .interact(hit, id.with(("reading_edge", side, index)), Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag to resize the reading column; double-click to reset it");
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Reading width"));
        if response.double_clicked() {
            *width = RenderedWidth::default();
            edge.committed = true;
        } else if response.dragged()
            && response.drag_delta().x != 0.0
            && let Some(pointer) = response.interact_pointer_pos()
        {
            edge.dragged = true;
            let center = content.left() + lane.center();
            let column = 2.0 * ((pointer.x - center).abs() - GUTTER);
            if column >= reading_room(lane) - SNAP_DISTANCE {
                width.full = true;
            } else {
                width.full = false;
                // Steps of a few points: dragging lays the blocks out again at fewer widths.
                let stepped = (column / EDGE_STEP).round() * EDGE_STEP;
                width.reading = (stepped / scale).max(RenderedWidth::MIN_READING);
            }
            ui.ctx().request_repaint();
        } else if response.dragged() {
            edge.dragged = true;
        }
        if response.drag_stopped() {
            edge.committed = true;
        }
        if response.hovered() || response.dragged() {
            let color = if response.dragged() { palette.accent } else { palette.border_strong };
            ui.painter().vline(x, content.y_range(), Stroke::new(1.0, color));
            let handle = Rect::from_center_size(pos2(x, content.center().y), vec2(4.0, 32.0));
            ui.painter().rect_filled(handle, 2, color);
        }
    }
    edge
}
