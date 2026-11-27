//! Stepping through the changes of a diff, shared by the text diff and the rendered Markdown diff:
//! n / p (or F7 / Shift+F7, as in VS Code) and the header's arrows.

use egui::{Key, Modifiers, Ui};

use cv_base::platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Next,
    Previous,
}

/// Where navigation puts a change: a quarter down the view, so the context above stays visible.
const FOCUS_FRACTION: f32 = 0.25;

/// n / p (or F7 / Shift+F7) while no text field has focus; a focused button (a removal wedge, say)
/// does not take the letters.
pub fn navigation_keys(ui: &Ui) -> Option<Direction> {
    if ui.ctx().text_edit_focused() {
        return None;
    }
    ui.input_mut(|input| {
        // Plain keys only: Alt+N is New only.
        let previous =
            input.consume_key(Modifiers::SHIFT, Key::F7) || platform::consume_plain(input, Key::P);
        let next =
            platform::consume_plain(input, Key::F7) || platform::consume_plain(input, Key::N);
        if previous {
            Some(Direction::Previous)
        } else if next {
            Some(Direction::Next)
        } else {
            None
        }
    })
}

/// The change to step to among `count` changes, `top_of` giving where each starts. From the
/// `current` change it is the one before or after; without one, "next" starts with the first one
/// on screen (at or below `view_top`) and "previous" with the one above it.
pub fn step(
    direction: Direction,
    current: Option<usize>,
    count: usize,
    view_top: f32,
    top_of: impl Fn(usize) -> f32,
) -> Option<usize> {
    match (direction, current) {
        (Direction::Next, Some(current)) => (current + 1 < count).then_some(current + 1),
        (Direction::Previous, Some(current)) => current.checked_sub(1),
        (Direction::Next, None) => (0..count).find(|change| top_of(*change) >= view_top - 1.0),
        (Direction::Previous, None) => {
            (0..count).rev().find(|change| top_of(*change) < view_top - 1.0)
        }
    }
}

/// The scroll offset that puts content starting at `top` a quarter down a view `view_height` tall,
/// no further than where the bottom of rows `total` tall meets the view's: a jump never lands in
/// the room past the end (see [`past_end`]).
///
/// [`past_end`]: crate::diff_overview::past_end
pub fn focus_offset(top: f32, view_height: f32, total: f32) -> f32 {
    (top - view_height * FOCUS_FRACTION).min(total - view_height).max(0.0)
}

/// The scroll offset that puts the row spanning `row` in the middle of a view `view_height` tall,
/// as far as rows `total` tall allow.
pub fn center_offset(row: std::ops::Range<f32>, view_height: f32, total: f32) -> f32 {
    let middle = f32::midpoint(row.start, row.end);
    (middle - view_height / 2.0).min(total - view_height).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPS: [f32; 3] = [0.0, 100.0, 200.0];

    fn from(direction: Direction, current: Option<usize>, view_top: f32) -> Option<usize> {
        step(direction, current, TOPS.len(), view_top, |change| TOPS[change])
    }

    #[test]
    fn from_a_current_change_steps_to_its_neighbour_and_stops_at_the_ends() {
        assert_eq!(from(Direction::Next, Some(0), 0.0), Some(1));
        assert_eq!(from(Direction::Next, Some(2), 0.0), None, "the last change stays");
        assert_eq!(from(Direction::Previous, Some(2), 0.0), Some(1));
        assert_eq!(from(Direction::Previous, Some(0), 0.0), None, "the first change stays");
    }

    #[test]
    fn without_one_next_takes_the_first_on_screen_and_previous_the_one_above() {
        assert_eq!(from(Direction::Next, None, 150.0), Some(2));
        assert_eq!(from(Direction::Next, None, 100.5), Some(1), "a change at the top is on screen");
        assert_eq!(from(Direction::Next, None, 250.0), None, "all changes are above");
        assert_eq!(from(Direction::Previous, None, 150.0), Some(1));
        assert_eq!(from(Direction::Previous, None, 0.0), None, "nothing above the top");
        assert_eq!(step(Direction::Next, None, 0, 0.0, |_| 0.0), None, "no changes");
    }

    /// The direction [`navigation_keys`] makes of one press of `key` with `modifiers`.
    fn keys_of(key: Key, modifiers: Modifiers) -> Option<Direction> {
        let ctx = egui::Context::default();
        let event =
            egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let input = egui::RawInput { events: vec![event], ..egui::RawInput::default() };
        let mut direction = None;
        ctx.run_ui(input, |ui| direction = navigation_keys(ui)).drop_without_applying_deltas();
        direction
    }

    #[test]
    fn n_steps_to_the_next_change_but_alt_n_stays_new_only() {
        assert_eq!(keys_of(Key::N, Modifiers::NONE), Some(Direction::Next));
        assert_eq!(keys_of(Key::N, Modifiers::ALT), None, "Alt+N switches New only");
        assert_eq!(keys_of(Key::F7, Modifiers::SHIFT), Some(Direction::Previous));
    }

    #[test]
    fn the_change_goes_a_quarter_down_the_view_but_never_above_the_top() {
        assert_eq!(focus_offset(500.0, 400.0, 5000.0), 400.0);
        assert_eq!(focus_offset(50.0, 400.0, 5000.0), 0.0);
        assert_eq!(focus_offset(4900.0, 400.0, 5000.0), 4600.0, "no further than the end");
        assert_eq!(focus_offset(100.0, 400.0, 300.0), 0.0, "rows shorter than the view");
    }
}
