//! The summary's keyboard: ↑/↓ scroll a few rows, Page Up/Down a screen, Home/End to the ends;
//! n/p (F7, Shift+F7) move between cards and Enter opens the current one. Reading the keys is
//! here, and so is carrying them out on the summary ([`apply`]). The scroll keys and where they
//! lead are [`ScrollKey`]'s; where n/p lead is worked out by [`step_target`].

use egui::{Key, Modifiers, Ui};

use crate::layout::Geometry;
use crate::{FolderSummary, MARGIN, SummaryEvent};
use cv_base::platform;
use cv_widgets::focus;
use cv_widgets::scroll_keys::ScrollKey;

/// Rows the arrow keys scroll.
const ARROW_ROWS: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Next,
    Previous,
}

/// The summary's keys pressed this frame.
pub(crate) struct Pressed {
    pub scroll: Option<ScrollKey>,
    pub step: Option<Step>,
    pub open: bool,
}

/// Consumes the summary's keys. Scrolling and Enter only while no widget has the keyboard, and
/// nothing while a text field has it. Shift+↑/↓ extend a selection in a card instead.
pub(crate) fn read(ui: &Ui) -> Option<Pressed> {
    let ctx = ui.ctx();
    if ctx.text_edit_focused() {
        return None;
    }
    let has_focus = ctx.memory(|memory| memory.focused().is_some());
    Some(ui.input_mut(|input| {
        let scroll = if has_focus { None } else { ScrollKey::consume(input) };
        // Plain keys only: Alt+N is New only, Shift+F7 the previous card.
        let next =
            platform::consume_plain(input, Key::N) || platform::consume_plain(input, Key::F7);
        let previous =
            platform::consume_plain(input, Key::P) || input.consume_key(Modifiers::SHIFT, Key::F7);
        let step = if next { Some(Step::Next) } else { previous.then_some(Step::Previous) };
        let open = !has_focus && input.consume_key(Modifiers::NONE, Key::Enter);
        Pressed { scroll, step, open }
    }))
}

/// Carries out the summary's keys on `summary`: scrolls, moves the current card or opens it. Only
/// while no field or button has the keyboard (see [`read`]).
pub(crate) fn apply(
    summary: &mut FolderSummary,
    ui: &Ui,
    geometry: &Geometry,
) -> Option<SummaryEvent> {
    let pressed = read(ui)?;
    let ctx = ui.ctx();
    let viewport = summary.viewport;
    let limit = summary.scroll_limit();
    if let Some(scroll) = pressed.scroll {
        let top = viewport.top() + MARGIN;
        let page = (viewport.height() - geometry.header).max(geometry.line);
        let target = scroll.target(top, geometry.line * ARROW_ROWS, page, limit);
        summary.scroll_to = Some(target.clamp(0.0, limit));
        ctx.request_repaint();
    }
    if let Some(step) = pressed.step {
        let shown =
            summary.current.unwrap_or_else(|| summary.index.at(viewport.top() + 1.0).unwrap_or(0));
        let starts_in_view = summary.index.top(shown) >= viewport.top() - 1.0;
        let card = step_target(step, summary.current, shown, starts_in_view, summary.slots.len());
        summary.current = Some(card);
        // The card's top at the top of the view, its margin above it.
        summary.scroll_to = Some(summary.index.top(card).clamp(0.0, limit));
        focus::use_keyboard(ctx);
        ctx.request_repaint();
    }
    if pressed.open {
        let card = summary.current.or_else(|| summary.index.at(viewport.top() + 1.0))?;
        return summary.slots.get(card).map(|slot| SummaryEvent::View(slot.file));
    }
    None
}

/// Where n/p lead among `count` cards: from the current card, or, with none, from the card at
/// the top of the view (`shown`, whose top `starts_in_view` or lies above the view).
pub(crate) fn step_target(
    step: Step,
    current: Option<usize>,
    shown: usize,
    starts_in_view: bool,
    count: usize,
) -> usize {
    let last = count.saturating_sub(1);
    match (step, current) {
        (Step::Next, Some(current)) => (current + 1).min(last),
        // The card at the top when it starts there, else the next one.
        (Step::Next, None) if starts_in_view => shown,
        (Step::Next, None) => (shown + 1).min(last),
        (Step::Previous, Some(current)) => current.saturating_sub(1),
        (Step::Previous, None) => shown,
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, RawInput};

    use super::{Step, read, step_target};

    /// The step [`read`] makes of one press of `key` with `modifiers`.
    fn step_of(key: Key, modifiers: Modifiers) -> Option<Step> {
        let ctx = egui::Context::default();
        let event = Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let input = RawInput { events: vec![event], ..RawInput::default() };
        let mut step = None;
        let output = ctx.run_ui(input, |ui| step = read(ui).and_then(|pressed| pressed.step));
        output.drop_without_applying_deltas();
        step
    }

    #[test]
    fn shift_f7_steps_back_and_alt_n_is_no_step() {
        assert_eq!(step_of(Key::F7, Modifiers::NONE), Some(Step::Next));
        assert_eq!(step_of(Key::F7, Modifiers::SHIFT), Some(Step::Previous), "not F7's Next");
        assert_eq!(step_of(Key::N, Modifiers::NONE), Some(Step::Next));
        assert_eq!(step_of(Key::N, Modifiers::ALT), None, "Alt+N is New only");
    }

    #[test]
    fn n_and_p_step_from_the_current_card_and_stop_at_the_ends() {
        assert_eq!(step_target(Step::Next, Some(2), 0, true, 5), 3);
        assert_eq!(step_target(Step::Next, Some(4), 0, true, 5), 4, "the last card stays");
        assert_eq!(step_target(Step::Previous, Some(2), 0, true, 5), 1);
        assert_eq!(step_target(Step::Previous, Some(0), 0, true, 5), 0, "the first card stays");
    }

    #[test]
    fn without_a_current_card_n_and_p_start_at_the_top_of_the_view() {
        assert_eq!(step_target(Step::Next, None, 3, true, 9), 3, "a card that starts in view");
        assert_eq!(step_target(Step::Next, None, 3, false, 9), 4, "one scrolled half away");
        assert_eq!(step_target(Step::Next, None, 8, false, 9), 8, "but never past the last");
        assert_eq!(step_target(Step::Previous, None, 3, false, 9), 3);
    }
}
