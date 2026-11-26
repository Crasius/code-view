//! A pull request row the pointer rests on: the list reads the pages around it ahead then
//! ([`super::super::warm`]), so a click that follows finds the detail and the commits ready.
//!
//! Resting means the pointer moved on the row and has not moved since, for [`DWELL`]: a pointer
//! passing over rows on its way, or moving within one, starts nothing. Nor does a row that comes
//! under a pointer that did not move (the list scrolled, or the page changed under it): the user
//! did not point at it. A press (a click on one of the row's buttons, its menu) is an action, not
//! the pause before one: the row needs a new movement to rest.

use std::time::Duration;

use cv_runtime::route::PullRequestKey;
use cv_services::diff_stats::StatRow;
use egui::{Id, Rect, Ui};

use super::Painted;

/// How long the pointer rests on a row before its page warms up: our own number, about the pause
/// before a click that a pointer travelling across the list does not make.
pub const DWELL: Duration = Duration::from_millis(400);

/// True once the pointer rests on the row `id` at `rect` (see the module docs). Until then a row
/// the pointer moved on asks for the one repaint that ends the wait.
pub fn is_resting(ui: &Ui, id: Id, rect: Rect) -> bool {
    if !ui.rect_contains_pointer(rect) {
        return false;
    }
    let now = ui.input(|input| input.time);
    // Exactly zero in the frame of a movement: egui stamps it with that frame's time.
    let has_moved = ui.input(|input| input.pointer.time_since_last_movement() <= 0.0);
    let has_pressed = ui.input(|input| input.pointer.any_pressed() || input.pointer.any_down());
    let moved_at = ui.ctx().data_mut(|data| {
        let hovered =
            data.get_temp_mut_or_insert_with(Id::unique("list_row_dwell"), || (id, None::<f64>));
        if hovered.0 != id {
            *hovered = (id, None);
        }
        if has_pressed {
            hovered.1 = None;
        } else if has_moved {
            hovered.1 = Some(now);
        }
        hovered.1
    });
    let Some(moved_at) = moved_at else {
        return false;
    };
    let left = DWELL.as_secs_f64() - (now - moved_at);
    if left > 0.0 {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(left));
        return false;
    }
    true
}

/// When the pointer rests on the row `id` at `rect` ([`is_resting`]), showing `row`, painted
/// next: the user saw what changed in it, and the list may read ahead around it.
pub fn note_rest(ui: &Ui, id: Id, rect: Rect, painted: &mut Painted, row: &StatRow) {
    if !is_resting(ui, id, rect) {
        return;
    }
    painted.seen = Some(PullRequestKey {
        connection_id: row.connection_id.clone(),
        repository_key: row.repository_key.clone(),
        number: row.number,
    });
    // Never without a token: anonymous requests are scarce (60 an hour on GitHub).
    if row.has_token {
        painted.rested = Some(painted.ahead.len());
    }
}
