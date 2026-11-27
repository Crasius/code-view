//! Removed lines in New only: a wedge on the boundary between the rows around them, the popover
//! that reads them out, and the notice for a file whose new version is empty.

use egui::{Rect, Ui, vec2};
use review_core::ThreadAnchor;
use review_git::{FileDiff, LineKind};

use super::frame::Frame;
use super::{DiffEvent, DiffInputs};
use crate::removal_popover::{self, PopoverAction, PopoverInputs, RemovalPopover};
use cv_base::theme::icons;
use cv_code::diff_paint::Metrics;
use cv_code::diff_rows::{self, Layout, Row};
use cv_widgets::numbers::group_digits;
use cv_widgets::states::{self, Action};

/// The wedge of removal row `row_index`, `row`, on the boundary at `top` between the rows around
/// it, in rows `total_height` tall.
pub(super) fn show_wedge(
    ui: &mut Ui,
    removal: &mut RemovalPopover,
    (row_index, row): (usize, &Row),
    top: f32,
    diff: &FileDiff,
    frame: &Frame<'_>,
    total_height: f32,
) {
    let Some(lines) = row.removed_lines() else {
        return;
    };
    // Removals at the very top or bottom of the file keep the whole wedge in view.
    let edge = frame.metrics.line_height / 2.0;
    let y = top.clamp(edge.min(total_height / 2.0), (total_height - edge).max(total_height / 2.0));
    let pane = Rect::from_min_size(frame.origin + vec2(0.0, y), vec2(frame.width, 0.0));
    let label = removal_popover::wedge_label(diff, lines);
    removal.wedge(ui, row_index, pane, pane.top(), frame.metrics, &label);
}

/// What the open popover of removed lines, inside `bounds`, shows and offers: copying them, and a
/// comment on them when `can_comment`.
pub(super) struct Popover<'a> {
    pub metrics: &'a Metrics,
    pub bounds: Rect,
    pub can_comment: bool,
}

/// The popover of the removal row it was opened on, if one is open; a new comment the user asked
/// for there comes back with the row to open it at.
pub(super) fn show_popover(
    ui: &Ui,
    removal: &mut RemovalPopover,
    rows: &[Row],
    inputs: &DiffInputs<'_>,
    popover: &Popover<'_>,
) -> Option<(ThreadAnchor, usize)> {
    let row_index = removal.open_row()?;
    let row = rows.get(row_index).copied()?;
    let lines = row.removed_lines()?;
    let shown = PopoverInputs {
        id: ui.scope_id().with("removal_popover"),
        diff: inputs.diff,
        contents: inputs.contents,
        line_spans: inputs.line_spans,
        lines: lines.clone(),
        metrics: popover.metrics,
        can_comment: popover.can_comment,
        bounds: popover.bounds,
    };
    match removal.show(ui, &shown)? {
        PopoverAction::Copy => {
            ui.ctx().copy_text(removal_popover::removed_text(inputs.diff, lines));
            None
        }
        PopoverAction::Comment => {
            diff_rows::anchor_for(inputs.diff, &row, false, inputs.path, inputs.old_path)
                .map(|anchor| (anchor, row_index))
        }
    }
}

/// New only for a deleted or emptied file: say so and offer the layouts that show the removed lines.
pub(super) fn show_empty_head(ui: &mut Ui, diff: &FileDiff) -> Option<DiffEvent> {
    let removed = diff.lines().iter().filter(|line| line.kind == LineKind::Deleted).count();
    let detail = match removed {
        1 => "Its only line was removed, so New only has nothing to show.".to_owned(),
        count => format!(
            "All {} lines were removed, so New only has nothing to show.",
            group_digits(count)
        ),
    };
    let actions = [
        Action::primary(icons::ROWS, "Show inline"),
        Action::secondary(icons::COLUMNS, "Show side by side"),
    ];
    match states::message_with_actions(
        ui,
        icons::FILE_MINUS,
        "The new version is empty",
        &detail,
        &actions,
    )? {
        0 => Some(DiffEvent::SwitchLayout(Layout::Inline)),
        _ => Some(DiffEvent::SwitchLayout(Layout::SideBySide)),
    }
}
