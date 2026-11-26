//! The parts of a find bar the views share: the search field, which keeps the keyboard after
//! Enter so Enter and Shift+Enter step through the matches, the ↑/↓ step buttons, and stepping
//! through the matches with wrap-around. The pipeline log's search and the diff's find bar build
//! on them; what they search and how they show the count stays theirs.

use cv_base::theme::icons;
use egui::text_edit::TextEditState;
use egui::text_selection::CCursorRange;
use egui::{Id, Key, Margin, Response, TextEdit, Ui};

use super::buttons;

/// Which way to move from the current match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

impl Step {
    /// Shift goes back, as Shift+Enter and Shift+F3 do.
    pub fn with_shift(shift: bool) -> Self {
        if shift { Step::Previous } else { Step::Next }
    }
}

/// The match after (or before) `current` among `count`, wrapping around at either end; without a
/// current one, the first (or last). `None` without matches.
pub fn step_index(current: Option<usize>, count: usize, step: Step) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match (step, current) {
        (Step::Next, Some(current)) if current < last => current + 1,
        (Step::Next, _) => 0,
        (Step::Previous, Some(current)) if current > 0 && current <= last => current - 1,
        (Step::Previous, _) => last,
    })
}

/// How a search field looks.
pub struct FieldLook<'a> {
    pub id: Id,
    /// Shown while the field is empty.
    pub hint: &'a str,
    pub width: f32,
    /// A tighter margin where the field keeps to a bar's height.
    pub margin: Option<Margin>,
}

/// What a search field did this frame.
pub struct FieldOutcome {
    pub response: Response,
    /// Enter (Shift+Enter) was pressed in it: the field keeps the keyboard.
    pub submitted: Option<Step>,
}

/// A one-line search field. Enter keeps the keyboard in it and reports the step it asks for.
pub fn field(ui: &mut Ui, text: &mut String, look: &FieldLook<'_>) -> FieldOutcome {
    let mut edit =
        TextEdit::singleline(text).id(look.id).hint_text(look.hint).desired_width(look.width);
    if let Some(margin) = look.margin {
        edit = edit.margin(margin);
    }
    let response = ui.add(edit);
    let submitted = (response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
        .then(|| Step::with_shift(ui.input(|input| input.modifiers.shift)));
    if submitted.is_some() {
        response.request_focus();
    }
    FieldOutcome { response, submitted }
}

/// Puts the keyboard into the field `id` holding `text`, all of it selected, so typing replaces it.
pub fn focus_all(ui: &Ui, id: Id, text: &str) {
    let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::select_all_str(text)));
    state.store(ui.ctx(), id);
    ui.memory_mut(|memory| memory.request_focus(id));
}

/// The ↓ and ↑ buttons, laid out in that order (right to left in a bar's trailing tools, so ↑
/// shows first); the step clicked.
pub fn step_buttons(ui: &mut Ui, next_hint: &str, previous_hint: &str) -> Option<Step> {
    let next = buttons::icon(ui, icons::ARROW_DOWN, next_hint).clicked();
    let previous = buttons::icon(ui, icons::ARROW_UP, previous_hint).clicked();
    match (next, previous) {
        (true, _) => Some(Step::Next),
        (false, true) => Some(Step::Previous),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, step_index};

    #[test]
    fn steps_wrap_around_at_either_end() {
        assert_eq!(step_index(Some(2), 3, Step::Next), Some(0), "after the last comes the first");
        assert_eq!(step_index(Some(0), 3, Step::Previous), Some(2), "before the first, the last");
        assert_eq!(step_index(Some(1), 3, Step::Next), Some(2));
        assert_eq!(step_index(None, 3, Step::Next), Some(0));
        assert_eq!(step_index(None, 3, Step::Previous), Some(2));
        assert_eq!(step_index(Some(7), 3, Step::Previous), Some(2), "a current one past the end");
        assert_eq!(step_index(None, 0, Step::Next), None, "nothing to move to");
    }
}
