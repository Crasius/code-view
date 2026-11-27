//! The keys that scroll the rendered diff, which has no focusable scroll area of its own: ↑/↓ a
//! step, Page Up/Down a screen, Home/End to the ends. The steps reach the room past the end (see
//! [`cv_pr_diff::diff_overview::past_end`]); End goes to the end of the rows.

use egui::Ui;

use cv_pr_diff::diff_overview::past_end::Ends;
use cv_widgets::scroll_keys::ScrollKey;

/// The scroll key pressed this frame without modifiers, while no field or button has the
/// keyboard (see [`ScrollKey::consume`]).
pub(super) fn read(ui: &Ui) -> Option<ScrollKey> {
    let ctx = ui.ctx();
    if ctx.text_edit_focused() || ctx.memory(|memory| memory.focused().is_some()) {
        return None;
    }
    ui.input_mut(ScrollKey::consume)
}

/// Where the view's top goes from `top` for `key`: a `step` for an arrow, a `page` for Page
/// Up/Down, the end of the rows for End, always within `0..=ends.max` (which holds the room past
/// the end).
pub(super) fn target(key: ScrollKey, top: f32, step: f32, page: f32, ends: Ends) -> f32 {
    key.target(top, step, page, ends.end).clamp(0.0, ends.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_keys_move_by_a_step_a_page_or_to_the_ends_and_stay_in_range() {
        let ends = Ends { end: 1000.0, max: 1000.0, height: 1600.0 };
        let to = |key: ScrollKey, top: f32| target(key, top, 48.0, 600.0, ends);
        assert_eq!(to(ScrollKey::Down, 100.0), 148.0);
        assert_eq!(to(ScrollKey::Up, 20.0), 0.0, "not above the top");
        assert_eq!(to(ScrollKey::PageDown, 500.0), 1000.0, "not past the end");
        assert_eq!(to(ScrollKey::PageUp, 700.0), 100.0);
        assert_eq!(to(ScrollKey::Home, 700.0), 0.0);
        assert_eq!(to(ScrollKey::End, 0.0), 1000.0);
    }

    #[test]
    fn steps_reach_the_room_past_the_end_and_end_goes_to_the_rows_end() {
        let ends = Ends { end: 1000.0, max: 1500.0, height: 2100.0 };
        let to = |key: ScrollKey, top: f32| target(key, top, 48.0, 600.0, ends);
        assert_eq!(to(ScrollKey::PageDown, 800.0), 1400.0);
        assert_eq!(to(ScrollKey::Down, 1480.0), 1500.0, "not past the room");
        assert_eq!(to(ScrollKey::End, 1400.0), 1000.0, "End: the last line at the bottom");
        assert_eq!(to(ScrollKey::End, 0.0), 1000.0);
    }
}
