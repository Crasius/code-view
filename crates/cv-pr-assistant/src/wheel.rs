//! Whether the card's conversation takes the mouse wheel. The card sits in the Overview's
//! scrolling side column; a conversation that took every wheel turn under the pointer would stop
//! the page wherever the card passes under it. So the conversation scrolls with the wheel only
//! once the user clicked into the card or moved the keyboard focus into it: the card is armed.
//! Until then the wheel scrolls the page (egui's [`egui::scroll_area::ScrollSource`] without the
//! wheel); its scroll bar can always be dragged. A click elsewhere, Esc, or a pass without the card
//! (another tab, another pull request, the expanded panel) gives the wheel back to the page. The
//! armed card wears the theme's focus stroke, so the difference is visible.
//!
//! The expanded panel fills the visible side column: nothing around it scrolls, so its
//! conversation always takes the wheel.

use egui::{Context, Id, Key, Rect};

#[derive(Default)]
pub struct CardWheel {
    is_armed: bool,
    /// The pass the card was last drawn in.
    shown_pass: Option<u64>,
    /// The focused widget when the card last looked, to notice the focus moving.
    focus: Option<Id>,
}

impl CardWheel {
    pub fn is_armed(&self) -> bool {
        self.is_armed
    }

    /// Before the card draws: a card that was not on screen the pass before starts unarmed, and
    /// Esc disarms it (read before the composer or the page takes the key).
    pub fn begin(&mut self, ctx: &Context) {
        let pass = ctx.cumulative_pass_nr();
        let was_shown = self.shown_pass.is_some_and(|shown| shown + 1 >= pass);
        self.shown_pass = Some(pass);
        if !was_shown {
            self.is_armed = false;
            self.focus = ctx.memory(egui::Memory::focused);
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.is_armed = false;
        }
    }

    /// After the card was drawn in `card`: the keyboard focus moving into the card arms it, moving
    /// to a widget outside disarms it; a press arms it inside and disarms it outside.
    pub fn end(&mut self, ctx: &Context, card: Rect) {
        let focus = ctx.memory(egui::Memory::focused);
        if focus != self.focus {
            self.focus = focus;
            if let Some(id) = focus {
                self.is_armed =
                    ctx.read_response(id).is_some_and(|widget| card.contains_rect(widget.rect));
            }
        }
        let press = ctx.input(|input| {
            input.pointer.any_pressed().then(|| input.pointer.press_origin()).flatten()
        });
        if let Some(at) = press {
            self.is_armed = card.contains(at);
        }
    }
}
