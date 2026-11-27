//! One block in its lane: the tint and bar of its change, the rendered block with its changed
//! words marked (and the find bar's hits over them, see [`super::find`]), and in the gutter its "+" or "−", the button to comment on its source lines, or
//! while comments are hidden the count of its threads.

use std::ops::Range;

use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, pos2, vec2,
};

use cv_base::theme::motion::mix;
use cv_base::theme::{Palette, icons, style};
use cv_markdown::Marks;
use cv_pr_threads::markers;
use cv_widgets::focus;

use super::align::Change;
use review_core::DiffSide;

use super::model::{Cell, Look, Mode};
use super::{Frame, GUTTER, ROW_PAD, RenderedEvent, STRIPE_WIDTH};

const BUTTON_SIZE: f32 = 18.0;
/// The gutter's "+" or "−", at zoom 1.
const MARKER_SIZE: f32 = 13.0;
/// How far down the first line of body text the gutter control is centered, in body sizes.
const MARKER_LINE_CENTER: f32 = 0.72;

/// (tint, bar and marker color, marker) of a block.
pub(super) fn look_colors(
    look: Look,
    palette: &Palette,
) -> (Color32, Color32, Option<&'static str>) {
    match look {
        Look::Plain => (Color32::TRANSPARENT, Color32::TRANSPARENT, None),
        Look::Added => (palette.diff_added_line, palette.success, Some("+")),
        Look::Removed => (palette.diff_deleted_line, palette.error, Some("\u{2212}")),
        Look::Focus => (
            mix(palette.surface, palette.accent, if palette.is_dark { 0.10 } else { 0.05 }),
            palette.accent,
            None,
        ),
    }
}

/// What showing a block gave.
pub(super) struct ShownCell {
    pub height: f32,
    pub event: Option<RenderedEvent>,
    /// The count of its hidden threads was clicked: show them, or hide them again.
    pub toggles_threads: bool,
}

/// One block in its lane: the tint, the bar, the block itself, and in the gutter its "+" or "−",
/// or under the pointer the button to comment on its source lines. While comments are hidden, the
/// count of its `hidden` threads takes the gutter instead.
pub(super) fn show_cell(
    ui: &mut Ui,
    frame: &Frame<'_>,
    cell: Cell,
    lane: Rect,
    palette: &Palette,
    hidden: Option<markers::Hidden>,
) -> ShownCell {
    let scale = frame.options.scale;
    let model = frame.model;
    let (tint, bar, marker) = look_colors(cell.look, palette);
    if tint != Color32::TRANSPARENT {
        ui.painter().rect_filled(lane, 0, tint);
        ui.painter().rect_filled(
            Rect::from_min_size(lane.min, vec2(STRIPE_WIDTH, lane.height())),
            0,
            bar,
        );
    }
    let text_top = lane.top() + ROW_PAD * scale;
    let text_left = frame.origin.x + frame.lanes.text(cell.side).min;
    let text = Rect::from_min_size(
        pos2(text_left, text_top),
        vec2(frame.lanes.text_width(cell.side), f32::INFINITY),
    );
    let pair = &model.pairs[cell.pair];
    let changed = (pair.change == Change::Modified
        && matches!(cell.look, Look::Added | Look::Removed))
    .then(|| match cell.side {
        DiffSide::Old => Marks { ranges: pair.old_words.clone(), color: palette.diff_deleted_word },
        DiffSide::New => Marks { ranges: pair.new_words.clone(), color: palette.diff_added_word },
    });
    // The find bar's hits are layered over the changed words.
    let found = frame.find.marks(cell, frame.mode, palette);
    let is_current = found.as_ref().is_some_and(|found| found.is_current);
    let marks: Vec<Marks> =
        changed.into_iter().chain(found.into_iter().flat_map(|found| found.layers)).collect();
    #[cfg(any(test, feature = "test-support"))]
    if marks.len() > 1 {
        note_layers(ui.ctx(), &marks);
    }
    let mut child = ui.new_child(
        UiBuilder::new().max_rect(text).layout(Layout::top_down(Align::Min)).id_salt((
            "markdown_diff_cell",
            cell.side,
            cell.unit,
        )),
    );
    let units = model.units(cell.side);
    // Images of added and removed blocks are framed in the change color, so a replaced image
    // reads as changed even where it fills the block.
    let border = (cell.look != Look::Plain).then_some(bar);
    let options = frame.lanes.options(model.options(frame.options, cell.side, border), cell.side);
    units.show(
        &mut child,
        frame.id.with(("unit", cell.side, cell.unit)),
        cell.unit,
        options,
        marks,
    );
    let height = child.min_rect().height();
    if is_current {
        let outline = Stroke::new(2.0, palette.warning);
        ui.painter().rect_stroke(
            lane.shrink(1.0),
            style::RADIUS_CONTROL,
            outline,
            StrokeKind::Inside,
        );
        #[cfg(any(test, feature = "test-support"))]
        cv_code::find_paint::note_current(ui.ctx(), lane, lane);
    }
    let gutter = Gutter {
        center: pos2(
            text_left - GUTTER / 2.0 + 1.0,
            text_top + style::BODY_SIZE * scale * MARKER_LINE_CENTER,
        ),
        is_hovered: !frame.is_resizing && ui.rect_contains_pointer(lane),
        marker,
        color: bar,
    };
    let (event, toggles_threads) = show_gutter(ui, frame, cell, &gutter, palette, hidden);
    ShownCell { height, event, toggles_threads }
}

/// Notes the fills of a rendered block that carries layered marks (see [`last_layers`]).
#[cfg(any(test, feature = "test-support"))]
fn note_layers(ctx: &egui::Context, layers: &[Marks]) {
    let colors: Vec<Color32> =
        layers.iter().filter(|layer| !layer.ranges.is_empty()).map(|layer| layer.color).collect();
    cv_base::frame_probe::note(ctx, LAYER_NOTE, colors);
}

/// The fill colors, layer by layer, of each rendered block the last frame painted with more than
/// one layer of marks.
#[cfg(any(test, feature = "test-support"))]
pub fn last_layers(ctx: &egui::Context) -> Vec<Vec<Color32>> {
    cv_base::frame_probe::last(ctx, LAYER_NOTE)
}

#[cfg(any(test, feature = "test-support"))]
const LAYER_NOTE: &str = "diff_find_layers";

/// Where a block's gutter control is centered, whether the block is hovered, and the change
/// marker it shows otherwise, in `color`.
struct Gutter {
    center: Pos2,
    is_hovered: bool,
    marker: Option<&'static str>,
    color: Color32,
}

/// The gutter of `cell`: the count of its `hidden` threads while comments are hidden, under the
/// pointer the button to comment on its source lines (or to show them in the text diff), else
/// its change marker. What the user did there, and true when the thread count was clicked.
fn show_gutter(
    ui: &mut Ui,
    frame: &Frame<'_>,
    cell: Cell,
    gutter: &Gutter,
    palette: &Palette,
    hidden: Option<markers::Hidden>,
) -> (Option<RenderedEvent>, bool) {
    let button = Rect::from_center_size(gutter.center, vec2(BUTTON_SIZE, BUTTON_SIZE));
    if let Some(hidden) = hidden {
        // As in the text diff, the threads' marker takes the place of the comment button.
        let id = frame.id.with(("thread_count", cell.side, cell.unit));
        return (None, markers::show_count(ui, button, id, &hidden));
    }
    let units = frame.model.units(cell.side);
    if gutter.is_hovered
        && let Some(unit) = units.units.get(cell.unit)
    {
        return (lines_button(ui, frame, cell, button, unit.lines.clone(), palette), false);
    }
    if let Some(marker) = gutter.marker {
        let font = FontId::monospace(MARKER_SIZE * frame.options.scale);
        ui.painter().text(gutter.center, Align2::CENTER_CENTER, marker, font, gutter.color);
    }
    (None, false)
}

/// The button at `button` that comments on the source `lines` of `cell` (or shows them in the
/// text diff where the user may not comment); what a click asks for.
fn lines_button(
    ui: &mut Ui,
    frame: &Frame<'_>,
    cell: Cell,
    button: Rect,
    lines: Range<u32>,
    palette: &Palette,
) -> Option<RenderedEvent> {
    let version = match (frame.mode, cell.side) {
        (Mode::Columns | Mode::Whole(_), _) | (_, DiffSide::New) => "",
        (_, DiffSide::Old) => " of the old version",
    };
    let (glyph, label) = if frame.can_comment {
        (icons::CHAT_CIRCLE_TEXT, format!("Comment on {}{version}", lines_label(&lines)))
    } else {
        (icons::FILE_TEXT, format!("Show {}{version} in the text diff", lines_label(&lines)))
    };
    let response = ui
        .interact(button, frame.id.with(("gutter", cell.side, cell.unit)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    let fill = if response.hovered() { palette.accent_hover } else { palette.accent };
    ui.painter().rect_filled(button, style::RADIUS_CONTROL, fill);
    ui.painter().text(
        button.center(),
        Align2::CENTER_CENTER,
        glyph,
        icons::font(icons::SIZE_SMALL, icons::Weight::Bold),
        palette.on_accent,
    );
    focus::ring(ui, &response, button.expand(2.0));
    let event = if frame.can_comment {
        RenderedEvent::Comment { side: cell.side, lines }
    } else {
        RenderedEvent::ShowSource { side: cell.side, lines }
    };
    response.clicked().then_some(event)
}

/// "line 12" or "lines 12–18" of a 1-based, end-exclusive range.
pub(super) fn lines_label(lines: &Range<u32>) -> String {
    let last = lines.end.saturating_sub(1).max(lines.start);
    if last == lines.start {
        format!("line {}", lines.start)
    } else {
        format!("lines {}\u{2013}{last}", lines.start)
    }
}
