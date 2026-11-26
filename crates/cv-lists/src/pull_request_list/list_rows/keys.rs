//! J and K on a pull request list: the keyboard focus moves from row to row over the open cards,
//! in the order they are drawn; the first J (or K) focuses the first row. Enter then opens the
//! focused row and Shift+Enter its Files tab ([`super::item`]).
//!
//! Rows are painted only where they meet the viewport, and egui drops the focus of a widget that
//! is not drawn. So a key names a target row: the list scrolls it into view, and the row takes the
//! focus when it is drawn, in the same frame when it is on screen, else in the next one.

use cv_widgets::focus;
use egui::{Context, Id, Key, Modifiers, Ui};

/// The list's keyboard state, kept between frames in egui's memory.
#[derive(Clone, Copy, Default)]
pub(super) struct RowKeys {
    /// The row that had the keyboard when last drawn: its place among the rows, and its id.
    focused: Option<(usize, Id)>,
    /// The row J or K moved to, focused once it is drawn.
    target: Option<usize>,
}

impl RowKeys {
    pub fn load(ctx: &Context, id: Id) -> Self {
        ctx.data(|data| data.get_temp(id)).unwrap_or_default()
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|data| data.insert_temp(id, self));
    }

    /// Reads J and K outside text fields; `rows` counts the rows that can take the focus, asked
    /// only when a key was pressed.
    pub fn read(&mut self, ui: &Ui, rows: impl FnOnce() -> usize) {
        if ui.ctx().text_edit_focused() {
            return;
        }
        let (next, previous) = ui.input_mut(|input| {
            (input.consume_key(Modifiers::NONE, Key::J), input.consume_key(Modifiers::NONE, Key::K))
        });
        if !next && !previous {
            return;
        }
        let Some(last) = rows().checked_sub(1) else {
            return;
        };
        let current = self
            .focused
            .filter(|(_, id)| ui.memory(|memory| memory.has_focus(*id)))
            .map(|(place, _)| place);
        self.target = Some(step(current, next, last));
        focus::use_keyboard(ui.ctx());
    }

    /// The row to scroll into view.
    pub fn target(&self) -> Option<usize> {
        self.target
    }

    /// Row `place` was drawn as widget `id`: it takes the focus when it is the target.
    pub fn drawn(&mut self, ui: &Ui, place: usize, id: Id) {
        if self.target == Some(place) {
            ui.memory_mut(|memory| memory.request_focus(id));
            self.target = None;
        }
        if ui.memory(|memory| memory.has_focus(id)) {
            self.focused = Some((place, id));
        }
    }
}

/// The row a key moves to from `current` (none: the first row), within `0..=last`.
fn step(current: Option<usize>, is_next: bool, last: usize) -> usize {
    match current {
        None => 0,
        Some(place) if is_next => (place + 1).min(last),
        Some(place) => place.saturating_sub(1),
    }
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn keys_start_at_the_first_row_and_stop_at_the_ends() {
        assert_eq!(step(None, true, 9), 0, "the first J focuses the first row");
        assert_eq!(step(None, false, 9), 0, "so does the first K");
        assert_eq!(step(Some(0), true, 9), 1);
        assert_eq!(step(Some(9), true, 9), 9, "J stays on the last row");
        assert_eq!(step(Some(0), false, 9), 0, "K stays on the first row");
        assert_eq!(step(Some(4), false, 9), 3);
    }
}
