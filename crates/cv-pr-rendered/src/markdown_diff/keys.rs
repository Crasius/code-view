//! The keys that scroll the rendered diff, which has no focusable scroll area of its own: ↑/↓ a
//! step, Page Up/Down a screen, Home/End to the ends. The steps reach the room past the end (see
//! [`cv_pr_diff::diff_overview::past_end`]); End goes to the end of the rows.

use egui::{Key, Modifiers, Ui};

use cv_pr_diff::diff_overview::past_end::Ends;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollKey {
    Down,
    Up,
    PageDown,
    PageUp,
    Home,
    End,
}

impl ScrollKey {
    /// The scroll key pressed this frame without modifiers, while no field or button has the
    /// keyboard. Every one of them pressed is taken from the input; the first in this order wins.
    pub fn read(ui: &Ui) -> Option<Self> {
        let ctx = ui.ctx();
        if ctx.text_edit_focused() || ctx.memory(|memory| memory.focused().is_some()) {
            return None;
        }
        ui.input_mut(|input| {
            let is_plain =
                !input.modifiers.shift && !input.modifiers.alt && !input.modifiers.command;
            let keys = [
                (Key::ArrowDown, Self::Down),
                (Key::ArrowUp, Self::Up),
                (Key::PageDown, Self::PageDown),
                (Key::PageUp, Self::PageUp),
                (Key::Home, Self::Home),
                (Key::End, Self::End),
            ];
            let pressed = keys.map(|(key, scroll)| {
                (is_plain && input.consume_key(Modifiers::NONE, key)).then_some(scroll)
            });
            pressed.into_iter().flatten().next()
        })
    }

    /// Where the view's top goes from `top`: a `step` for an arrow, a `page` for Page Up/Down,
    /// the end of the rows for End, always within `0..=ends.max` (which holds the room past the
    /// end).
    pub fn target(self, top: f32, step: f32, page: f32, ends: Ends) -> f32 {
        let target = match self {
            Self::Down => top + step,
            Self::Up => top - step,
            Self::PageDown => top + page,
            Self::PageUp => top - page,
            Self::Home => 0.0,
            Self::End => ends.end,
        };
        target.clamp(0.0, ends.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_keys_move_by_a_step_a_page_or_to_the_ends_and_stay_in_range() {
        let ends = Ends { end: 1000.0, max: 1000.0, height: 1600.0 };
        let target = |key: ScrollKey, top: f32| key.target(top, 48.0, 600.0, ends);
        assert_eq!(target(ScrollKey::Down, 100.0), 148.0);
        assert_eq!(target(ScrollKey::Up, 20.0), 0.0, "not above the top");
        assert_eq!(target(ScrollKey::PageDown, 500.0), 1000.0, "not past the end");
        assert_eq!(target(ScrollKey::PageUp, 700.0), 100.0);
        assert_eq!(target(ScrollKey::Home, 700.0), 0.0);
        assert_eq!(target(ScrollKey::End, 0.0), 1000.0);
    }

    #[test]
    fn steps_reach_the_room_past_the_end_and_end_goes_to_the_rows_end() {
        let ends = Ends { end: 1000.0, max: 1500.0, height: 2100.0 };
        let target = |key: ScrollKey, top: f32| key.target(top, 48.0, 600.0, ends);
        assert_eq!(target(ScrollKey::PageDown, 800.0), 1400.0);
        assert_eq!(target(ScrollKey::Down, 1480.0), 1500.0, "not past the room");
        assert_eq!(target(ScrollKey::End, 1400.0), 1000.0, "End: the last line at the bottom");
        assert_eq!(target(ScrollKey::End, 0.0), 1000.0);
    }
}
