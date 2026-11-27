//! The summary's keyboard: ↑/↓ scroll a few rows, Page Up/Down a screen, Home/End to the ends;
//! n/p (F7, Shift+F7) move between cards and Enter opens the current one. Reading the keys is
//! here, and so is carrying them out on the summary ([`apply`]); where they lead is worked out by
//! the pure functions below.

use egui::{Key, Modifiers, Ui};

use crate::layout::Geometry;
use crate::{FolderSummary, MARGIN, SummaryEvent};
use cv_base::platform;
use cv_widgets::focus;

/// Rows the arrow keys scroll.
const ARROW_ROWS: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    Down,
    Up,
    PageDown,
    PageUp,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

/// The summary's keys pressed this frame.
pub struct Pressed {
    pub scroll: Option<Scroll>,
    pub step: Option<Step>,
    pub open: bool,
}

/// Consumes the summary's keys. Scrolling and Enter only while no widget has the keyboard, and
/// nothing while a text field has it. Shift+↑/↓ extend a selection in a card instead.
pub fn read(ui: &Ui) -> Option<Pressed> {
    let ctx = ui.ctx();
    if ctx.text_edit_focused() {
        return None;
    }
    let has_focus = ctx.memory(|memory| memory.focused().is_some());
    Some(ui.input_mut(|input| {
        // Shift+↑/↓ extend a selection in a card; egui would match them as ↑/↓ here.
        let is_plain = !input.modifiers.shift && !input.modifiers.alt;
        let scrolls = [
            (Key::ArrowDown, Scroll::Down),
            (Key::ArrowUp, Scroll::Up),
            (Key::PageDown, Scroll::PageDown),
            (Key::PageUp, Scroll::PageUp),
            (Key::Home, Scroll::Home),
            (Key::End, Scroll::End),
        ];
        // Every key is consumed, the first one in this order wins.
        let mut scroll = None;
        for (key, direction) in scrolls {
            if !has_focus && is_plain && input.consume_key(Modifiers::NONE, key) {
                scroll = scroll.or(Some(direction));
            }
        }
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
pub fn apply(summary: &mut FolderSummary, ui: &Ui, geometry: &Geometry) -> Option<SummaryEvent> {
    let pressed = read(ui)?;
    let ctx = ui.ctx();
    let viewport = summary.viewport;
    let limit = summary.scroll_limit();
    if let Some(scroll) = pressed.scroll {
        let top = viewport.top() + MARGIN;
        let page = (viewport.height() - geometry.header).max(geometry.line);
        let target = scroll_target(scroll, top, page, geometry.line, limit);
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

/// Where the view's top goes for `scroll` from `top`, unclamped: `page` is a screen of cards,
/// `line` one row, `end` the offset of the last screen.
pub fn scroll_target(scroll: Scroll, top: f32, page: f32, line: f32, end: f32) -> f32 {
    match scroll {
        Scroll::Down => top + line * ARROW_ROWS,
        Scroll::Up => top - line * ARROW_ROWS,
        Scroll::PageDown => top + page,
        Scroll::PageUp => top - page,
        Scroll::Home => 0.0,
        Scroll::End => end,
    }
}

/// Where n/p lead among `count` cards: from the current card, or, with none, from the card at
/// the top of the view (`shown`, whose top `starts_in_view` or lies above the view).
pub fn step_target(
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

    use super::{Scroll, Step, read, scroll_target, step_target};

    /// The step [`read`] makes of one press of `key` with `modifiers`.
    fn step_of(key: Key, modifiers: Modifiers) -> Option<Step> {
        let ctx = egui::Context::default();
        let event = Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let input = RawInput { events: vec![event], ..RawInput::default() };
        let mut step = None;
        let mut output = ctx.run_ui(input, |ui| step = read(ui).and_then(|pressed| pressed.step));
        output.textures_delta.clear();
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

    #[test]
    fn scroll_keys_move_by_rows_screens_and_to_the_ends() {
        let target = |scroll| scroll_target(scroll, 100.0, 500.0, 20.0, 4_000.0);
        assert_eq!(target(Scroll::Down), 160.0);
        assert_eq!(target(Scroll::Up), 40.0);
        assert_eq!(target(Scroll::PageDown), 600.0);
        assert_eq!(target(Scroll::PageUp), -400.0, "the caller clamps");
        assert_eq!(target(Scroll::Home), 0.0);
        assert_eq!(target(Scroll::End), 4_000.0);
    }
}
