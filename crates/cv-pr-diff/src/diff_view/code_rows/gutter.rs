//! The comment buttons in the marker column of a code row: on the hovered line, and "Comment on
//! lines X–Y" on the line the selection was last moved to.

use egui::{Align2, CursorIcon, Rect, RectAlign, Sense, Tooltip, Ui, pos2, vec2};

use crate::diff_view::COMMENT_SHORTCUT;
use crate::diff_view::selecting::RangeTarget;
use cv_base::theme::{self, icons};
use cv_code::diff_paint::Metrics;
use cv_code::line_source::Pane;

/// Where the hint of the "Comment on lines X–Y" button goes when it does not fit on the far side of
/// the selection: beside the gutter, never over the selected lines.
const HINT_BESIDE_BELOW: [RectAlign; 2] = [RectAlign::LEFT_END, RectAlign::LEFT_START];
const HINT_BESIDE_ABOVE: [RectAlign; 2] = [RectAlign::LEFT_START, RectAlign::LEFT_END];
/// The comment button: a speech bubble on an accent square in the marker column.
const COMMENT_BUTTON_SIZE: f32 = 18.0;

/// What happened to a line's comment button this frame.
pub(super) struct ButtonPress {
    pub clicked: bool,
    /// The pointer went down on it: a drag over the lines below may follow.
    pub pointer_down: bool,
}

/// The comment button of the hovered line on row `row_index`, in the marker column of `pane`
/// (the pane `which`). Dragging it down picks several lines, as on GitHub.
pub(super) fn comment_button(
    ui: &mut Ui,
    pane: Rect,
    row_index: usize,
    which: Pane,
    two_numbers: bool,
    metrics: &Metrics,
) -> ButtonPress {
    let palette = theme::palette(ui.ctx());
    let button = comment_button_rect(metrics, pane, two_numbers);
    let response = ui
        .interact(
            button,
            ui.scope_id().with(("add_comment", row_index, which as u8)),
            Sense::click_and_drag(),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Add a comment on this line. Drag down to comment on several lines.");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Add comment"));
    let fill = if response.hovered() { palette.accent_hover } else { palette.accent };
    paint_comment_button(ui, button, fill, palette.on_accent);
    ButtonPress { clicked: response.clicked(), pointer_down: response.is_pointer_button_down_on() }
}

/// The comment button on the selected line the selection was last moved to: "Comment on lines
/// X–Y". Where the platform does not take a comment on these lines, it stays muted and says
/// what to select instead. True when clicked.
pub(super) fn range_button(
    ui: &mut Ui,
    pane: Rect,
    two_numbers: bool,
    metrics: &Metrics,
    target: &RangeTarget,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let button = comment_button_rect(metrics, pane, two_numbers);
    let label = format!("Comment on {}", target.label);
    let is_allowed = target.blocked.is_none();
    // Senses clicks even when muted, so a press on it never starts a new selection underneath.
    let response =
        ui.interact(button, ui.scope_id().with("comment_selected_lines"), Sense::click());
    let hint = match target.blocked {
        Some(reason) => reason.to_owned(),
        None => format!("{label} ({COMMENT_SHORTCUT})"),
    };
    let response = response.on_hover_cursor(if is_allowed {
        CursorIcon::PointingHand
    } else {
        CursorIcon::NotAllowed
    });
    // The button sits on the first or last selected line: the hint goes past that line, away
    // from the selection (or beside the gutter where the view ends), never over selected code.
    let line = Rect::from_min_max(
        pos2(button.left(), pane.top().min(button.top())),
        pos2(button.right(), pane.bottom().max(button.bottom())),
    );
    let (align, beside) = if target.focus_is_start {
        (RectAlign::TOP_START, &HINT_BESIDE_ABOVE)
    } else {
        (RectAlign::BOTTOM_START, &HINT_BESIDE_BELOW)
    };
    let mut tooltip = Tooltip::for_enabled(&response);
    tooltip.popup = tooltip.popup.anchor(line).align(align).align_alternatives(beside);
    tooltip.show(|ui| {
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.label(hint);
    });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, is_allowed, &label));
    let fill = match (is_allowed, response.hovered()) {
        (false, _) => palette.text_disabled,
        (true, true) => palette.accent_hover,
        (true, false) => palette.accent,
    };
    paint_comment_button(ui, button, fill, palette.on_accent);
    is_allowed && response.clicked()
}

/// The comment button's place: centred in the marker column of the line's first visual row.
fn comment_button_rect(metrics: &Metrics, pane: Rect, two_numbers: bool) -> Rect {
    let marker = metrics.marker_rect(pane, two_numbers, pane.height());
    Rect::from_center_size(marker.center(), vec2(COMMENT_BUTTON_SIZE, COMMENT_BUTTON_SIZE))
}

/// A speech bubble on an accent square, as Azure DevOps marks "comment here".
fn paint_comment_button(ui: &Ui, button: Rect, fill: egui::Color32, glyph: egui::Color32) {
    ui.painter().rect_filled(button, 4, fill);
    let font = icons::font(icons::SIZE_SMALL, icons::Weight::Bold);
    ui.painter().text(button.center(), Align2::CENTER_CENTER, icons::CHAT_CIRCLE_TEXT, font, glyph);
}
