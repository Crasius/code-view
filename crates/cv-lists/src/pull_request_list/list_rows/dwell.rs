//! A pull request row the pointer rests on: the list reads the pages around it ahead then
//! ([`super::super::warm`]), so a click that follows finds the detail and the commits ready.
//!
//! Resting means the pointer moved on the row and has not moved since, for [`DWELL`]: a pointer
//! passing over rows on its way, or moving within one, starts nothing. Nor does a row that comes
//! under a pointer that did not move (the list scrolled, or the page changed under it): the user
//! did not point at it. A press (a click on one of the row's buttons, its menu) is an action, not
//! the pause before one: the row needs a new movement to rest.

use std::time::Duration;

use cv_services::diff_stats::StatRow;
use egui::{Id, Rect, Ui};

use super::super::warm::key_of;
use super::Painted;

/// How long the pointer rests on a row before its page warms up: our own number, about the pause
/// before a click that a pointer travelling across the list does not make.
pub(crate) const DWELL: Duration = Duration::from_millis(400);

/// True once the pointer rests on the row `id` at `rect` (see the module docs). Until then a row
/// the pointer moved on asks for the one repaint that ends the wait.
pub(crate) fn is_resting(ui: &Ui, id: Id, rect: Rect) -> bool {
    if !ui.rect_contains_pointer(rect) {
        return false;
    }
    let now = ui.input(|input| input.time);
    // Exactly zero in the frame of a movement: egui stamps it with that frame's time.
    let has_moved = ui.input(|input| input.pointer.time_since_last_movement() <= 0.0);
    let has_pressed = ui.input(|input| input.pointer.any_pressed() || input.pointer.any_down());
    let left = ui.ctx().data_mut(|data| {
        let hovered =
            data.get_temp_mut_or_insert_with(Id::unique("list_row_dwell"), || (id, None::<f64>));
        update(hovered, id, has_moved, has_pressed, now)
    });
    let Some(left) = left else {
        return false;
    };
    if left > 0.0 {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(left));
        return false;
    }
    true
}

/// The rules of the module docs for one frame with the pointer on the row `id` at `now`:
/// `hovered` is the row the pointer was on and when it last moved there (`None`: not since it came
/// or was pressed). Returns the seconds left until the row rests, zero or less once it does;
/// `None` while it waits for a movement.
fn update(
    hovered: &mut (Id, Option<f64>),
    id: Id,
    has_moved: bool,
    has_pressed: bool,
    now: f64,
) -> Option<f64> {
    if hovered.0 != id {
        *hovered = (id, None);
    }
    if has_pressed {
        hovered.1 = None;
    } else if has_moved {
        hovered.1 = Some(now);
    }
    hovered.1.map(|moved_at| DWELL.as_secs_f64() - (now - moved_at))
}

/// When the pointer rests on the row `id` at `rect` ([`is_resting`]), showing `row`, painted
/// next: the user saw what changed in it, and the list may read ahead around it.
pub(crate) fn note_rest(ui: &Ui, id: Id, rect: Rect, painted: &mut Painted, row: &StatRow) {
    if !is_resting(ui, id, rect) {
        return;
    }
    painted.seen = Some(key_of(row));
    // Never without a token: anonymous requests are scarce (60 an hour on GitHub).
    if row.has_token {
        painted.rested = Some(painted.ahead.len());
    }
}

#[cfg(test)]
mod tests {
    use egui::Id;

    use super::{DWELL, update};

    /// One frame on `row` at `now`: whether the pointer moved and was pressed, and whether the row
    /// rests then.
    struct Frame {
        row: &'static str,
        now: f64,
        has_moved: bool,
        has_pressed: bool,
        rests: bool,
    }

    const fn frame(row: &'static str, now: f64, (has_moved, has_pressed): (bool, bool)) -> Frame {
        Frame { row, now, has_moved, has_pressed, rests: false }
    }

    const fn resting(row: &'static str, now: f64) -> Frame {
        Frame { row, now, has_moved: false, has_pressed: false, rests: true }
    }

    const STILL: (bool, bool) = (false, false);
    const MOVED: (bool, bool) = (true, false);
    const PRESSED: (bool, bool) = (false, true);

    #[test]
    fn a_row_rests_after_a_movement_and_a_still_dwell() {
        let after = DWELL.as_secs_f64() + 0.1;
        let cases: [(&str, Vec<Frame>); 4] = [
            (
                "a row that slid under a still pointer never rests",
                vec![frame("a", 1.0, STILL), frame("a", 1.0 + after, STILL)],
            ),
            (
                "a movement, then the dwell of stillness",
                vec![frame("a", 1.0, MOVED), frame("a", 1.1, STILL), resting("a", 1.0 + after)],
            ),
            (
                "a press asks for a new movement",
                vec![
                    frame("a", 1.0, MOVED),
                    frame("a", 1.1, PRESSED),
                    frame("a", 1.0 + after, STILL),
                    frame("a", 2.0, MOVED),
                    resting("a", 2.0 + after),
                ],
            ),
            (
                "another row starts the wait again",
                vec![
                    frame("a", 1.0, MOVED),
                    frame("b", 1.2, MOVED),
                    frame("b", 1.0 + after, STILL),
                    resting("b", 1.2 + after),
                    frame("a", 2.0, STILL),
                ],
            ),
        ];
        for (case, frames) in cases {
            let mut hovered = (Id::unique(frames[0].row), None);
            for Frame { row, now, has_moved, has_pressed, rests } in frames {
                let left = update(&mut hovered, Id::unique(row), has_moved, has_pressed, now);
                assert_eq!(left.is_some_and(|left| left <= 0.0), rests, "{case}: {row} at {now}");
            }
        }
    }
}
