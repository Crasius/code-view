//! Rows a write of the user's moved out of their list (a merge out of the open list): each stays in
//! its place for [`LEAVE_SECONDS`], fading and closing up, so the rows below glide into its place
//! instead of jumping. It no longer takes clicks or the keyboard; under reduced motion it is gone at
//! once.

use cv_base::theme::motion;
use egui::{Context, Rect, Ui, UiBuilder, vec2};
use review_core::PullRequestSummary;

use super::super::queries::{ConnectionResult, LEAVE_SECONDS};
use super::{Drawn, Hosts, ROW_HEIGHT, Row, RowFilter, Slot, paint_pull_request};

/// How far a row that started to leave at input time `since` is gone: 0 to 1, eased. Repaints
/// while it moves.
fn gone(ctx: &Context, since: f64) -> f32 {
    let total = motion::duration(ctx, LEAVE_SECONDS);
    let elapsed = ctx.input(|input| input.time) - since;
    if total <= 0.0 || elapsed >= f64::from(total) {
        return 1.0;
    }
    ctx.request_repaint();
    motion::ease_out_cubic(elapsed as f32 / total)
}

/// Puts the rows leaving `result` that `filter` keeps back among its rows, which start at `first`
/// in `body`, where they were.
pub(super) fn insert<'a>(
    ctx: &Context,
    body: &mut Vec<Row<'a>>,
    first: usize,
    result: &'a ConnectionResult,
    filter: &RowFilter,
) {
    for leaving in &result.leaving {
        let gone = gone(ctx, leaving.since);
        if gone < 1.0 && filter.matches(&result.connection_id, &leaving.row) {
            let at = (first + leaving.index).min(body.len());
            body.insert(at, Row::Leaving(result, &leaving.row, gone));
        }
    }
}

/// [`insert`] for the Favorites card, whose rows mix every connection's, most recently updated
/// first: a leaving row goes back where that order puts it.
pub(super) fn insert_by_update<'a>(
    ctx: &Context,
    body: &mut Vec<Row<'a>>,
    first: usize,
    results: &'a [ConnectionResult],
    filter: &RowFilter,
) {
    for result in results {
        for leaving in &result.leaving {
            let gone = gone(ctx, leaving.since);
            if gone >= 1.0 || !filter.matches(&result.connection_id, &leaving.row) {
                continue;
            }
            let later = body[first..]
                .iter()
                .take_while(|row| matches!(row, Row::Item(_, shown) if shown.last_activity() >= leaving.row.last_activity()))
                .count();
            body.insert(first + later, Row::Leaving(result, &leaving.row, gone));
        }
    }
}

/// The height of a row that is `gone` that far.
pub(super) fn height(gone: f32) -> f32 {
    ROW_HEIGHT * (1.0 - gone)
}

/// Paints the leaving `pull_request` in `slot`, as tall as it still is: the full row, faded and cut
/// off at the slot's bottom, and out of reach of clicks and the keyboard.
pub(super) fn paint(
    ui: &mut Ui,
    slot: Slot,
    gone: f32,
    (result, pull_request): (&ConnectionResult, &PullRequestSummary),
    hosts: &Hosts<'_>,
    drawn: &mut Drawn<'_>,
) {
    let full = Rect::from_min_size(slot.rect.min, vec2(slot.rect.width(), ROW_HEIGHT));
    let mut ghost = ui.new_child(UiBuilder::new().max_rect(full));
    ghost.disable();
    ghost.set_clip_rect(slot.rect.intersect(ui.clip_rect()));
    ghost.multiply_opacity(1.0 - gone);
    // No place of its own: J and K never land on it.
    let slot = Slot { rect: full, place: usize::MAX, ..slot };
    let _gone_rows_ask_nothing =
        paint_pull_request(&mut ghost, slot, result, pull_request, hosts, drawn);
}
